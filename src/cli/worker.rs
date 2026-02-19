use std::cmp::Ordering;
use anyhow::{Context, Result};
use colored::Colorize;
use dialoguer::{theme::ColorfulTheme, Select};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use crate::config::worker::WorkerConfig;
use crate::config::settings::Settings;
use crate::config::state::{self, sha256_bytes, WorkerState};
use crate::config::yaml_store;
use crate::docker::client::DockerClient;
use crate::docker::config::ContainerConfig;
use crate::docker::gpu::GpuConfig;
use crate::docker::dockerfile::get_dockerfile_config;
use crate::cli::plugins;
use crate::cli::plugins::{verify_arcgis_plugin_installed, verify_qgis_plugin_installed};
use crate::utils::versioning::{compare_versions, validate_version, get_latest_worker_version_clientless, get_latest_worker_version, compare_worker_version};
// ---------------------------------------------------------------------------
// JSON output structs (used by --json flags and plugin integration)
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct WorkerListEntry {
    name: String,
    path: String,
    has_tool: bool,
    found: bool,
    description: Option<String>
}

#[derive(Serialize, Deserialize)]
struct WorkerDescription {
    name: String,
    description: Option<String>,
    version: Option<String>,
    version_built: Option<String>,
    inputs: Vec<InputDescriptionJson>,
}

#[derive(Serialize, Deserialize)]
struct InputDescriptionJson {
    name: String,
    #[serde(rename = "param_type")]
    param_type: String,
    required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    default: Option<serde_yaml::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enum_values: Option<Vec<String>>,
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
}

// ---------------------------------------------------------------------------
// geoengine init
// ---------------------------------------------------------------------------

pub async fn init_worker(name: Option<&str>) -> Result<()> {
    let current_dir = std::env::current_dir()?;
    let config_path = current_dir.join("geoengine.yaml");

    if config_path.exists() {
        let replace = Select::with_theme(&ColorfulTheme::default())
            .with_prompt(format!("geoengine.yaml already exists in {}. Overwrite existing geoengine.yaml?", current_dir.display()))
            .items(&["Yes", "No"])
            .default(1)
            .interact()?;

        match replace {
            0 => (),
            _ => return Ok(())
        }
    }

    let worker_name = name
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            current_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("my-worker")
                .to_string()
        });

    let mut template = WorkerConfig::template(&worker_name);

    if let Err(e) = get_dockerfile_config(&current_dir, &mut template) {
        println!(
            "{} Dockerfile discovery skipped: {}",
            "!".yellow().bold(),
            e
        );
    }

    let yaml = serde_yaml::to_string(&template)?;

    std::fs::write(&config_path, yaml)?;

    println!(
        "{} Created {} in {}",
        "✓".green().bold(),
        "geoengine.yaml".cyan(),
        current_dir.display()
    );
    println!("\nNext steps:");
    println!("  1. Edit geoengine.yaml to configure your worker");
    println!("  2. Run {} to register and build", "geoengine apply".cyan());

    Ok(())
}

// ---------------------------------------------------------------------------
// geoengine build
// ---------------------------------------------------------------------------

pub async fn build_worker_local(no_cache: bool, dev: bool, build_args: &[String]) -> Result<()> {
    let (worker_name, _) = resolve_worker_from_cwd();
    build_worker(&worker_name, no_cache, dev, build_args).await
}

