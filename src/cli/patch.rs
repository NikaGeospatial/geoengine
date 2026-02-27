use anyhow::Result;
use colored::Colorize;
use std::path::PathBuf;

use crate::cli::plugins::{self, PluginPatchResult};
use crate::config::settings::Settings;
use crate::config::state;
use crate::config::worker::WorkerConfig;
use crate::config::yaml_store;
use crate::docker::dockerfile;
use crate::utils::paths;

/// Validate all GeoEngine artifacts and regenerate stale Dockerfiles and GIS plugins.
///
/// Global artifacts checked:
///   - ~/.geoengine/settings.yaml  (parse validation)
///   - ~/.geoengine/state/*.yaml   (parse + orphan check)
///   - ~/.geoengine/configs/*.json (parse + orphan check)
///
/// Per-worker checks (for every registered worker):
///   - Worker path existence
///   - geoengine.yaml schema validation (read-only)
///   - pixi.toml existence (read-only)
///   - Dockerfile content vs. current canonical template (overwritten if stale)
///   - .dockerignore content vs. current canonical template (overwritten if stale)
///
/// GIS plugin checks (skipped if the GIS application is not installed):
///   - QGIS plugin files vs. embedded canonical content (reinstalled if stale)
///   - ArcGIS plugin files vs. embedded canonical content (reinstalled if stale)
pub async fn patch_all() -> Result<()> {
    let mut issues: Vec<String> = Vec::new();
    let mut dockerfiles_updated: usize = 0;
    let mut plugins_updated: usize = 0;
    let mut workers_checked: usize = 0;

    // -----------------------------------------------------------------------
    // 1. Load global settings
    // -----------------------------------------------------------------------
    println!("{}", "Checking global artifacts...".bold());

    let settings = match Settings::load() {
        Ok(s) => {
            println!("  {} settings.yaml", "✓".green().bold());
            s
        }
        Err(e) => {
            let msg = format!("settings.yaml failed to parse: {}", e);
            println!("  {} {}", "✗".red().bold(), msg);
            issues.push(msg);
            // Cannot continue without settings — report and exit
            print_summary(workers_checked, dockerfiles_updated, 0, &issues);
            std::process::exit(1);
        }
    };

    // -----------------------------------------------------------------------
    // 2. Validate state files
    // -----------------------------------------------------------------------
    let state_dir = paths::get_state_dir()?;
    let state_entries: Vec<PathBuf> = std::fs::read_dir(&state_dir)?
        .filter_map(|e| e.ok().map(|d| d.path()))
        .filter(|p| p.extension().map_or(false, |ext| ext == "yaml"))
        .collect();

    if !state_entries.is_empty() {
        println!("\n{}", "Checking state files...".bold());
    }

    for path in &state_entries {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        match state::load_state(&stem) {
            Ok(_) => {
                let registered = settings.workers.contains_key(&stem);
                if registered {
                    println!("  {} state/{}.yaml", "✓".green().bold(), stem);
                } else {
                    let msg = format!(
                        "state/{}.yaml is orphaned (no registered worker named '{}')",
                        stem, stem
                    );
                    println!("  {} {}", "!".yellow().bold(), msg);
                    issues.push(msg);
                }
            }
            Err(e) => {
                let msg = format!("state/{}.yaml failed to parse: {}", stem, e);
                println!("  {} {}", "✗".red().bold(), msg);
                issues.push(msg);
            }
        }
    }

    // -----------------------------------------------------------------------
    // 3. Validate saved config files
    // -----------------------------------------------------------------------
    let configs_dir = paths::get_config_dir()?.join("configs");
    std::fs::create_dir_all(&configs_dir)?;
    let config_entries: Vec<PathBuf> = std::fs::read_dir(&configs_dir)?
        .filter_map(|e| e.ok().map(|d| d.path()))
        .filter(|p| p.extension().map_or(false, |ext| ext == "json"))
        .collect();

    if !config_entries.is_empty() {
        println!("\n{}", "Checking saved configs...".bold());
    }

    for path in &config_entries {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        match yaml_store::load_saved_config(&stem) {
            Ok(_) => {
                let registered = settings.workers.contains_key(&stem);
                if registered {
                    println!("  {} configs/{}.json", "✓".green().bold(), stem);
                } else {
                    let msg = format!(
                        "configs/{}.json is orphaned (no registered worker named '{}')",
                        stem, stem
                    );
                    println!("  {} {}", "!".yellow().bold(), msg);
                    issues.push(msg);
                }
            }
            Err(e) => {
                let msg = format!("configs/{}.json failed to parse: {}", stem, e);
                println!("  {} {}", "✗".red().bold(), msg);
                issues.push(msg);
            }
        }
    }

    // -----------------------------------------------------------------------
    // 4. Per-worker sweep
    // -----------------------------------------------------------------------
    if !settings.workers.is_empty() {
        println!("\n{}", "Checking registered workers...".bold());
    }

    let canonical_dockerfile = dockerfile::canonical_dockerfile_content();
    let canonical_dockerignore = dockerfile::canonical_dockerignore_content();

    let mut worker_names: Vec<&String> = settings.workers.keys().collect();
    worker_names.sort();

    for name in worker_names {
        let worker_path = &settings.workers[name];
        workers_checked += 1;

        println!("\n  {}", name.cyan().bold());

        // 4a. Path existence
        if !worker_path.exists() {
            let msg = format!(
                "worker '{}' path does not exist: {}",
                name,
                worker_path.display()
            );
            println!("    {} Path not found: {}", "✗".red().bold(), worker_path.display());
            issues.push(msg);
            continue;
        }
        println!("    {} Path: {}", "✓".green().bold(), worker_path.display());

        // 4b. geoengine.yaml schema validation
        let yaml_path = worker_path.join("geoengine.yaml");
        if !yaml_path.exists() {
            let msg = format!("worker '{}' is missing geoengine.yaml", name);
            println!("    {} geoengine.yaml missing", "✗".red().bold());
            issues.push(msg);
        } else {
            match WorkerConfig::load(&yaml_path) {
                Ok(_) => println!("    {} geoengine.yaml valid", "✓".green().bold()),
                Err(e) => {
                    let msg = format!("worker '{}' geoengine.yaml parse error: {}", name, e);
                    println!("    {} geoengine.yaml parse error: {}", "✗".red().bold(), e);
                    issues.push(msg);
                }
            }
        }

        // 4c. pixi.toml existence
        let pixi_path = worker_path.join("pixi.toml");
        if !pixi_path.exists() {
            let msg = format!("worker '{}' is missing pixi.toml", name);
            println!("    {} pixi.toml missing", "!".yellow().bold());
            issues.push(msg);
        } else {
            println!("    {} pixi.toml present", "✓".green().bold());
        }

        // 4d. Dockerfile patch
        let dockerfile_path = worker_path.join("Dockerfile");
        let dockerfile_needs_update = if dockerfile_path.exists() {
            match std::fs::read_to_string(&dockerfile_path) {
                Ok(content) => content != canonical_dockerfile,
                Err(_) => true,
            }
        } else {
            true
        };

        // 4e. .dockerignore patch
        let dockerignore_path = worker_path.join(".dockerignore");
        let dockerignore_needs_update = if dockerignore_path.exists() {
            match std::fs::read_to_string(&dockerignore_path) {
                Ok(content) => content != canonical_dockerignore,
                Err(_) => true,
            }
        } else {
            true
        };

        if dockerfile_needs_update || dockerignore_needs_update {
            match dockerfile::generate_dockerfile(worker_path) {
                Ok(_) => {
                    dockerfiles_updated += 1;
                    if dockerfile_needs_update && dockerignore_needs_update {
                        println!(
                            "    {} Dockerfile and .dockerignore regenerated",
                            "✓".green().bold()
                        );
                    } else if dockerfile_needs_update {
                        println!("    {} Dockerfile regenerated", "✓".green().bold());
                    } else {
                        println!("    {} .dockerignore regenerated", "✓".green().bold());
                    }
                }
                Err(e) => {
                    let msg = format!(
                        "worker '{}' Dockerfile regeneration failed: {}",
                        name, e
                    );
                    println!("    {} Dockerfile regeneration failed: {}", "✗".red().bold(), e);
                    issues.push(msg);
                }
            }
        } else {
            println!(
                "    {} Dockerfile and .dockerignore up-to-date",
                "•".cyan()
            );
        }
    }

    // -----------------------------------------------------------------------
    // 5. GIS plugin checks
    // -----------------------------------------------------------------------
    println!("\n{}", "Checking GIS plugins...".bold());

    // QGIS
    match plugins::patch_qgis().await {
        Ok(PluginPatchResult::NotInstalled) => {
            println!("  {} QGIS not installed on this machine — skipping", "•".cyan());
        }
        Ok(PluginPatchResult::UpToDate) => {
            println!("  {} QGIS plugin up-to-date", "✓".green().bold());
        }
        Ok(PluginPatchResult::Updated) => {
            plugins_updated += 1;
            println!("  {} QGIS plugin reinstalled (files were stale)", "✓".green().bold());
        }
        Ok(PluginPatchResult::Failed(e)) => {
            let msg = format!("QGIS plugin reinstall failed: {}", e);
            println!("  {} {}", "✗".red().bold(), msg);
            issues.push(msg);
        }
        Err(e) => {
            let msg = format!("QGIS plugin check error: {}", e);
            println!("  {} {}", "✗".red().bold(), msg);
            issues.push(msg);
        }
    }

    // ArcGIS
    match plugins::patch_arcgis().await {
        Ok(PluginPatchResult::NotInstalled) => {
            println!("  {} ArcGIS not installed on this machine — skipping", "•".cyan());
        }
        Ok(PluginPatchResult::UpToDate) => {
            println!("  {} ArcGIS plugin up-to-date", "✓".green().bold());
        }
        Ok(PluginPatchResult::Updated) => {
            plugins_updated += 1;
            println!(
                "  {} ArcGIS plugin reinstalled (files were stale). {}",
                "✓".green().bold(),
                "Please restart ArcGIS to reload the toolbox.".bold()
            );
        }
        Ok(PluginPatchResult::Failed(e)) => {
            let msg = format!("ArcGIS plugin reinstall failed: {}", e);
            println!("  {} {}", "✗".red().bold(), msg);
            issues.push(msg);
        }
        Err(e) => {
            let msg = format!("ArcGIS plugin check error: {}", e);
            println!("  {} {}", "✗".red().bold(), msg);
            issues.push(msg);
        }
    }

    // -----------------------------------------------------------------------
    // 6. Summary
    // -----------------------------------------------------------------------
    println!();
    print_summary(workers_checked, dockerfiles_updated, plugins_updated, &issues);

    if !issues.is_empty() {
        std::process::exit(1);
    }

    Ok(())
}

fn print_summary(
    workers_checked: usize,
    dockerfiles_updated: usize,
    plugins_updated: usize,
    issues: &[String],
) {
    let issue_count = issues.len();
    println!(
        "{} {} worker{} checked, {} Dockerfile{} updated, {} plugin{} updated, {} issue{} found.",
        "Patch complete:".bold(),
        workers_checked,
        if workers_checked == 1 { "" } else { "s" },
        dockerfiles_updated,
        if dockerfiles_updated == 1 { "" } else { "s" },
        plugins_updated,
        if plugins_updated == 1 { "" } else { "s" },
        issue_count,
        if issue_count == 1 { "" } else { "s" },
    );
}
