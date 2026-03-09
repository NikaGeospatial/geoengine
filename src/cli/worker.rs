use crate::cli::plugins;
use crate::cli::plugins::{verify_arcgis_plugin_installed, verify_qgis_plugin_installed};
use crate::config::pixi::PixiConfig;
use crate::config::settings::Settings;
use crate::config::state::{self, sha256_bytes, WorkerState};
use crate::config::worker::WorkerConfig;
use crate::config::yaml_store;
use crate::docker::client::DockerClient;
use crate::docker::container::ContainerConfig;
use crate::docker::dockerfile;
use crate::docker::gpu::GpuConfig;
use crate::utils::versioning::{
    compare_versions, compare_worker_version, get_latest_worker_version, validate_version,
};
use anyhow::{Context, Result};
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Select};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
// ---------------------------------------------------------------------------
// JSON output structs (used by --json flags and plugin integration)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct WorkerListEntry {
    name: String,
    path: String,
    has_tool: bool,
    found: bool,
    description: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct WorkerDescription {
    name: String,
    description: Option<String>,
    version: Option<String>,
    version_built: Option<String>,
    inputs: Vec<InputDescriptionJson>,
    /// RFC3339 timestamp of the last `geoengine apply`
    #[serde(skip_serializing_if = "Option::is_none")]
    applied_at: Option<String>,
    /// RFC3339 timestamp of the last `geoengine build`
    #[serde(skip_serializing_if = "Option::is_none")]
    built_at: Option<String>,
    /// First 12 hex chars of the full geoengine.yaml SHA-256 hash
    #[serde(skip_serializing_if = "Option::is_none")]
    yaml_hash: Option<String>,
    /// First 12 hex chars of the command script SHA-256 hash
    #[serde(skip_serializing_if = "Option::is_none")]
    script_hash: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct InputDescriptionJson {
    name: String,
    #[serde(rename = "param_type")]
    param_type: String,
    required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    readonly: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<serde_yaml::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enum_values: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    filetypes: Option<Vec<String>>,
}

#[derive(Serialize, Deserialize)]
struct RunResult {
    status: String,
    exit_code: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    files: Vec<OutputFileInfo>,
}

#[derive(Serialize, Deserialize)]
struct OutputFileInfo {
    name: String,
    path: String,
    size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileFingerprint {
    size: u64,
    mtime_nanos: Option<u128>,
}

fn file_fingerprint(path: &Path) -> Option<FileFingerprint> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let mtime_nanos = meta
        .modified()
        .ok()
        .and_then(|mtime| mtime.duration_since(UNIX_EPOCH).ok())
        .map(|dur| dur.as_nanos());
    Some(FileFingerprint {
        size: meta.len(),
        mtime_nanos,
    })
}

fn collect_file_fingerprints_recursive(
    path: &Path,
    out: &mut HashMap<PathBuf, FileFingerprint>,
) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    let meta = fs::metadata(path)
        .with_context(|| format!("Failed to read metadata for {}", path.display()))?;

    if meta.is_file() {
        let normalized = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if let Some(fp) = file_fingerprint(&normalized) {
            out.insert(normalized, fp);
        }
        return Ok(());
    }

    if !meta.is_dir() {
        return Ok(());
    }

    for entry in fs::read_dir(path)
        .with_context(|| format!("Failed to read directory {}", path.display()))?
    {
        let entry = entry?;
        let child = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_file_fingerprints_recursive(&child, out)?;
        } else if file_type.is_file() {
            let normalized = child.canonicalize().unwrap_or(child);
            if let Some(fp) = file_fingerprint(&normalized) {
                out.insert(normalized, fp);
            }
        }
    }

    Ok(())
}

fn snapshot_file_fingerprints(paths: &[PathBuf]) -> HashMap<PathBuf, FileFingerprint> {
    let mut files = HashMap::new();
    for path in paths {
        if let Err(err) = collect_file_fingerprints_recursive(path, &mut files) {
            eprintln!(
                "Warning: failed to scan output path '{}': {}",
                path.display(),
                err
            );
        }
    }
    files
}