pub async fn build_worker(worker: &str, no_cache: bool, dev: bool, build_args: &[String]) -> Result<()> {
    let settings = Settings::load()?;
    let worker_path = settings.get_worker_path(worker)?;
    let config = yaml_store::load_saved_config(worker)?;

    let client = DockerClient::new().await?;

    let new_version = config.version.clone().unwrap_or("".to_string());

    // --- Version validation ---
    let ver_cmp = compare_worker_version(worker, &new_version, &client).await;
    let version_changed = match ver_cmp {
        Ok(c) => match c {
            Ordering::Less => {
                let latest_built = get_latest_worker_version(worker, &client).await.unwrap_or_default();
                if dev {
                    println!("{} Version is lower than latest built version: {} < {}", "!".yellow().bold(), new_version, latest_built);
                    println!("{} Correct it before your next push.", " ");
                } else {
                    anyhow::bail!("{}\n{}: {}\n{}: {}",
                        "New version cannot be lower than latest built version!".red().bold(),
                        "New version  ",
                        new_version,
                        "Built version",
                        latest_built
                    );
                }
                true
            },
            Ordering::Equal => false,
            Ordering::Greater => true
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
        true => prev_state.as_ref().and_then(|s| s.pushed_build_hash.clone()),
        false => Some(yaml_build_hash.clone() + dockerfile_hash.as_ref().unwrap_or(&"".to_string()) + command_hash.as_ref().unwrap_or(&"".to_string())),
    };
    let files_changed = match dev {
        true => match &prev_state {
            Some(prev) => {
                prev.yaml_build_hash != yaml_build_hash
                    || prev.dockerfile_hash != dockerfile_hash
                    || prev.command_hash != command_hash
            }
            None => true,
        }
        false => match &prev_state {
            Some(prev) => pushed_build_hash != prev.pushed_build_hash,
            None => true,
        },
    };

    if !no_cache {
        match (version_changed, files_changed) {
            (true, false) => {
                // Version bumped but no file changes — skip rebuild
                println!(
                    "{} {}o build-related files have been modified. Skipping rebuild.",
                    "!".yellow().bold(),
                    match dev {
                        true => "N".to_string(),
                        false => format!("Version changed to '{}', but n", new_version.cyan()),
                    },
                );
                return Ok(());
            },
            (false, true) => {
                // Files changed but version not bumped — ask user to increment
                if !dev {
                    anyhow::bail!(
                        "{}\n  Build-related files have changed, but the version ('{}') has not been incremented.\n  Please bump the version in geoengine.yaml before rebuilding.",
                        "Cannot rebuild without a version increment.".red().bold(),
                        new_version
                    );
                }
            },
            (false, false) => {
                // Nothing changed at all
                println!(
                    "{} No changes detected for worker '{}'. Nothing to build.",
                    "✓".green().bold(),
                    worker.cyan()
                );
                return Ok(());
            },
            (true, true) => {
                // Both changed — proceed with build
            }
        }
    }

    // --- Build ---
    println!(
        "{} Building worker '{}'...",
        "=>".blue().bold(),
        worker.cyan()
    );

    let context = worker_path.clone();
    let image_tag = format!("geoengine-local{}/{}:{}",
        match dev {
            true => "-dev",
            false => ""
        },
        config.name,
        match dev {
            true => "latest".to_string(),
            false => new_version
        })
    ;

    // Parse build args from CLI only
    let mut args: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for arg in build_args {
        let parts: Vec<&str> = arg.splitn(2, '=').collect();
        if parts.len() == 2 {
            args.insert(parts[0].to_string(), parts[1].to_string());
        }
    }

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")?,
    );
    pb.set_message("Building image...");
    pb.enable_steady_tick(std::time::Duration::from_millis(100));

    client
        .build_image(&dockerfile, &context, &image_tag, &args, no_cache)
        .await?;

    pb.finish_and_clear();
    println!(
        "{} Successfully built image: {}",
        "✓".green().bold(),
        image_tag.cyan()
    );

    // --- Update state with new hashes after successful build ---
    let prev_state = state::load_state(worker)?;
    let new_state = WorkerState {
        worker_name: worker.to_string(),
        applied_at: chrono::Utc::now().to_rfc3339(),
        yaml_build_hash,
        yaml_hash: prev_state.as_ref().and_then(|s| s.yaml_hash.clone()),
        dockerfile_hash,
        command_hash,
        pushed_build_hash,
        image_tag: Some(image_tag),
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
    // 1. Resolve the worker
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
                    anyhow::bail!("Worker '{}' not found and no geoengine.yaml at that path.", name);
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

    // 2. Load current config from YAML and detect changes from saved state. If changed, save it, if not exit.
    let config_changed = yaml_store::check_changed_config(&worker_name, &worker_path)?;
    if !config_changed {
        println!("{} No changes detected in geoengine.yaml of worker '{}'. Nothing to apply.", "!".yellow().bold(), worker_name);
        return Ok(());
    }
    let config = WorkerConfig::load(&worker_path.join("geoengine.yaml"))?;
    yaml_store::save_config(&config)?;

    // 3. Auto-register if not already registered
    let mut settings = Settings::load()?;
    if settings.workers.get(&worker_name).is_none() {
        let canonical = worker_path.canonicalize().unwrap_or_else(|_| worker_path.clone());
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
            "{} Worker '{}' is already registered",
            "✓".green().bold(),
            worker_name.cyan()
        );
    }

    // 4. Load previous state for plugin comparison
    let prev_state = state::load_state(&worker_name)?;

    // 5. Detect and apply plugin changes
    let cur_arcgis = config.plugins.as_ref().and_then(|p| p.arcgis).unwrap_or(false);
    let cur_qgis = config.plugins.as_ref().and_then(|p| p.qgis).unwrap_or(false);
    let prev_arcgis = prev_state.as_ref().and_then(|s| s.plugins_arcgis).unwrap_or(false);
    let prev_qgis = prev_state.as_ref().and_then(|s| s.plugins_qgis).unwrap_or(false);

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
                            plugin_change_msgs.push(format!("{} {} {}",
                                "•".yellow(),
                                "ArcGIS plugin installed.".to_string(),
                                "Please restart ArcGIS to see the plugin!".bold()
                            ));
                            plugin_change_msgs.push(format!("{} {}",
                                "✓".green(),
                                "Tool registered in ArcGIS plugin.".to_string()
                            ));
                        },
                        Err(e) => {
                            res_arcgis = false;
                            set_plugin_flag_in_yaml(&mut yaml_content, "arcgis", false)?;
                            yaml_dirty = true;
                            plugin_change_msgs.push(format!("{} {}",
                                "×".red(),
                                format!("ArcGIS plugin NOT installed: {}.", e).to_string()
                            ));
                            plugin_change_msgs.push(format!("{} {}",
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
                    plugin_change_msgs.push(format!("{} {}",
                        "×".red(),
                        "ArcGIS plugin installation rejected.".to_string()
                    ));
                    plugin_change_msgs.push(format!("{} {}",
                        "×".red(),
                        "Tool not registered. YAML reverted.".to_string()
                    ));
                },
            }
        } else if cur_arcgis && verify_arcgis_plugin_installed()? {
            plugin_change_msgs.push(format!("{} {}",
                "✓".green(),
                "Tool registered in ArcGIS plugin.".to_string()
            ));
        } else if !cur_qgis && verify_qgis_plugin_installed()? {
            plugin_change_msgs.push(format!("{} {}",
                "✓".red(),
                "Tool de-registered from ArcGIS plugin.".to_string()
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
                            plugin_change_msgs.push(format!("{} {} {}",
                                "•".yellow(),
                                "QGIS plugin installed.".to_string(),
                                "Please restart QGIS to see the plugin!".bold()
                            ));
                            plugin_change_msgs.push(format!("{} {}",
                                "✓".green(),
                                "Tool registered in QGIS plugin.".to_string()
                            ))
                        },
                        Err(e) => {
                            res_qgis = false;
                            set_plugin_flag_in_yaml(&mut yaml_content, "qgis", false)?;
                            yaml_dirty = true;
                            plugin_change_msgs.push(format!("{} {}",
                                "×".red(),
                                format!("QGIS plugin NOT installed: {}.", e).to_string()
                            ));
                            plugin_change_msgs.push(format!("{} {}",
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
                    plugin_change_msgs.push(format!("{} {}",
                        "×".red(),
                        "QGIS plugin NOT installed. Tool not registered.".to_string()
                    ));
                    plugin_change_msgs.push(format!("{} {}",
                        "×".red(),
                        "Tool not registered. YAML reverted.".to_string()
                    ));
                },
            }
        } else if cur_qgis && verify_qgis_plugin_installed()? {
            plugin_change_msgs.push(format!("{} {}",
                "✓".green(),
                "Tool registered in QGIS plugin.".to_string()
            ));
        } else if !cur_qgis && verify_qgis_plugin_installed()? {
            plugin_change_msgs.push(format!("{} {}",
                "✓".red(),
                "Tool de-registered from QGIS plugin.".to_string()
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

    // 5b. If plugin flags were reverted, persist to disk and update saved config
    let config = if yaml_dirty {
        std::fs::write(&yaml_path, &yaml_content)
            .with_context(|| format!("Failed to write reverted YAML to {}", yaml_path.display()))?;
        let updated_config = WorkerConfig::load(&yaml_path)?;
        yaml_store::save_config(&updated_config)?;
        updated_config
    } else {
        config
    };

    // 6. Recompute YAML hashes from current files; preserve build hashes from previous state.
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

    let new_state = WorkerState {
        worker_name: worker_name.clone(),
        applied_at: chrono::Utc::now().to_rfc3339(),
        yaml_build_hash,
        yaml_hash,
        dockerfile_hash,
        command_hash,
        pushed_build_hash,
        image_tag,
        plugins_arcgis: Some(res_arcgis),
        plugins_qgis: Some(res_qgis),
    };
    state::save_state(&new_state)?;

    // 7. Touch QGIS refresh trigger so the plugin auto-reloads tools
    touch_qgis_refresh_trigger();

    println!("{} Apply complete for worker '{}'", "✓".green().bold(), worker_name.cyan());
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
        println!(
            "  The directory may have been moved or deleted.",
        );
    }

    // Remove from settings
    settings.unregister_worker(&worker_name)?;
    settings.save()?;

    // Clean up state file and saved config
    state::delete_state(&worker_name)?;
    yaml_store::delete_saved_config(&worker_name)?;

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
    let this_ver = config.version.unwrap_or("latest".to_string());


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

    // Build extra mounts from input values that are explicitly defined as
    // file/folder inputs in worker config.
    let mut extra_mounts: Vec<(String, String, bool)> = Vec::new();
    let mut input_counter = 0usize;
    let input_definitions: HashMap<String, (String, bool)> = cmd_config
        .inputs
        .as_ref()
        .map(|defs| {
            defs.iter()
                .map(|d| {
                    (
                        d.name.clone(),
                        (d.param_type.to_ascii_lowercase(), d.readonly.unwrap_or(true)),
                    )
                })
                .collect()
        })
        .unwrap_or_default();

    // Build script arguments from inputs
    let mut script_args: Vec<String> = Vec::new();
    for (key, value) in &inputs {
        // Only auto-mount for declared file/folder inputs.
        let path = Path::new(value);
        let processed_value = if let Some((param_type, readonly)) = input_definitions.get(key) {
            match param_type.as_str() {
                "file" => {
                    if !path.exists() {
                        anyhow::bail!(
                            "Input '{}' is declared as type 'file' but path does not exist: {}",
                            key,
                            value
                        );
                    }
                    if !path.is_file() {
                        anyhow::bail!(
                            "Input '{}' is declared as type 'file' but received a non-file path: {}",
                            key,
                            value
                        );
                    }

                    let filename = path.file_name().ok_or_else(|| {
                        anyhow::anyhow!(
                            "Input '{}' is declared as type 'file' but has no file name: {}",
                            key,
                            value
                        )
                    })?;
                    let abs_path = path
                        .canonicalize()
                        .with_context(|| format!("Failed to resolve input file path: {}", value))?;
                    let container_path = format!("/inputs/{}/{}", key, filename.to_string_lossy());
                    extra_mounts.push((
                        abs_path.to_string_lossy().to_string(),
                        container_path.clone(),
                        *readonly,
                    ));
                    container_path
                }
                "folder" => {
                    if !path.exists() {
                        anyhow::bail!(
                            "Input '{}' is declared as type 'folder' but path does not exist: {}",
                            key,
                            value
                        );
                    }
                    if !path.is_dir() {
                        anyhow::bail!(
                            "Input '{}' is declared as type 'folder' but received a non-directory path: {}",
                            key,
                            value
                        );
                    }

                    let abs_path = path
                        .canonicalize()
                        .with_context(|| format!("Failed to resolve input directory path: {}", value))?;
                    let container_path = format!("/mnt/input_{}", input_counter);
                    input_counter += 1;
                    extra_mounts.push((
                        abs_path.to_string_lossy().to_string(),
                        container_path.clone(),
                        *readonly,
                    ));
                    container_path
                }
                _ => value.clone(),
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

    // Build full command
    let full_command = if script_args.is_empty() {
        format!("{} {}", cmd_config.program, cmd_config.script)
    } else {
        let escaped_args: Vec<String> = script_args.iter().map(|a| shell_escape(a)).collect();
        format!("{} {} {}", cmd_config.program, cmd_config.script, escaped_args.join(" "))
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
        env_vars: HashMap::new(),
        mounts,
        gpu_config,
        workdir: None,
        name: None,
        remove_on_exit: true,
        detach: false,
        tty: !json_output,
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
    let client = DockerClient::new().await?;
    let exit_code = if json_output {
        client.run_container_attached_to_stderr(&container_config).await?
    } else {
        client.run_container_attached(&container_config).await?
    };

    // Handle output
    if json_output {
        let result = RunResult {
            status: if exit_code == 0 { "completed".to_string() } else { "failed".to_string() },
            exit_code,
            error: if exit_code != 0 {
                Some(format!("Container exited with code {}", exit_code))
            } else {
                None
            },
            files: Vec::new(),
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
    let inputs = config.command.as_ref()
        .and_then(|c| c.inputs.as_ref())
        .map(|inputs| {
            inputs.iter().map(|i| InputDescriptionJson {
                name: i.name.clone(),
                param_type: i.param_type.clone(),
                required: i.required.unwrap_or(true),
                default: i.default.clone(),
                description: i.description.clone(),
                enum_values: i.enum_values.clone(),
            }).collect()
        })
        .unwrap_or_default();

    let version_built = get_latest_worker_version_clientless(&config.name).await;

    let desc = WorkerDescription {
        name: config.name.clone(),
        description: config.description.clone(),
        version: config.version.clone(),
        version_built,
        inputs,
    };

    if json {
        println!("{}", serde_json::to_string(&desc)?);
    }
    else {
        println!();
        println!("{:<13}: {}", "WORKER".bold(), desc.name);
        println!("{:<13}: {}", "DESCRIPTION".bold(), match desc.description {
            Some(d) => d.normal(),
            None => "No description provided.".italic(),
        });
        match desc.version {
            Some(v) => {
                println!("{:<13}: {} {}", "VERSION".bold(), v,
                         if validate_version(&v).is_ok() { "".normal() } else { "✗".red() });
                let built_ver = desc.version_built;
                if built_ver.is_none() {
                    match validate_version(&v) {
                        Ok(_) => {
                            println!("{:<13}: {} {}",
                                     "IMAGE VERSION".bold(),
                                     "not built",
                                     "↻".yellow()
                            );
                            println!("{}{}",
                                     " ".repeat(15),
                                     "Run `geoengine build` to build image.".italic().yellow()
                            );
                        },
                        Err(e) => {
                            println!("{:<13}: {}",
                                     "IMAGE VERSION".bold(),
                                     "not built"
                            );
                            println!("{}{}",
                                     " ".repeat(15),
                                     e.italic().red()
                            );
                        }
                    }
                }
                else {
                    let version_cmp = compare_versions(&config.version.clone().unwrap(), built_ver.clone().unwrap().as_ref());
                    match version_cmp {
                        Ok(order) => match order {
                            Ordering::Equal => {
                                println!("{:<13}: {} {}",
                                         "IMAGE VERSION".bold(),
                                         built_ver.unwrap(),
                                         "✓".green()
                                );
                            },
                            Ordering::Greater => {
                                println!("{:<13}: {} {}",
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
                                println!("{:<13}: {} {}",
                                         "IMAGE VERSION".bold(),
                                         built_ver.unwrap(),
                                         "✗".red()
                                );
                                println!("{}{}",
                                         " ".repeat(15),
                                         "Please ensure your versions are incremental.".italic().red()
                                );
                            }
                        },
                        Err(e) => {
                            println!("{:<13}: {}",
                                     "IMAGE VERSION".bold(),
                                     built_ver.unwrap(),
                            );
                            println!("{}{}",
                                     " ".repeat(15),
                                     e.italic().red()
                            );
                        }
                    }
                }
            },
            None => println!("{}: {}",
                "VERSION".bold(),
                "No version specified. Please specify a version before the next build.".italic().red()
            )
        };
        // Compute column widths dynamically
        let name_w = desc.inputs.iter().map(|t| t.name.len()).max().unwrap_or(4).max(4);
        let type_w = desc.inputs.iter().map(|t| t.param_type.len()).max().unwrap_or(4).max(4);
        let req_w = 8; // "Required"
        let desc_w = desc.inputs.iter()
            .map(|t| t.description.as_deref().unwrap_or("").len())
            .max()
            .unwrap_or(11)
            .max(11);
        let def_w = desc.inputs.iter()
            .map(|t| {
                let default = t.default.clone().unwrap_or(serde_yaml::Value::from(""));
                yaml_value_to_display_string(&default).len()
            })
            .max()
            .unwrap_or(7)
            .max(7);
        let enum_w = desc.inputs.iter()
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
        let total_width =
            name_w + type_w + req_w + desc_w + def_w + enum_w
                + (5 * 3); // 5 separators " | "
        println!("{:^total_width$}", "INPUTS".bold(), total_width = total_width);
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
        Some(g) => {
            match g.as_str() {
                "arcgis" => 1,
                "qgis" => 2,
                _ => {
                    anyhow::bail!("Invalid --gis listed: '{}'", g);
                },
            }
        }
        None => 0,
    };

    if json {
        let mut entries: Vec<WorkerListEntry> = Vec::new();
        for (name, path) in &workers {
            let (has_tool, description, is_registered) = match yaml_store::load_saved_config(name) {
                Ok(config) => {
                    let is_registered = match choice {
                        0 => None,
                        1 => Some(config.plugins.as_ref().and_then(|p| p.arcgis).unwrap_or(false)),
                        2 => Some(config.plugins.as_ref().and_then(|p| p.qgis).unwrap_or(false)),
                        _ => unreachable!(),
                    };
                    (config.command.is_some(), config.description, is_registered)
                },
                Err(_) => (false, None, if choice == 0 { None } else { Some(false) }),
            };
            match is_registered {
                Some(t) => {
                    if !t { continue } else {}
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
        println!(
            "\nRegister a worker with: {}",
            "geoengine apply".cyan()
        );
        return Ok(());
    }

    // 3 extra for tick/cross icon + space + separator
    let name_w = workers.iter().map(|(n, _)| n.len() + 3).max().unwrap_or(5).max(7);
    let found_w = 5; // "FOUND"
    let path_w = workers.iter().map(|(_, p)| p.display().to_string().len()).max().unwrap_or(4);

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
        println!("{} {:<name_w$} {}       {}", applied, name, found, path.display(), name_w = name_w - 2);
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
            println!(
                "      old: {}",
                short_hash(&entry.old_hash).dimmed()
            );
            println!(
                "      new: {}",
                short_hash(&entry.new_hash).yellow()
            );
            println!(
                "      {} {}{}",
                "Run 'geoengine".yellow().italic(),
                match entry.label.as_str() {
                    "geoengine.yaml" => "apply".yellow().italic().bold(),
                    _ => "build".yellow().italic().bold()
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
        println!(
            "{} Changes detected",
            "!".yellow().bold()
        );
    } else {
        println!(
            "{} Everything is up to date.",
            "✓".green().bold()
        );
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
            eprintln!("{} Could not find a registered worker for the current directory:", "Error:".red().bold());
            eprintln!("  {}", cwd.display());
            eprintln!();
            eprintln!("Please check the following:");
            eprintln!("  1. You are in the correct worker directory.");
            eprintln!("  2. The worker has been registered (run {} to see registered workers).", "geoengine workers".cyan());
            eprintln!("  3. If you moved the worker directory, run {} to re-register it.", "geoengine apply".cyan());
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
                anyhow::bail!("Worker '{}' not found. Run 'geoengine apply' to register it.", name)
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

fn set_plugin_flag_in_yaml(yaml_content: &mut String, plugin_key: &str, enabled: bool) -> Result<()> {
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