fn collect_output_files(
    writable_mount_roots: &[PathBuf],
    baseline_files: &HashMap<PathBuf, FileFingerprint>,
    writable_file_input_targets: &[PathBuf],
) -> Vec<OutputFileInfo> {
    let current_files = snapshot_file_fingerprints(writable_mount_roots);
    let mut candidate_paths: HashSet<PathBuf> = current_files
        .iter()
        .filter_map(|(path, current_fp)| match baseline_files.get(path) {
            None => Some(path.clone()),
            Some(prev_fp) if prev_fp != current_fp => Some(path.clone()),
            _ => None,
        })
        .collect();

    for path in writable_file_input_targets {
        if path.is_file() {
            let normalized = path.canonicalize().unwrap_or_else(|_| path.clone());
            candidate_paths.insert(normalized);
        }
    }

    let mut files: Vec<OutputFileInfo> = candidate_paths
        .into_iter()
        .filter_map(|path| {
            let meta = fs::metadata(&path).ok()?;
            if !meta.is_file() {
                return None;
            }
            Some(OutputFileInfo {
                name: path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string(),
                path: path.to_string_lossy().to_string(),
                size: meta.len(),
                kind: Some("output".to_string()),
            })
        })
        .collect();

    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

fn collect_input_file_infos(paths: &[PathBuf]) -> Vec<OutputFileInfo> {
    let mut dedup = HashSet::new();
    let mut files: Vec<OutputFileInfo> = Vec::new();

    for path in paths {
        let normalized = path.canonicalize().unwrap_or_else(|_| path.clone());
        if !dedup.insert(normalized.clone()) {
            continue;
        }

        let Ok(meta) = fs::metadata(&normalized) else {
            continue;
        };
        if !meta.is_file() {
            continue;
        }

        files.push(OutputFileInfo {
            name: normalized
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string(),
            path: normalized.to_string_lossy().to_string(),
            size: meta.len(),
            kind: Some("input".to_string()),
        });
    }

    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

// ---------------------------------------------------------------------------
// geoengine init
// ---------------------------------------------------------------------------

pub async fn init_worker(name: Option<&str>, env: Option<&str>) -> Result<()> {
    // Validate env early to avoid partial writes on invalid input
    match env {
        None | Some("py") | Some("r") => {}
        Some(invalid) => anyhow::bail!(
            "Invalid --env: {}. Use either {} or {} and try again.",
            invalid,
            "r".bold(),
            "py".bold()
        ),
    }

    let current_dir = std::env::current_dir()?;
    let config_path = current_dir.join("geoengine.yaml");

    let mut replaced_pixi = false;
    let mut replaced_conf = false;

    let worker_name = name.map(|s| s.to_string()).unwrap_or_else(|| {
        current_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("my-worker")
            .to_string()
    });

    if config_path.exists() {
        let replace = Select::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "geoengine.yaml already exists in {}. Overwrite existing geoengine.yaml?",
                current_dir.display()
            ))
            .items(&["Yes", "No"])
            .default(1)
            .interact()?;

        replaced_conf = match replace {
            0 => true,
            _ => false,
        }
    }

    if !config_path.exists() || replaced_conf {
        let template = WorkerConfig::template(&worker_name);
        let yaml = serde_yaml::to_string(&template)?;
        std::fs::write(&config_path, yaml)?;
        println!(
            "{} Created {} in {}",
            "✓".green().bold(),
            "geoengine.yaml".cyan(),
            current_dir.display()
        );
    }

    let pixitoml_path = current_dir.join("pixi.toml");

    if pixitoml_path.exists() {
        let replace_pixi = Select::with_theme(&ColorfulTheme::default())
            .with_prompt(format!(
                "pixi.toml already exists in {}. Overwrite existing pixi.toml?",
                current_dir.display()
            ))
            .items(&["Yes", "No"])
            .default(1)
            .interact()?;

        replaced_pixi = match replace_pixi {
            0 => true,
            _ => false,
        };
    }

    if !pixitoml_path.exists() || replaced_pixi {
        let toml_template = match env {
            Some("r") => PixiConfig::r_template(&worker_name),
            _ => PixiConfig::py_template(&worker_name),
        };
        let pixi_toml = toml::to_string(&toml_template)?;
        fs::write(&pixitoml_path, pixi_toml)?;
        println!(
            "{} Created {} in {}",
            "✓".green().bold(),
            "pixi.toml".cyan(),
            current_dir.display()
        );
    }

    println!("\nNext steps:");
    println!("  1. Edit geoengine.yaml and pixi.toml to configure your worker");
    println!(
        "  2. Run {} to register and build",
        "geoengine apply".cyan()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// geoengine build
// ---------------------------------------------------------------------------

pub async fn build_worker_local(
    no_cache: bool,
    dev: bool,
    build_args: &[String],
    verbose: bool,
) -> Result<()> {
    let (worker_name, _) = resolve_worker_from_cwd();
    build_worker(&worker_name, no_cache, dev, build_args, verbose).await
}

async fn local_image_exists(client: &DockerClient, image_name: &str) -> Result<bool> {
    let images = client
        .list_images(Some(image_name), true)
        .await
        .with_context(|| format!("Failed to verify local image '{}'", image_name))?;

    Ok(images
        .into_iter()
        .any(|image| image.repo_tags.into_iter().any(|tag| tag == image_name)))
}

pub async fn build_worker(
    worker: &str,
    no_cache: bool,
    dev: bool,
    build_args: &[String],
    verbose: bool,
) -> Result<()> {
    let settings = Settings::load()?;
    let worker_path = settings.get_worker_path(worker)?;
    let config = yaml_store::load_saved_config(worker)?;

    let new_version = config.version.clone();
    let build_image_tag = if dev {
        format!("geoengine-local-dev/{}:latest", config.name)
    } else {
        format!("geoengine-local/{}:{}", config.name, new_version)
    };

    // --- File change detection ---
    let dockerfile = worker_path.join("Dockerfile");
    if !dockerfile.exists() {
        anyhow::bail!("Dockerfile not found: {}", dockerfile.display());
    }

    let yaml_build_hash = config.build_relevant_hash();
    let dockerfile_hash = Some(state::compute_file_hash(&dockerfile)?);

    // Hash the command script file (e.g. main.py) so changes to it trigger a rebuild
    let command_hash: Option<String> = config.command.as_ref().and_then(|cmd| {
        let script_path = worker_path.join(&cmd.script);
        if script_path.exists() {
            state::compute_file_hash(&script_path).ok()
        } else {
            None
        }
    });

    let prev_state = state::load_state(worker)?;
    let pushed_build_hash = match dev {
        true => prev_state
            .as_ref()
            .and_then(|s| s.pushed_build_hash.clone()),
        false => Some(
            yaml_build_hash.clone()
                + dockerfile_hash.as_ref().unwrap_or(&"".to_string())
                + command_hash.as_ref().unwrap_or(&"".to_string()),
        ),
    };
    let files_changed = match dev {
        true => match &prev_state {
            Some(prev) => {
                prev.yaml_build_hash != yaml_build_hash
                    || prev.dockerfile_hash != dockerfile_hash
                    || prev.command_hash != command_hash
            }
            None => true,
        },
        false => match &prev_state {
            Some(prev) => pushed_build_hash != prev.pushed_build_hash,
            None => true,
        },
    };

    let client = DockerClient::new().await;

    if !no_cache && !files_changed {
        match &client {
            Ok(client) => match local_image_exists(client, &build_image_tag).await {
                Ok(true) => {
                    println!(
                        "{} No build-related changes detected for worker '{}'. Nothing to build.",
                        "✓".green().bold(),
                        worker.cyan()
                    );
                    return Ok(());
                }
                Ok(false) => {
                    println!(
                        "{} Cached image '{}' was not found locally. Rebuilding.",
                        "!".yellow().bold(),
                        build_image_tag.as_str().cyan()
                    );
                }
                Err(e) => {
                    println!("{} {}. Rebuilding.", "!".yellow().bold(), e);
                }
            },
            Err(e) => {
                println!(
                    "{} Could not verify cached image '{}': {}. Attempting rebuild.",
                    "!".yellow().bold(),
                    build_image_tag.as_str().cyan(),
                    e
                );
            }
        }
    }

    // --- Version validation (Docker-backed) ---
    // Reuse the same Docker client initialized above.
    let client = client?;
    let ver_cmp = compare_worker_version(worker, &new_version, &client).await;
    let version_changed = match ver_cmp {
        Ok(c) => match c {
            Ordering::Less => {
                let latest_built = get_latest_worker_version(worker, &client)
                    .await
                    .unwrap_or_default();
                if dev {
                    println!(
                        "{} Version is lower than latest built version: {} < {}",
                        "!".yellow().bold(),
                        new_version,
                        latest_built
                    );
                    println!("{} Correct it before your next push.", " ");
                } else {
                    anyhow::bail!(
                        "{}\n{}: {}\n{}: {}",
                        "New version cannot be lower than latest built version!"
                            .red()
                            .bold(),
                        "New version  ",
                        new_version,
                        "Built version",
                        latest_built
                    );
                }
                true
            }
            Ordering::Equal => false,
            Ordering::Greater => true,
        },
        Err(e) => {
            if dev {
                println!("{} {}", "!".yellow().bold(), e);
                println!("{} Correct it before your next push.", " ");
                true
            } else {
                anyhow::bail!(e.red().bold());
            }
        }
    };

    if !no_cache && files_changed && !dev && !version_changed {
        anyhow::bail!(
            "{}\n  Build-related files have changed, but the version ('{}') has not been incremented.\n  Please bump the version in geoengine.yaml before rebuilding.",
            "Cannot rebuild without a version increment.".red().bold(),
            new_version
        );
    }

    // --- Build ---
    println!(
        "{} Building worker '{}'...",
        "=>".blue().bold(),
        worker.cyan()
    );

    let context = worker_path.clone();
    let image_tag = if dev {
        prev_state.as_ref().and_then(|s| s.image_tag.clone())
    } else {
        Some(build_image_tag.clone())
    };

    // Parse build args from CLI only
    let mut args: HashMap<String, String> = HashMap::new();
    for arg in build_args {
        let parts: Vec<&str> = arg.splitn(2, '=').collect();
        if parts.len() == 2 {
            args.insert(parts[0].to_string(), parts[1].to_string());
        }
    }

    let (pb, step_pb, step_task, progress_step_tx) = if verbose {
        (None, None, None, None)
    } else {
        let mp = MultiProgress::new();

        let pb = mp.add(ProgressBar::new_spinner());
        pb.set_style(ProgressStyle::default_spinner().template("{spinner:.green} {msg}")?);
        pb.set_message("Building image...");
        pb.enable_steady_tick(std::time::Duration::from_millis(100));

        let step_pb = mp.add(ProgressBar::new_spinner());
        step_pb.set_style(ProgressStyle::with_template("  {msg}")?);
        step_pb.set_message(" ");

        let (progress_step_tx, mut progress_step_rx) =
            tokio::sync::mpsc::unbounded_channel::<String>();
        let step_pb_clone = step_pb.clone();
        let step_task = tokio::spawn(async move {
            while let Some(step) = progress_step_rx.recv().await {
                step_pb_clone.set_message(step);
            }
        });

        (
            Some(pb),
            Some(step_pb),
            Some(step_task),
            Some(progress_step_tx),
        )
    };

    let build_result = client
        .build_image(
            &dockerfile,
            &context,
            &build_image_tag,
            &args,
            no_cache,
            verbose,
            progress_step_tx,
        )
        .await;

    if let Some(step_task) = step_task {
        let _ = step_task.await;
    }

    if let Some(step_pb) = step_pb {
        step_pb.finish_and_clear();
    }
    if let Some(pb) = pb {
        pb.finish_and_clear();
    }

    build_result?;
    println!(
        "{} Successfully built image: {}",
        "✓".green().bold(),
        build_image_tag.cyan()
    );

    // --- Update state with new hashes after successful build ---
    let prev_state = state::load_state(worker)?;
    let now = chrono::Utc::now().to_rfc3339();
    let new_state = WorkerState {
        worker_name: worker.to_string(),
        applied_at: prev_state
            .as_ref()
            .map(|s| s.applied_at.clone())
            .unwrap_or_else(|| now.clone()),
        built_at: Some(now),
        yaml_build_hash,
        yaml_hash: prev_state.as_ref().and_then(|s| s.yaml_hash.clone()),
        dockerfile_hash,
        command_hash,
        pushed_build_hash,
        image_tag,
        script: prev_state.as_ref().and_then(|s| s.script.clone()),
        plugins_arcgis: prev_state.as_ref().and_then(|s| s.plugins_arcgis),
        plugins_qgis: prev_state.as_ref().and_then(|s| s.plugins_qgis),
    };
    state::save_state(&new_state)?;

    Ok(())
}

// ---------------------------------------------------------------------------
// geoengine apply
// ---------------------------------------------------------------------------

pub async fn apply_worker(worker: Option<&str>, _force: bool) -> Result<()> {
    // Resolve the worker
    let (worker_name, worker_path) = if let Some(name) = worker {
        let settings = Settings::load()?;
        match settings.get_worker_path(name) {
            Ok(path) => (name.to_string(), path),
            Err(_) => {
                // Try as a path
                let path = PathBuf::from(name);
                if path.join("geoengine.yaml").exists() {
                    let config = WorkerConfig::load(&path.join("geoengine.yaml"))?;
                    (config.name.clone(), path.canonicalize()?)
                } else {
                    anyhow::bail!(
                        "Worker '{}' not found and no geoengine.yaml at that path.",
                        name
                    );
                }
            }
        }
    } else {
        let cwd = std::env::current_dir()?;
        let config_path = cwd.join("geoengine.yaml");
        if !config_path.exists() {
            anyhow::bail!("No geoengine.yaml found in current directory. Specify a worker name or run from a worker directory.");
        }

        let settings = Settings::load()?;
        // Try to find registered worker by cwd path
        match settings.find_worker_by_path(&cwd) {
            Some((name, path)) => {
                // Check if the name in geoengine.yaml has changed
                let config = WorkerConfig::load(&config_path)?;
                if config.name != name {
                    // Name changed in YAML — update registration
                    let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());
                    let mut settings = settings;
                    settings.unregister_worker(&name)?;
                    settings.register_worker(&config.name, &canonical)?;
                    settings.save()?;

                    // Migrate state and saved config to the new name
                    state::rename_state(&name, &config.name)?;
                    yaml_store::rename_saved_config(&name, &config.name)?;

                    println!(
                        "{} Worker renamed from '{}' to '{}' — registration updated.",
                        "✓".green().bold(),
                        name.cyan(),
                        config.name.cyan()
                    );
                    (config.name.clone(), canonical)
                } else {
                    (name, path)
                }
            }
            None => {
                // Path not registered — try matching by worker name from the YAML
                let config = WorkerConfig::load(&config_path)?;
                let canonical = cwd.canonicalize().unwrap_or_else(|_| cwd.clone());

                if settings.workers.contains_key(&config.name) {
                    // Worker name exists but at a different path — update path
                    let mut settings = settings;
                    let old_path = settings.workers.get(&config.name).cloned().unwrap();
                    settings.register_worker(&config.name, &canonical)?;
                    settings.save()?;
                    println!(
                        "{} Worker '{}' path updated: {} → {}",
                        "✓".green().bold(),
                        config.name.cyan(),
                        old_path.display(),
                        canonical.display()
                    );
                    (config.name.clone(), canonical)
                } else {
                    // Completely new worker — will be registered below
                    (config.name.clone(), cwd)
                }
            }
        }
    };

    // Load current config from YAML and detect changes from saved state. If changed, save it, if not exit.
    let config_changed = yaml_store::check_changed_config(&worker_name, &worker_path)?;
    if !config_changed && worker_path.clone().join("Dockerfile").exists() {
        println!(
            "{} No changes detected in geoengine.yaml of worker '{}'. Nothing to apply.",
            "!".yellow().bold(),
            worker_name
        );
        return Ok(());
    }
    let config = WorkerConfig::load(&worker_path.join("geoengine.yaml"))?;
    verify_worker_config_path_types(&config, &worker_path)?;
    if config.command.is_none() {
        anyhow::bail!(
            "No command specified in geoengine.yaml of worker '{}'. Cannot apply.",
            worker_name
        );
    }
    yaml_store::save_config(&config)?;

    // Auto-register if not already registered
    let mut settings = Settings::load()?;
    if settings.workers.get(&worker_name).is_none() {
        let canonical = worker_path
            .canonicalize()
            .unwrap_or_else(|_| worker_path.clone());
        settings.register_worker(&worker_name, &canonical)?;
        settings.save()?;
        println!(
            "{} Registered worker '{}' at {}",
            "✓".green().bold(),
            worker_name.cyan(),
            canonical.display()
        );
    } else {
        println!(
            "{} Worker '{}' is already registered. Updated geoengine.yaml saved.",
            "✓".green().bold(),
            worker_name.cyan()
        );
    }

    // Load previous state for plugin comparison
    let prev_state = state::load_state(&worker_name)?;

    // Generate the Dockerfile if not already done so
    if !worker_path.clone().join("Dockerfile").exists() {
        if config.command.is_some() {
            dockerfile::generate_dockerfile(&worker_path)?;
            println!("{} Dockerfile generated.", "✓".green().bold(),);
        } else {
            anyhow::bail!("No command specified in geoengine.yaml of worker '{}'. Cannot generate Dockerfile.", worker_name);
        }
    }

    // Detect and apply plugin changes
    let cur_arcgis = config
        .plugins
        .as_ref()
        .and_then(|p| p.arcgis)
        .unwrap_or(false);
    let cur_qgis = config
        .plugins
        .as_ref()
        .and_then(|p| p.qgis)
        .unwrap_or(false);
    let prev_arcgis = prev_state
        .as_ref()
        .and_then(|s| s.plugins_arcgis)
        .unwrap_or(false);
    let prev_qgis = prev_state
        .as_ref()
        .and_then(|s| s.plugins_qgis)
        .unwrap_or(false);

    let mut res_arcgis = cur_arcgis.clone();
    let mut res_qgis = cur_qgis.clone();
    let yaml_path = worker_path.join("geoengine.yaml");
    let yaml_content_u8 = std::fs::read(&yaml_path)
        .with_context(|| format!("Failed to read file for hashing: {}", yaml_path.display()))?;
    let mut yaml_content = String::from_utf8(yaml_content_u8)?;

    let mut plugin_change_msgs: Vec<String> = Vec::new();
    let mut yaml_dirty = false;

    if cur_arcgis != prev_arcgis {
        if cur_arcgis && !verify_arcgis_plugin_installed()? {
            let install_arcgis = Select::with_theme(&ColorfulTheme::default())
                .with_prompt("ArcGIS plugin is not installed yet. Would you like to do so?")
                .items(&["Yes", "No"])
                .default(0)
                .interact()?;
            match install_arcgis {
                0 => {
                    match plugins::register_arcgis(None).await {
                        Ok(_) => {
                            res_arcgis = true;
                            plugin_change_msgs.push(format!(
                                "{} {} {}",
                                "•".yellow(),
                                "ArcGIS plugin installed.".to_string(),
                                "Please restart ArcGIS to see the plugin!".bold()
                            ));
                            plugin_change_msgs.push(format!(
                                "{} {}",
                                "✓".green(),
                                "Tool registered in ArcGIS plugin.".to_string()
                            ));
                        }
                        Err(e) => {
                            res_arcgis = false;
                            set_plugin_flag_in_yaml(&mut yaml_content, "arcgis", false)?;
                            yaml_dirty = true;
                            plugin_change_msgs.push(format!(
                                "{} {}",
                                "×".red(),
                                format!("ArcGIS plugin NOT installed: {}.", e).to_string()
                            ));
                            plugin_change_msgs.push(format!(
                                "{} {}",
                                "×".red(),
                                "Tool not registered. YAML reverted.".to_string()
                            ));
                        }
                    };
                }
                _ => {
                    res_arcgis = false;
                    set_plugin_flag_in_yaml(&mut yaml_content, "arcgis", false)?;
                    yaml_dirty = true;
                    plugin_change_msgs.push(format!(
                        "{} {}",
                        "×".red(),
                        "ArcGIS plugin installation rejected.".to_string()
                    ));
                    plugin_change_msgs.push(format!(
                        "{} {}",
                        "×".red(),
                        "Tool not registered. YAML reverted.".to_string()
                    ));
                }
            }
        } else if cur_arcgis && verify_arcgis_plugin_installed()? {
            plugin_change_msgs.push(format!(
                "{} {}",
                "✓".green(),
                format!("Tool {} in ArcGIS plugin.", "registered".green())
            ));
        } else if !cur_arcgis && verify_arcgis_plugin_installed()? {
            plugin_change_msgs.push(format!(
                "{} {}",
                "✓".green(),
                format!("Tool {} from ArcGIS plugin.", "de-registered".red())
            ));
        };
    }

    if cur_qgis != prev_qgis {
        if cur_qgis && !verify_qgis_plugin_installed()? {
            let install_qgis = Select::with_theme(&ColorfulTheme::default())
                .with_prompt("QGIS plugin is not installed yet. Would you like to do so?")
                .items(&["Yes", "No"])
                .default(0)
                .interact()?;
            match install_qgis {
                0 => {
                    match plugins::register_qgis(None).await {
                        Ok(_) => {
                            res_qgis = true;
                            plugin_change_msgs.push(format!(
                                "{} {} {}",
                                "•".yellow(),
                                "QGIS plugin installed.".to_string(),
                                "Please restart QGIS to see the plugin!".bold()
                            ));
                            plugin_change_msgs.push(format!(
                                "{} {}",
                                "✓".green(),
                                "Tool registered in QGIS plugin.".to_string()
                            ))
                        }
                        Err(e) => {
                            res_qgis = false;
                            set_plugin_flag_in_yaml(&mut yaml_content, "qgis", false)?;
                            yaml_dirty = true;
                            plugin_change_msgs.push(format!(
                                "{} {}",
                                "×".red(),
                                format!("QGIS plugin NOT installed: {}.", e).to_string()
                            ));
                            plugin_change_msgs.push(format!(
                                "{} {}",
                                "×".red(),
                                "Tool not registered. YAML reverted.".to_string()
                            ));
                        }
                    };
                }
                _ => {
                    res_qgis = false;
                    set_plugin_flag_in_yaml(&mut yaml_content, "qgis", false)?;
                    yaml_dirty = true;
                    plugin_change_msgs.push(format!(
                        "{} {}",
                        "×".red(),
                        "QGIS plugin NOT installed. Tool not registered.".to_string()
                    ));
                    plugin_change_msgs.push(format!(
                        "{} {}",
                        "×".red(),
                        "Tool not registered. YAML reverted.".to_string()
                    ));
                }
            }
        } else if cur_qgis && verify_qgis_plugin_installed()? {
            plugin_change_msgs.push(format!(
                "{} {}",
                "✓".green(),
                format!("Tool {} in QGIS plugin.", "registered".green())
            ));
        } else if !cur_qgis && verify_qgis_plugin_installed()? {
            plugin_change_msgs.push(format!(
                "{} {}",
                "✓".green(),
                format!("Tool {} from QGIS plugin.", "de-registered".red())
            ));
        };
    }

    if !plugin_change_msgs.is_empty() {
        println!("{} Plugin changes applied:", "=>".blue().bold());
        for change_msg in &plugin_change_msgs {
            println!("  {}", change_msg);
        }
    } else {
        println!("{} No plugin changes detected", "✓".green().bold());
    }

    // If plugin flags were reverted, persist to disk and update saved config
    let config = if yaml_dirty {
        std::fs::write(&yaml_path, &yaml_content)
            .with_context(|| format!("Failed to write reverted YAML to {}", yaml_path.display()))?;
        let updated_config = WorkerConfig::load(&yaml_path)?;
        yaml_store::save_config(&updated_config)?;
        updated_config
    } else {
        config
    };

    // Recompute YAML hashes from current files; preserve build hashes from previous state.
    //    yaml_hash: full YAML file hash (used by `apply` change detection)
    //    yaml_build_hash: hash of build-relevant fields only (used by `build`)
    //    dockerfile_hash: preserved from previous state (used by `build`)
    //    command_hash: preserved from previous state (used by `build`)
    //    image_tag: only set by `build`, preserved from prev_state
    let yaml_hash = Some(sha256_bytes(yaml_content.as_bytes()));
    let yaml_build_hash = config.build_relevant_hash();
    let dockerfile_hash = prev_state
        .as_ref()
        .and_then(|prev| prev.dockerfile_hash.clone());
    let command_hash = prev_state
        .as_ref()
        .and_then(|prev| prev.command_hash.clone());
    let pushed_build_hash = prev_state
        .as_ref()
        .and_then(|prev| prev.pushed_build_hash.clone());
    let image_tag = match &prev_state {
        Some(prev) => prev.image_tag.clone(),
        None => None,
    };

    let script = Some(
        config
            .command
            .as_ref()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "No command specified in geoengine.yaml of worker '{}'",
                    worker_name
                )
            })?
            .script
            .clone(),
    );
    let built_at = prev_state.as_ref().and_then(|s| s.built_at.clone());
    let new_state = WorkerState {
        worker_name: worker_name.clone(),
        applied_at: chrono::Utc::now().to_rfc3339(),
        built_at,
        yaml_build_hash,
        yaml_hash,
        dockerfile_hash,
        command_hash,
        pushed_build_hash,
        image_tag,
        script,
        plugins_arcgis: Some(res_arcgis),
        plugins_qgis: Some(res_qgis),
    };
    state::save_state(&new_state)?;

    // 7. Touch QGIS refresh trigger so the plugin auto-reloads tools
    touch_qgis_refresh_trigger();

    println!(
        "{} Apply complete for worker '{}'",
        "✓".green().bold(),
        worker_name.cyan()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// geoengine delete
// ---------------------------------------------------------------------------

pub async fn delete_worker(name: Option<&str>) -> Result<()> {
    let worker_name = if let Some(n) = name {
        n.to_string()
    } else {
        let (name, _) = resolve_worker_from_cwd();
        name
    };

    let mut settings = Settings::load()?;

    // Check if worker exists
    let worker_path = match settings.workers.get(&worker_name) {
        Some(path) => path.clone(),
        None => anyhow::bail!("Worker '{}' is not registered", worker_name),
    };

    // Warn if the worker's directory no longer contains geoengine.yaml
    if !worker_path.join("geoengine.yaml").exists() {
        println!(
            "{} Worker directory no longer contains geoengine.yaml: {}",
            "!".yellow().bold(),
            worker_path.display()
        );
        println!("  The directory may have been moved or deleted.",);
    }

    // Remove from settings
    settings.unregister_worker(&worker_name)?;
    settings.save()?;

    // Clean up state file and saved config
    state::delete_state(&worker_name)?;
    yaml_store::delete_saved_config(&worker_name)?;

    // Touch QGIS refresh trigger so the plugin auto-reloads tools after delete
    touch_qgis_refresh_trigger();

    println!(
        "{} Deleted worker '{}'",
        "✓".green().bold(),
        worker_name.cyan()
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// geoengine run
// ---------------------------------------------------------------------------

pub async fn run_worker(
    worker: Option<&str>,
    input_args: &[String],
    json_output: bool,
    dev: bool,
    extra_args: &[String],
) -> Result<()> {
    // Resolve worker name and path
    let (worker_name, worker_path) = resolve_worker(worker)?;
    let config = yaml_store::load_saved_config(&worker_name)?;

    // Update if version changed
    let this_ver = config.version;

    // Get command config
    let cmd_config = config
        .command
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No command defined for worker '{}'", worker_name))?;

    // Parse --input KEY=VALUE args into a HashMap
    let mut inputs: HashMap<String, String> = HashMap::new();
    for arg in input_args {
        let parts: Vec<&str> = arg.splitn(2, '=').collect();
        if parts.len() != 2 {
            anyhow::bail!("Invalid input format: '{}'. Expected KEY=VALUE", arg);
        }
        inputs.insert(parts[0].to_string(), parts[1].to_string());
    }
    let mut client: Option<DockerClient> = None;

    // Get global GeoEngine settings
    let settings = Settings::load()?;

    // Build extra mounts from input values that are explicitly defined as
    // file/folder inputs in worker config.
    let mut extra_mounts: Vec<(String, String, bool)> = Vec::new();
    let mut writable_file_input_targets: Vec<PathBuf> = Vec::new();
    let mut readonly_input_files: Vec<PathBuf> = Vec::new();
    let input_definitions: HashMap<String, (String, bool, bool, Option<Vec<String>>)> = cmd_config
        .inputs
        .as_ref()
        .map(|defs| {
            defs.iter()
                .map(|d| {
                    (
                        d.name.clone(),
                        (
                            d.param_type.to_ascii_lowercase(),
                            d.readonly.unwrap_or(true),
                            d.required.unwrap_or(true),
                            d.filetypes.clone(),
                        ),
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    // Build script arguments from inputs
    let mut script_args: Vec<String> = Vec::new();
    for (key, value) in &inputs {
        let path = Path::new(match value.as_ref() {
            "NULL" => "",
            _ => value,
        });
        // Only auto-mount for declared file/folder inputs.
        // Ignore optional fields left blank.
        let processed_value = if let Some((param_type, readonly, required, filetypes)) =
            input_definitions.get(key)
        {
            match param_type.as_str() {
                "file" => {
                    // Check if path given is empty, and enforce if it is a required input.
                    // If path given is not empty, check if it indeed is a file (could be a directory).
                    // If path given is not empty and is a file, check if it exists and enforce existence if it is readonly, else just create it.
                    if path.as_os_str().is_empty() {
                        // If this parameter is not required, it can be empty
                        match required {
                            true => anyhow::bail!(
                                "Input '{}' is declared required but received an empty path: {}",
                                key,
                                value
                            ),
                            false => continue,
                        }
                    } else if !path.exists() {
                        match readonly {
                            true => anyhow::bail!(
                                "Input '{}' is declared as readonly but received a non-existent path: {}",
                                key,
                                value
                            ),
                            false => {
                                // Ensure Docker is reachable before mutating host paths.
                                if client.is_none() {
                                    client = Some(DockerClient::new().await?);
                                }
                                if let Some(parent) = path.parent() {
                                    if !parent.as_os_str().is_empty() {
                                        fs::create_dir_all(parent)?;
                                    }
                                }
                                File::create(path)?;
                            },
                        };
                    } else if !path.is_file() {
                        anyhow::bail!(
                            "Input '{}' is declared as type 'file' but received a non-file path: {}",
                            key,
                            value
                        );
                    }

                    // Validate file extension against accepted filetypes (early, before mounting).
                    // Only applies to readonly (input) files — for writable (output) files the
                    // script decides the format at runtime, so pre-validating the path extension
                    // is not meaningful and would block extension-free temp paths.
                    // None or [".*"] means all types are accepted.
                    if *readonly {
                        if let Some(accepted) = filetypes {
                            let accepts_all =
                                accepted.is_empty() || accepted.iter().any(|ft| ft == ".*");
                            if !accepts_all {
                                let ext = path
                                    .extension()
                                    .map(|e| {
                                        format!(".{}", e.to_string_lossy().to_ascii_lowercase())
                                    })
                                    .unwrap_or_default();
                                let matched =
                                    accepted.iter().any(|ft| ft.to_ascii_lowercase() == ext);
                                if !matched {
                                    anyhow::bail!(
                                        "Input '{}': file '{}' has extension '{}' but only {:?} are accepted",
                                        key,
                                        value,
                                        if ext.is_empty() { "(none)" } else { &ext },
                                        accepted
                                    );
                                }
                            }
                        }
                    }

                    let filename = path.file_name().ok_or_else(|| {
                        anyhow::anyhow!(
                            "Input '{}' is declared as type 'file' but has no file name: {}",
                            key,
                            value
                        )
                    })?;
                    let container_path = format!("/inputs/{}/{}", key, filename.to_string_lossy());
                    if *readonly {
                        // Readonly file input example: `--mask /data/masks/roi.tif` (must exist).
                        // Readonly: bind-mount the file directly (it must exist).
                        let abs_path = path.canonicalize().with_context(|| {
                            format!("Failed to resolve input file path: {}", value)
                        })?;
                        readonly_input_files.push(abs_path.clone());
                        extra_mounts.push((
                            abs_path.to_string_lossy().to_string(),
                            container_path.clone(),
                            true,
                        ));
                    } else {
                        // Writable file input example: `--report /tmp/out/report.json` (may be created).
                        // Writable: bind-mount the parent directory so the container path
                        // /inputs/<key>/ exists as a real directory. Mounting a single file
                        // into a non-existent container directory causes EACCES/ENOENT on write.
                        let parent = path.parent().ok_or_else(|| {
                            anyhow::anyhow!("Input '{}' has no parent directory: {}", key, value)
                        })?;
                        let abs_parent = parent.canonicalize().with_context(|| {
                            format!(
                                "Failed to resolve parent directory for input '{}': {}",
                                key, value
                            )
                        })?;
                        let abs_file = path.canonicalize().with_context(|| {
                            format!("Failed to resolve writable file input '{}': {}", key, value)
                        })?;
                        writable_file_input_targets.push(abs_file);
                        let container_dir = format!("/inputs/{}", key);
                        extra_mounts.push((
                            abs_parent.to_string_lossy().to_string(),
                            container_dir,
                            false,
                        ));
                    }
                    container_path
                }
                "folder" => {
                    // Folder input example: `--scratch /tmp/work` or `--config_dir /etc/geoengine`.
                    // Check if path given is empty, and enforce if it is a required input.
                    // If path given is not empty, check if it indeed is a directory (could be a file).
                    // If path given is not empty and is a directory, check if it exists and enforce existence if it is readonly, else just create it.
                    if path.as_os_str().is_empty() {
                        // If this parameter is not required, it can be empty
                        match required {
                            true => anyhow::bail!(
                                "Input '{}' is declared required but received an empty path: {}",
                                key,
                                value
                            ),
                            false => continue,
                        }
                    } else if !path.exists() {
                        // If readonly, require the folder to exist (e.g., `/data/input`).
                        // If writable, create it (e.g., `/tmp/work`).
                        match readonly {
                            true => anyhow::bail!(
                                "Input '{}' is declared as readonly but received a non-existent path: {}",
                                key,
                                value
                            ),
                            false => {
                                // Ensure Docker is reachable before mutating host paths.
                                if client.is_none() {
                                    client = Some(DockerClient::new().await?);
                                }
                                std::fs::create_dir_all(path)?
                            },
                        };
                    } else if !path.is_dir() {
                        anyhow::bail!(
                            "Input '{}' is declared as type 'folder' but received a non-directory path: {}",
                            key,
                            value
                        );
                    }

                    let abs_path = path.canonicalize().with_context(|| {
                        format!("Failed to resolve input directory path: {}", value)
                    })?;
                    let container_path = format!("/mnt/input_{}", key);
                    extra_mounts.push((
                        abs_path.to_string_lossy().to_string(),
                        container_path.clone(),
                        *readonly,
                    ));
                    container_path
                }
                _ => {
                    if value.is_empty() {
                        match required {
                            true => anyhow::bail!(
                                "Input '{}' is declared required but received an empty value: {}",
                                key,
                                value
                            ),
                            false => continue,
                        }
                    }
                    value.clone()
                }
            }
        } else {
            value.clone()
        };

        script_args.push(format!("--{}", key));
        script_args.push(processed_value);
    }

    // Add any extra trailing args
    script_args.extend_from_slice(extra_args);

    // Build mounts from config
    let mut mounts: Vec<(String, String, bool)> = Vec::new();
    if let Some(mount_configs) = &config.local_dir_mounts {
        for m in mount_configs {
            let host_path = if m.host_path.starts_with("./") {
                worker_path.join(&m.host_path[2..])
            } else {
                PathBuf::from(&m.host_path)
            };
            mounts.push((
                host_path.to_string_lossy().to_string(),
                m.container_path.clone(),
                m.readonly.unwrap_or(false),
            ));
        }
    }
    mounts.extend(extra_mounts);

    let writable_mount_roots: Vec<PathBuf> = mounts
        .iter()
        .filter(|(_, _, readonly)| !*readonly)
        .map(|(host_path, _, _)| PathBuf::from(host_path))
        .collect();

    let baseline_output_files = if json_output {
        snapshot_file_fingerprints(&writable_mount_roots)
    } else {
        HashMap::new()
    };

    // Build full command
    let full_command = if script_args.is_empty() {
        format!("{} {}", cmd_config.program, cmd_config.script)
    } else {
        let escaped_args: Vec<String> = script_args.iter().map(|a| shell_escape(a)).collect();
        format!(
            "{} {} {}",
            cmd_config.program,
            cmd_config.script,
            escaped_args.join(" ")
        )
    };

    // Auto-detect system GPUs — only keep configs that Docker can use (NVIDIA).
    // Metal (macOS) is handled natively by the container runtime and needs no
    // explicit device passthrough.
    let gpu_config = match GpuConfig::detect().await {
        Ok(cfg) if cfg.is_available() => {
            if !json_output {
                let label = cfg.devices.join(", ");
                eprintln!(
                    "{} GPU detected: {} ({})",
                    "•".cyan(),
                    label,
                    if cfg.is_nvidia() { "NVIDIA" } else { "Metal" }
                );
            }
            Some(cfg)
        }
        _ => None,
    };

    // Build ContainerConfig
    let image_tag = if dev {
        format!("geoengine-local-dev/{}:latest", config.name)
    } else {
        format!("geoengine-local/{}:{}", config.name, this_ver)
    };
    let container_config = ContainerConfig {
        image: image_tag,
        command: Some(vec!["/bin/sh".to_string(), "-c".to_string(), full_command]),
        env_vars: match settings.env {
            None => HashMap::new(),
            Some(env) => env.clone(),
        },
        mounts,
        gpu_config,
        workdir: None,
        name: None,
        remove_on_exit: true,
        detach: false,
        tty: !json_output,
        inject_host_user: true,
    };

    // Print status message
    if !json_output {
        eprintln!(
            "{} Running worker '{}'...",
            "=>".blue().bold(),
            worker_name.cyan()
        );
    }

    // Run the container
    if client.is_none() {
        client = Some(DockerClient::new().await?);
    }
    let client = client.as_ref().expect("Docker client must be initialized");
    let exit_code = if json_output {
        client
            .run_container_attached_to_stderr(&container_config)
            .await?
    } else {
        client.run_container_attached(&container_config).await?
    };

    // Handle output
    if json_output {
        let mut files = collect_output_files(
            &writable_mount_roots,
            &baseline_output_files,
            &writable_file_input_targets,
        );
        let input_files = collect_input_file_infos(&readonly_input_files);
        let mut by_path: HashMap<String, OutputFileInfo> = HashMap::new();
        for file in input_files {
            by_path.insert(file.path.clone(), file);
        }
        for file in files.drain(..) {
            // Outputs win over inputs for the same path.
            by_path.insert(file.path.clone(), file);
        }
        let mut merged: Vec<OutputFileInfo> = by_path.into_values().collect();
        merged.sort_by(|a, b| a.path.cmp(&b.path));
        let result = RunResult {
            status: if exit_code == 0 {
                "completed".to_string()
            } else {
                "failed".to_string()
            },
            exit_code,
            error: if exit_code != 0 {
                Some(format!("Container exited with code {}", exit_code))
            } else {
                None
            },
            files: merged,
        };
        println!("{}", serde_json::to_string(&result)?);
    } else if exit_code == 0 {
        eprintln!("{} Completed successfully", "✓".green().bold());
    } else {
        eprintln!("{} Failed with exit code {}", "✗".red().bold(), exit_code);
    }

    if exit_code != 0 {
        anyhow::bail!("Worker '{}' exited with code {}", worker_name, exit_code);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// geoengine describe
// ---------------------------------------------------------------------------

pub async fn describe_worker(worker: Option<&str>, json: bool) -> Result<()> {
    let (worker_name, _worker_path) = resolve_worker(worker)?;
    let config = yaml_store::load_saved_config(&worker_name)?;
    let inputs = config
        .command
        .as_ref()
        .and_then(|c| c.inputs.as_ref())
        .map(|inputs| {
            inputs
                .iter()
                .map(|i| InputDescriptionJson {
                    name: i.name.clone(),
                    param_type: i.param_type.clone(),
                    required: i.required.unwrap_or(true),
                    readonly: i.readonly,
                    default: i.default.clone(),
                    description: i.description.clone(),
                    enum_values: i.enum_values.clone(),
                    filetypes: i.filetypes.clone(),
                })
                .collect()
        })
        .unwrap_or_default();

    let worker_state = state::load_state(&worker_name).ok().flatten();
    let version_built = worker_state
        .as_ref()
        .and_then(|s| s.image_tag.as_deref())
        .map(|tag| tag.rsplit_once('/').map_or(tag, |(_, tail)| tail))
        .map(|tail| tail.rsplit_once(':').map_or(tail, |(_, version)| version))
        .map(|version| version.strip_prefix('v').unwrap_or(version).to_string());
    let applied_at = worker_state.as_ref().map(|s| s.applied_at.clone());
    let built_at = worker_state.as_ref().and_then(|s| s.built_at.clone());
    let yaml_hash = worker_state
        .as_ref()
        .and_then(|s| s.yaml_hash.as_ref().map(|h| short_hash(h)));
    let script_hash = worker_state
        .as_ref()
        .and_then(|s| s.command_hash.as_ref().map(|h| short_hash(h)));

    let desc = WorkerDescription {
        name: config.name.clone(),
        description: config.description.clone(),
        version: Some(config.version.clone()),
        version_built,
        inputs,
        applied_at,
        built_at,
        yaml_hash,
        script_hash,
    };

    if json {
        println!("{}", serde_json::to_string(&desc)?);
    } else {
        println!();
        println!("{:<13}: {}", "WORKER".bold(), desc.name);
        println!(
            "{:<13}: {}",
            "DESCRIPTION".bold(),
            match desc.description {
                Some(d) => d.normal(),
                None => "No description provided.".italic(),
            }
        );
        match desc.version {
            Some(v) => {
                println!(
                    "{:<13}: {} {}",
                    "VERSION".bold(),
                    v,
                    if validate_version(&v).is_ok() {
                        "".normal()
                    } else {
                        "✗".red()
                    }
                );
                let built_ver = desc.version_built;
                if built_ver.is_none() {
                    match validate_version(&v) {
                        Ok(_) => {
                            println!(
                                "{:<13}: {} {}",
                                "IMAGE VERSION".bold(),
                                "not built",
                                "↻".yellow()
                            );
                            println!(
                                "{}{}",
                                " ".repeat(15),
                                "Run `geoengine build` to build pushable image."
                                    .italic()
                                    .yellow()
                            );
                        }
                        Err(e) => {
                            println!("{:<13}: {}", "IMAGE VERSION".bold(), "not built");
                            println!("{}{}", " ".repeat(15), e.italic().red());
                        }
                    }
                } else {
                    let version_cmp = compare_versions(
                        &config.version.clone(),
                        built_ver.clone().unwrap().as_ref(),
                    );
                    match version_cmp {
                        Ok(order) => {
                            match order {
                                Ordering::Equal => {
                                    println!(
                                        "{:<13}: {} {}",
                                        "IMAGE VERSION".bold(),
                                        built_ver.unwrap(),
                                        "✓".green()
                                    );
                                }
                                Ordering::Greater => {
                                    println!(
                                        "{:<13}: {} {}",
                                        "IMAGE VERSION".bold(),
                                        built_ver.unwrap(),
                                        "↻".yellow()
                                    );
                                    println!("{}{}",
                                         " ".repeat(15),
                                         "New version available. Run `geoengine build` to update image.".italic().yellow()
                                );
                                }
                                Ordering::Less => {
                                    println!(
                                        "{:<13}: {} {}",
                                        "IMAGE VERSION".bold(),
                                        built_ver.unwrap(),
                                        "✗".red()
                                    );
                                    println!(
                                        "{}{}",
                                        " ".repeat(15),
                                        "Please ensure your versions are incremental."
                                            .italic()
                                            .red()
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            println!("{:<13}: {}", "IMAGE VERSION".bold(), built_ver.unwrap(),);
                            println!("{}{}", " ".repeat(15), e.italic().red());
                        }
                    }
                }
            }
            None => println!(
                "{}: {}",
                "VERSION".bold(),
                "No version specified. Please specify a version before the next build."
                    .italic()
                    .red()
            ),
        };
        // Compute column widths dynamically
        let name_w = desc
            .inputs
            .iter()
            .map(|t| t.name.len())
            .max()
            .unwrap_or(4)
            .max(4);
        let type_w = desc
            .inputs
            .iter()
            .map(|t| t.param_type.len())
            .max()
            .unwrap_or(4)
            .max(4);
        let req_w = 8; // "Required"
        let desc_w = desc
            .inputs
            .iter()
            .map(|t| t.description.as_deref().unwrap_or("").len())
            .max()
            .unwrap_or(11)
            .max(11);
        let def_w = desc
            .inputs
            .iter()
            .map(|t| {
                let default = t.default.clone().unwrap_or(serde_yaml::Value::from(""));
                yaml_value_to_display_string(&default).len()
            })
            .max()
            .unwrap_or(7)
            .max(7);
        let enum_w = desc
            .inputs
            .iter()
            .map(|t| {
                t.enum_values
                    .as_ref()
                    .map(|v| v.join(", ").len())
                    .unwrap_or(0)
            })
            .max()
            .unwrap_or(5)
            .max(5);
        // Print header
        let total_width = name_w + type_w + req_w + desc_w + def_w + enum_w + (5 * 3); // 5 separators " | "
        println!(
            "{:^total_width$}",
            "INPUTS".bold(),
            total_width = total_width
        );
        println!("{}", "=".repeat(total_width));
        println!(
            "{:<name_w$} | {:<type_w$} | {:<req_w$} | {:<desc_w$} | {:<def_w$} | {:<enum_w$}",
            "Name",
            "Type",
            "Required",
            "Description",
            "Default",
            "Enum",
            name_w = name_w,
            type_w = type_w,
            req_w = req_w,
            desc_w = desc_w,
            def_w = def_w,
            enum_w = enum_w
        );
        // Print separator
        println!(
            "{}-+-{}-+-{}-+-{}-+-{}-+-{}",
            "-".repeat(name_w),
            "-".repeat(type_w),
            "-".repeat(req_w),
            "-".repeat(desc_w),
            "-".repeat(def_w),
            "-".repeat(enum_w),
        );
        // Print rows
        for t in desc.inputs {
            let description = t.description.as_deref().unwrap_or("-");
            let default = t.default.clone().unwrap_or(serde_yaml::Value::from(""));
            let default_str = yaml_value_to_display_string(&default);
            let enum_str = t
                .enum_values
                .as_ref()
                .map(|v| v.join(", "))
                .unwrap_or_else(|| "-".to_string());

            println!(
                "{:<name_w$} | {:<type_w$} | {:<req_w$} | {:<desc_w$} | {:<def_w$} | {:<enum_w$}",
                t.name,
                t.param_type,
                if t.required { "Yes" } else { "No" },
                description,
                &default_str,
                enum_str,
                name_w = name_w,
                type_w = type_w,
                req_w = req_w,
                desc_w = desc_w,
                def_w = def_w,
                enum_w = enum_w
            );
        }
        println!();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// geoengine workers
// ---------------------------------------------------------------------------

pub async fn list_workers(json: bool, gis: Option<String>) -> Result<()> {
    let settings = Settings::load()?;
    let workers = settings.list_workers();
    let choice = match gis {
        Some(g) => match g.as_str() {
            "arcgis" => 1,
            "qgis" => 2,
            _ => {
                anyhow::bail!("Invalid --gis listed: '{}'", g);
            }
        },
        None => 0,
    };

    if json {
        let mut entries: Vec<WorkerListEntry> = Vec::new();
        for (name, path) in &workers {
            let (has_tool, description, is_registered) = match yaml_store::load_saved_config(name) {
                Ok(config) => {
                    let is_registered = match choice {
                        0 => None,
                        1 => Some(
                            config
                                .plugins
                                .as_ref()
                                .and_then(|p| p.arcgis)
                                .unwrap_or(false),
                        ),
                        2 => Some(
                            config
                                .plugins
                                .as_ref()
                                .and_then(|p| p.qgis)
                                .unwrap_or(false),
                        ),
                        _ => unreachable!(),
                    };
                    (config.command.is_some(), config.description, is_registered)
                }
                Err(_) => (false, None, if choice == 0 { None } else { Some(false) }),
            };
            match is_registered {
                Some(t) => {
                    if !t {
                        continue;
                    } else {
                    }
                }
                None => {}
            }
            entries.push(WorkerListEntry {
                name: name.to_string(),
                path: path.display().to_string(),
                has_tool,
                found: path.join("geoengine.yaml").exists(),
                description,
            });
        }
        println!("{}", serde_json::to_string(&entries)?);
        return Ok(());
    }

    if workers.is_empty() {
        println!("{}", "No workers registered".yellow());
        println!("\nRegister a worker with: {}", "geoengine apply".cyan());
        return Ok(());
    }

    // 3 extra for tick/cross icon + space + separator
    let name_w = workers
        .iter()
        .map(|(n, _)| n.len() + 3)
        .max()
        .unwrap_or(5)
        .max(7);
    let found_w = 5; // "FOUND"
    let path_w = workers
        .iter()
        .map(|(_, p)| p.display().to_string().len())
        .max()
        .unwrap_or(4);

    println!();
    println!(
        "{:<name_w$} {:<found_w$}   {:<path_w$}",
        "NAME".bold(),
        "FOUND".bold(),
        "PATH".bold(),
        name_w = name_w,
        found_w = found_w,
        path_w = path_w
    );
    println!("{}", "-".repeat(name_w + found_w + path_w + 4));

    for (name, path) in workers {
        let applied = if yaml_store::load_saved_config(name).is_ok() {
            "✓".green()
        } else {
            "✗".red()
        };
        let found = if path.join("geoengine.yaml").exists() {
            "✓".green()
        } else {
            "✗".red()
        };
        println!(
            "{} {:<name_w$} {}       {}",
            applied,
            name,
            found,
            path.display(),
            name_w = name_w - 2
        );
    }
    println!();

    Ok(())
}

// ---------------------------------------------------------------------------
// geoengine diff
// ---------------------------------------------------------------------------

/// Check which tracked files have changed since the last `apply` / `build`.
///
/// `target` controls the scope:
///   - `"all"`    – check geoengine.yaml, Dockerfile and command script
///   - `"yaml"`   – check geoengine.yaml only
///   - `"docker"` – check Dockerfile only
///   - `"command"`– check the command script file only
///
/// If `target` is `None` the default is `"all"`.
pub async fn diff_worker(target: Option<&str>) -> Result<()> {
    let target = target.unwrap_or("all");

    // Validate target value
    match target {
        "all" | "yaml" | "docker" | "command" => {}
        other => anyhow::bail!(
            "Invalid diff target '{}'. Expected one of: all, yaml, docker, command",
            other
        ),
    }

    // Resolve worker from cwd
    let (resolved_name, cwd) = resolve_worker_from_cwd();
    let config_path = cwd.join("geoengine.yaml");
    let config = WorkerConfig::load(&config_path)?;
    let worker_name = &resolved_name;

    let prev_state = state::load_state(worker_name)?;
    if prev_state.is_none() {
        anyhow::bail!(
            "No saved state for worker '{}'. Run 'geoengine apply' first.",
            worker_name
        );
    }
    let prev = prev_state.unwrap();

    // ── Collect results ────────────────────────────────────────────────
    struct DiffEntry {
        label: String,
        old_hash: String,
        new_hash: String,
        changed: bool,
    }

    let mut entries: Vec<DiffEntry> = Vec::new();

    // YAML
    if target == "all" || target == "yaml" {
        let yaml_path = cwd.join("geoengine.yaml");
        let old = prev.yaml_hash.clone().unwrap_or_default();
        let new = state::compute_file_hash(&yaml_path)?;
        entries.push(DiffEntry {
            label: "geoengine.yaml".to_string(),
            old_hash: old.clone(),
            new_hash: new.clone(),
            changed: old != new,
        });
    }

    // Dockerfile
    if target == "all" || target == "docker" {
        let df_path = cwd.join("Dockerfile");
        if df_path.exists() {
            let old = prev.dockerfile_hash.clone().unwrap_or_default();
            let new = state::compute_file_hash(&df_path)?;
            entries.push(DiffEntry {
                label: "Dockerfile".to_string(),
                old_hash: old.clone(),
                new_hash: new.clone(),
                changed: old != new,
            });
        } else {
            println!(
                "  {} {} (file not found, skipping)",
                "⚠".yellow(),
                "Dockerfile".cyan()
            );
        }
    }

    // Command script
    if target == "all" || target == "command" {
        if let Some(cmd) = &config.command {
            let script_path = cwd.join(&cmd.script);
            if script_path.exists() {
                let old = prev.command_hash.clone().unwrap_or_default();
                let new = state::compute_file_hash(&script_path)?;
                entries.push(DiffEntry {
                    label: format!("{} (command script)", cmd.script),
                    old_hash: old.clone(),
                    new_hash: new.clone(),
                    changed: old != new,
                });
            } else {
                println!(
                    "  {} {} '{}' (file not found, skipping)",
                    "⚠".yellow(),
                    "Command script".cyan(),
                    cmd.script
                );
            }
        } else if target == "command" {
            println!(
                "  {} No command section defined in geoengine.yaml",
                "⚠".yellow()
            );
        }
    }

    // ── Pretty output ──────────────────────────────────────────────────
    let any_changed = entries.iter().any(|e| e.changed);

    println!();
    println!(
        "{} Diff for worker '{}' (target: {})",
        "=>".blue().bold(),
        worker_name.cyan(),
        target.cyan()
    );
    println!("{}", "─".repeat(60));

    for entry in &entries {
        if entry.changed {
            println!(
                "  {} {}  {}",
                "✗".red().bold(),
                entry.label.cyan(),
                "changed".red()
            );
            println!("      old: {}", short_hash(&entry.old_hash).dimmed());
            println!("      new: {}", short_hash(&entry.new_hash).yellow());
            println!(
                "      {} {}{}",
                "Run 'geoengine".yellow().italic(),
                match entry.label.as_str() {
                    "geoengine.yaml" => "apply".yellow().italic().bold(),
                    _ => "build".yellow().italic().bold(),
                },
                "' to update.".yellow().italic()
            );
        } else {
            println!(
                "  {} {}  {}",
                "✓".green().bold(),
                entry.label.cyan(),
                "unchanged".green()
            );
        }
    }

    println!("{}", "─".repeat(60));
    if any_changed {
        println!("{} Changes detected", "!".yellow().bold());
    } else {
        println!("{} Everything is up to date.", "✓".green().bold());
    }
    println!();

    Ok(())
}

/// Return the first 12 hex chars of a hash (or the full string if shorter).
fn short_hash(h: &str) -> String {
    if h.is_empty() {
        "(none)".to_string()
    } else {
        h.chars().take(12).collect()
    }
}

// ---------------------------------------------------------------------------
// Utility functions
// ---------------------------------------------------------------------------

/// Resolve a worker from the current working directory by matching its path
/// against the registered workers in settings. Panics with a helpful message
/// if no matching worker is found.
fn resolve_worker_from_cwd() -> (String, PathBuf) {
    let cwd = std::env::current_dir().expect("Failed to determine current directory");
    let settings = Settings::load().expect("Failed to load settings");
    match settings.find_worker_by_path(&cwd) {
        Some((name, path)) => (name, path),
        None => {
            eprintln!(
                "{} Could not find a registered worker for the current directory:",
                "Error:".red().bold()
            );
            eprintln!("  {}", cwd.display());
            eprintln!();
            eprintln!("Please check the following:");
            eprintln!("  1. You are in the correct worker directory.");
            eprintln!(
                "  2. The worker has been registered (run {} to see registered workers).",
                "geoengine workers".cyan()
            );
            eprintln!(
                "  3. If you moved the worker directory, run {} to re-register it.",
                "geoengine apply".cyan()
            );
            std::process::exit(1);
        }
    }
}

/// Resolve a worker name to (name, path). If worker is None, use cwd path matching.
fn resolve_worker(worker: Option<&str>) -> Result<(String, PathBuf)> {
    if let Some(name) = worker {
        let settings = Settings::load()?;
        match settings.get_worker_path(name) {
            Ok(path) => Ok((name.to_string(), path)),
            Err(_) => {
                anyhow::bail!(
                    "Worker '{}' not found. Run 'geoengine apply' to register it.",
                    name
                )
            }
        }
    } else {
        let (name, path) = resolve_worker_from_cwd();
        Ok((name, path))
    }
}

/// Touch ~/.geoengine/.qgis_refresh so the QGIS plugin's file-system watcher
/// picks up the change and silently reloads its tool list.
fn touch_qgis_refresh_trigger() {
    if let Some(home) = dirs::home_dir() {
        let trigger = home.join(".geoengine").join(".qgis_refresh");
        // Write current timestamp so the file content actually changes
        let _ = std::fs::write(&trigger, chrono::Utc::now().to_rfc3339());
    }
}

fn set_plugin_flag_in_yaml(
    yaml_content: &mut String,
    plugin_key: &str,
    enabled: bool,
) -> Result<()> {
    let mut yaml_value: serde_yaml::Value = serde_yaml::from_str(yaml_content)
        .context("Failed to parse geoengine.yaml while updating plugin status")?;

    let root = yaml_value
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("Expected top-level mapping in geoengine.yaml"))?;
    let plugins_key = serde_yaml::Value::String("plugins".to_string());
    let plugin_entry_key = serde_yaml::Value::String(plugin_key.to_string());
    let plugins_value = root
        .entry(plugins_key)
        .or_insert_with(|| serde_yaml::Value::Mapping(serde_yaml::Mapping::new()));
    let plugins_map = plugins_value
        .as_mapping_mut()
        .ok_or_else(|| anyhow::anyhow!("Expected 'plugins' to be a mapping in geoengine.yaml"))?;
    plugins_map.insert(plugin_entry_key, serde_yaml::Value::Bool(enabled));

    *yaml_content = serde_yaml::to_string(&yaml_value)
        .context("Failed to serialize geoengine.yaml after updating plugin status")?;
    Ok(())
}

fn yaml_value_to_display_string(value: &serde_yaml::Value) -> String {
    match value {
        serde_yaml::Value::Null => String::new(),
        serde_yaml::Value::Bool(v) => v.to_string(),
        serde_yaml::Value::Number(v) => v.to_string(),
        serde_yaml::Value::String(v) => v.clone(),
        _ => serde_json::to_string(value).unwrap_or_else(|_| String::new()),
    }
}

/// Shell-escape a string for safe inclusion in a shell command
fn shell_escape(s: &str) -> String {
    if s.chars().any(|c| " \t\n\"'\\$`!*?[]{}();<>&|".contains(c)) {
        format!("'{}'", s.replace('\'', "'\\''"))
    } else {
        s.to_string()
    }
}

/// Verifies path-typed `geoengine.yaml` entries for a worker.
///
/// For now this checks:
/// - `command.script` exists and is a file
/// - each `local_dir_mounts[*].host_path` exists and is a directory
///
/// Note: relative paths are resolved against the provided `worker_path`.
/// If path resolution rules change, keep this function and its call sites aligned.
fn verify_worker_config_path_types(config: &WorkerConfig, worker_path: &Path) -> Result<()> {
    let mut errors = Vec::new();

    if let Some(command) = &config.command {
        let script_path = Path::new(&command.script);
        let script_path = if script_path.is_absolute() {
            script_path.to_path_buf()
        } else {
            worker_path.join(script_path)
        };

        if !script_path.exists() {
            errors.push(format!(
                "Worker '{}': command.script does not exist: {}",
                config.name,
                script_path.display()
            ));
        } else if !script_path.is_file() {
            errors.push(format!(
                "Worker '{}': command.script is not a file: {}",
                config.name,
                script_path.display()
            ));
        }
    }

    if let Some(mounts) = &config.local_dir_mounts {
        for (idx, mount) in mounts.iter().enumerate() {
            let host_path = Path::new(&mount.host_path);
            let host_path = if host_path.is_absolute() {
                host_path.to_path_buf()
            } else {
                worker_path.join(host_path)
            };

            if !host_path.exists() {
                errors.push(format!(
                    "Worker '{}': local_dir_mounts[{}].host_path does not exist: {}",
                    config.name,
                    idx,
                    host_path.display()
                ));
            } else if !host_path.is_dir() {
                errors.push(format!(
                    "Worker '{}': local_dir_mounts[{}].host_path is not a directory: {}",
                    config.name,
                    idx,
                    host_path.display()
                ));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        anyhow::bail!(
            "geoengine.yaml path validation failed:\n{}",
            errors.join("\n")
        )
    }
}
