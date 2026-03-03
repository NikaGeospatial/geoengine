use anyhow::Result;
use colored::Colorize;
use std::path::PathBuf;

use crate::cli::plugins::{self, PluginPatchResult};
use crate::config::settings::Settings;
use crate::config::state;
use crate::config::worker::WorkerConfig;
use crate::config::yaml_store;
use crate::docker::{dockerfile};
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

use anyhow::anyhow;
use futures::future::BoxFuture;
use std::path::Path;
use crate::docker::client::DockerClient;
use crate::utils::versioning::get_latest_worker_version;

// =================================================================================================
//                                      STAGE FUNCTION DEFINITIONS
// =================================================================================================
type V2GlobalCheckFn = for<'a> fn(&'a mut PatchV2Ctx) -> BoxFuture<'a, Result<V2PatchFlow>>;
type V2StateCheckFn = for<'a> fn(
    &'a mut PatchV2Ctx,
    &'a str,
    &'a mut state::WorkerState,
) -> BoxFuture<'a, Result<V2StateFlow>>;
type V2ConfigCheckFn = for<'a> fn(
    &'a mut PatchV2Ctx,
    &'a str,
    &'a mut WorkerConfig,
) -> BoxFuture<'a, Result<V2ConfigFlow>>;
type V2WorkerCheckFn =
    for<'a> fn(&'a mut PatchV2Ctx, &'a str, &'a Path) -> BoxFuture<'a, Result<V2WorkerFlow>>;
type V2PluginCheckFn = for<'a> fn(&'a mut PatchV2Ctx) -> BoxFuture<'a, Result<()>>;

// =================================================================================================
//                                       STAGE FLOW DEFINITIONS
// =================================================================================================
enum V2PatchFlow {
    Continue,
    AbortPatch,
}

enum V2StateFlow {
    Continue,
    SkipRemainingChecks,
}

enum V2ConfigFlow {
    Continue,
    SkipRemainingChecks,
}

enum V2WorkerFlow {
    Continue,
    SkipWorker,
}

/// Main central context for the v2 patch pipeline.
struct PatchV2Ctx {
    issues: Vec<String>,
    dockerfiles_updated: usize,
    plugins_updated: usize,
    workers_checked: usize,
    settings: Option<Settings>,
    canonical_dockerfile: String,
    canonical_dockerignore: String,
}

impl PatchV2Ctx {
    fn new() -> Self {
        Self {
            issues: Vec::new(),
            dockerfiles_updated: 0,
            plugins_updated: 0,
            workers_checked: 0,
            settings: None,
            canonical_dockerfile: dockerfile::canonical_dockerfile_content(),
            canonical_dockerignore: dockerfile::canonical_dockerignore_content(),
        }
    }

    fn settings(&self) -> Result<&Settings> {
        self.settings
            .as_ref()
            .ok_or_else(|| anyhow!("settings were not loaded before dependent checks"))
    }
}

// =================================================================================================
//                                   STAGE-WISE RUNNER COLLECTIONS
// =================================================================================================

/// Sequential running of global stage.
async fn v2_run_global_checks(ctx: &mut PatchV2Ctx) -> Result<V2PatchFlow> {
    for check in V2_GLOBAL_CHECKS {
        match check(ctx).await? {
            V2PatchFlow::Continue => {}
            V2PatchFlow::AbortPatch => return Ok(V2PatchFlow::AbortPatch),
        }
    }
    Ok(V2PatchFlow::Continue)
}

/// Sequential running of state stage.
async fn v2_run_state_checks(ctx: &mut PatchV2Ctx, stem: &str) -> Result<()> {
    let mut worker_state = match state::load_state(stem) {
        Ok(s) => match s {
            Some(s) => s,
            None => {
                let msg = format!("Failed to find state/{}.yaml", stem);
                println!("  {} {}", "✗".red().bold(), msg);
                ctx.issues.push(msg);
                return Ok(());
            }
        },
        Err(e) => {
            let msg = format!("state/{}.yaml failed to parse: {}", stem, e);
            println!("  {} {}", "✗".red().bold(), msg);
            ctx.issues.push(msg);
            return Ok(());
        }
    };

    for check in V2_STATE_CHECKS {
        match check(ctx, stem, &mut worker_state).await? {
            V2StateFlow::Continue => {}
            V2StateFlow::SkipRemainingChecks => break,
        }
    }

    Ok(())
}

/// Sequential running of config stage.
async fn v2_run_config_checks(ctx: &mut PatchV2Ctx, stem: &str) -> Result<()> {
    let mut worker_config = match yaml_store::load_saved_config(stem) {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("configs/{}.json failed to parse: {}", stem, e);
            println!("  {} {}", "✗".red().bold(), msg);
            ctx.issues.push(msg);
            return Ok(());
        }
    };

    for check in V2_CONFIG_CHECKS {
        match check(ctx, stem, &mut worker_config).await? {
            V2ConfigFlow::Continue => {}
            V2ConfigFlow::SkipRemainingChecks => break,
        }
    }

    Ok(())
}

/// Sequential running of worker stage.
async fn v2_run_worker_checks(
    ctx: &mut PatchV2Ctx,
    name: &str,
    worker_path: &Path,
) -> Result<V2WorkerFlow> {
    for check in V2_WORKER_CHECKS {
        match check(ctx, name, worker_path).await? {
            V2WorkerFlow::Continue => {}
            V2WorkerFlow::SkipWorker => return Ok(V2WorkerFlow::SkipWorker),
        }
    }
    Ok(V2WorkerFlow::Continue)
}

/// Sequential running of plugin stage.
async fn v2_run_plugin_checks(ctx: &mut PatchV2Ctx) -> Result<()> {
    for check in V2_PLUGIN_CHECKS {
        check(ctx).await?;
    }
    Ok(())
}

// =================================================================================================
//                                 STAGE-WISE FUNCTION SIGNATURE REGISTRY
// =================================================================================================

/// Check global GeoEngine-related settings.
const V2_GLOBAL_CHECKS: &[V2GlobalCheckFn] = &[
    v2_load_settings,
];

/// Check worker-specific states.
const V2_STATE_CHECKS: &[V2StateCheckFn] = &[
    v2_validate_state_parse,
    v2_validate_state_orphan,
    v2_remove_state_image_tag_no_dev,
];

/// Check worker-specific configuration save files (saved geoengine.yaml).
const V2_CONFIG_CHECKS: &[V2ConfigCheckFn] = &[
    v2_validate_config_parse,
    v2_validate_config_orphan,
];

/// Check worker-specific artifacts.
const V2_WORKER_CHECKS: &[V2WorkerCheckFn] = &[
    v2_validate_worker_path,
    v2_validate_worker_yaml,
    v2_validate_worker_pixi,
    v2_patch_worker_docker_artifacts,
];

/// Check GIS plugin artifacts.
const V2_PLUGIN_CHECKS: &[V2PluginCheckFn] = &[v2_patch_qgis_stage, v2_patch_arcgis_stage];
// Define these above functions below patch_all_v2()


/// v2 pipeline for `geoengine patch` implemented as staged function registries.
/// Register a function in the respective stage above, then define them below this function.
/// Runs all patches sequentially.
pub async fn patch_all_v2() -> Result<()> {
    let mut ctx = PatchV2Ctx::new();

    println!("{}", "Checking global artifacts...".bold());
    if matches!(v2_run_global_checks(&mut ctx).await?, V2PatchFlow::AbortPatch) {
        return Ok(());
    }

    let state_dir = paths::get_state_dir()?;
    let state_entries: Vec<PathBuf> = std::fs::read_dir(&state_dir)?
        .filter_map(|e| e.ok().map(|d| d.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "yaml"))
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
        v2_run_state_checks(&mut ctx, &stem).await?;
    }

    let configs_dir = paths::get_config_dir()?.join("configs");
    std::fs::create_dir_all(&configs_dir)?;
    let config_entries: Vec<PathBuf> = std::fs::read_dir(&configs_dir)?
        .filter_map(|e| e.ok().map(|d| d.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "json"))
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
        v2_run_config_checks(&mut ctx, &stem).await?;
    }

    let mut worker_records: Vec<(String, PathBuf)> = ctx
        .settings()?
        .workers
        .iter()
        .map(|(name, path)| (name.clone(), path.clone()))
        .collect();
    worker_records.sort_by(|a, b| a.0.cmp(&b.0));

    if !worker_records.is_empty() {
        println!("\n{}", "Checking registered workers...".bold());
    }

    for (name, worker_path) in worker_records {
        ctx.workers_checked += 1;
        println!("\n  {}", name.cyan().bold());

        if matches!(
            v2_run_worker_checks(&mut ctx, &name, &worker_path).await?,
            V2WorkerFlow::SkipWorker
        ) {
            continue;
        }
    }

    println!("\n{}", "Checking GIS plugins...".bold());
    v2_run_plugin_checks(&mut ctx).await?;

    println!();
    print_summary(
        ctx.workers_checked,
        ctx.dockerfiles_updated,
        ctx.plugins_updated,
        &ctx.issues,
    );

    if !ctx.issues.is_empty() {
        std::process::exit(1);
    }

    Ok(())
}

// =================================================================================================
//                                      FUNCTION DEFINITION REGISTRY
// =================================================================================================

// -------------------------------------------GLOBAL STAGE------------------------------------------
// Inputs:
// - ctx (central config)
// -------------------------------------------------------------------------------------------------

/// Load settings from `settings.yaml`.
fn v2_load_settings(ctx: &'_ mut PatchV2Ctx) -> BoxFuture<'_, Result<V2PatchFlow>> {
    Box::pin(async move {
        match Settings::load() {
            Ok(settings) => {
                println!("  {} settings.yaml", "✓".green().bold());
                ctx.settings = Some(settings);
                Ok(V2PatchFlow::Continue)
            }
            Err(e) => {
                let msg = format!("settings.yaml failed to parse: {}", e);
                println!("  {} {}", "✗".red().bold(), msg);
                ctx.issues.push(msg);
                print_summary(ctx.workers_checked, ctx.dockerfiles_updated, 0, &ctx.issues);
                std::process::exit(1);
            }
        }
    })
}

// -------------------------------------------STATE STAGE-------------------------------------------
// Inputs:
// - ctx (central config)
// - stem (stem of worker state file)
// - worker_state (worker state)
// -------------------------------------------------------------------------------------------------

/// Validates if a saved worker state is a valid YAML.
fn v2_validate_state_parse<'a>(
    _ctx: &'a mut PatchV2Ctx,
    stem: &'a str,
    _worker_state: &'a mut state::WorkerState,
) -> BoxFuture<'a, Result<V2StateFlow>> {
    Box::pin(async move {
        println!("  {} state/{}.yaml", "✓".green().bold(), stem);
        Ok(V2StateFlow::Continue)
    })
}

/// Validates if a saved worker state is orphaned, i.e., the worker is not registered.
fn v2_validate_state_orphan<'a>(
    ctx: &'a mut PatchV2Ctx,
    stem: &'a str,
    _worker_state: &'a mut state::WorkerState,
) -> BoxFuture<'a, Result<V2StateFlow>> {
    Box::pin(async move {
        let registered = ctx.settings()?.workers.contains_key(stem);
        if !registered {
            let msg = format!(
                "state/{}.yaml is orphaned (no registered worker named '{}')",
                stem, stem
            );
            println!("  {} {}", "!".yellow().bold(), msg);
            ctx.issues.push(msg);
        }
        Ok(V2StateFlow::Continue)
    })
}

/// # Migration from v0.4.2.
/// Image tag in a worker's state should not show dev images.
fn v2_remove_state_image_tag_no_dev<'a>(
    _ctx: &'a mut PatchV2Ctx,
    stem: &'a str,
    worker_state: &'a mut state::WorkerState,
) -> BoxFuture<'a, Result<V2StateFlow>> {
    Box::pin(async move{
        let image_tag = worker_state.image_tag.as_ref();
        if image_tag.is_some_and(|t| t.as_str().contains("dev")) {
            let mut updated_image_tag = false;
            match DockerClient::new().await {
                Ok(docker) => {
                    let latest_built_ver = get_latest_worker_version(
                        worker_state.worker_name.as_ref(),
                        &docker,
                    )
                    .await;

                    if let Some(version) = latest_built_ver {
                        worker_state.image_tag = Some(version);
                        updated_image_tag = true;
                    } else {
                        let msg = format!(
                            "Could not determine latest built image for '{}' while patching state/{}.yaml; keeping existing image tag.",
                            worker_state.worker_name, stem
                        );
                        println!("  {} {}", "!".yellow().bold(), msg);
                    }
                }
                Err(e) => {
                    let msg = format!(
                        "Could not connect to Docker while patching state/{}.yaml: {}. Keeping existing image tag.",
                        stem, e
                    );
                    println!("  {} {}", "!".yellow().bold(), msg);
                }
            }

            state::save_state(worker_state)?;

            if updated_image_tag {
                let msg = format!(
                    "Made image tag not reflect dev images from state/{}.yaml",
                    stem
                );
                println!("  {} {}", "!".yellow().bold(), msg);
            }
        }
        Ok(V2StateFlow::Continue)
    })
}

// ------------------------------------------CONFIG STAGE-------------------------------------------
// Inputs:
// - ctx (central config)
// - stem (stem of config file)
// - worker_config (worker config)
// -------------------------------------------------------------------------------------------------

/// Validates if a saved worker configuration is a valid JSON.
fn v2_validate_config_parse<'a>(
    _ctx: &'a mut PatchV2Ctx,
    stem: &'a str,
    _worker_config: &'a mut WorkerConfig,
) -> BoxFuture<'a, Result<V2ConfigFlow>> {
    Box::pin(async move {
        println!("  {} configs/{}.json", "✓".green().bold(), stem);
        Ok(V2ConfigFlow::Continue)
    })
}

/// Validates if a saved worker configuration is orphaned, i.e., the worker is not registered.
fn v2_validate_config_orphan<'a>(
    ctx: &'a mut PatchV2Ctx,
    stem: &'a str,
    _worker_config: &'a mut WorkerConfig,
) -> BoxFuture<'a, Result<V2ConfigFlow>> {
    Box::pin(async move {
        let registered = ctx.settings()?.workers.contains_key(stem);
        if !registered {
            let msg = format!(
                "configs/{}.json is orphaned (no registered worker named '{}')",
                stem, stem
            );
            println!("  {} {}", "!".yellow().bold(), msg);
            ctx.issues.push(msg);
        }
        Ok(V2ConfigFlow::Continue)
    })
}

// -------------------------------------------WORKER STAGE------------------------------------------
// Input(s):
// - ctx (central config)
// - name (name of worker)
// - worker_path (path to worker)
// -------------------------------------------------------------------------------------------------

/// Validates if a worker's path still exists.
fn v2_validate_worker_path<'a>(
    ctx: &'a mut PatchV2Ctx,
    name: &'a str,
    worker_path: &'a Path,
) -> BoxFuture<'a, Result<V2WorkerFlow>> {
    Box::pin(async move {
        if !worker_path.exists() {
            let msg = format!(
                "worker '{}' path does not exist: {}",
                name,
                worker_path.display()
            );
            println!("    {} Path not found: {}", "✗".red().bold(), worker_path.display());
            ctx.issues.push(msg);
            return Ok(V2WorkerFlow::SkipWorker);
        }

        println!("    {} Path: {}", "✓".green().bold(), worker_path.display());
        Ok(V2WorkerFlow::Continue)
    })
}

/// Validates a worker's configuration file against canonical config.
fn v2_validate_worker_yaml<'a>(
    ctx: &'a mut PatchV2Ctx,
    name: &'a str,
    worker_path: &'a Path,
) -> BoxFuture<'a, Result<V2WorkerFlow>> {
    Box::pin(async move {
        let yaml_path = worker_path.join("geoengine.yaml");
        if !yaml_path.exists() {
            let msg = format!("worker '{}' is missing geoengine.yaml", name);
            println!("    {} geoengine.yaml missing", "✗".red().bold());
            ctx.issues.push(msg);
            return Ok(V2WorkerFlow::Continue);
        }

        match WorkerConfig::load(&yaml_path) {
            Ok(_) => println!("    {} geoengine.yaml valid", "✓".green().bold()),
            Err(e) => {
                let msg = format!("worker '{}' geoengine.yaml parse error: {}", name, e);
                println!("    {} geoengine.yaml parse error: {}", "✗".red().bold(), e);
                ctx.issues.push(msg);
            }
        }

        Ok(V2WorkerFlow::Continue)
    })
}

/// Validates worker's dependencies file against canonical file.
fn v2_validate_worker_pixi<'a>(
    ctx: &'a mut PatchV2Ctx,
    name: &'a str,
    worker_path: &'a Path,
) -> BoxFuture<'a, Result<V2WorkerFlow>> {
    Box::pin(async move {
        let pixi_path = worker_path.join("pixi.toml");
        if !pixi_path.exists() {
            let msg = format!("worker '{}' is missing pixi.toml", name);
            println!("    {} pixi.toml missing", "!".yellow().bold());
            ctx.issues.push(msg);
        } else {
            println!("    {} pixi.toml present", "✓".green().bold());
        }

        Ok(V2WorkerFlow::Continue)
    })
}

/// Validates worker's Dockerfile and .dockerignore against canonical files.
fn v2_patch_worker_docker_artifacts<'a>(
    ctx: &'a mut PatchV2Ctx,
    name: &'a str,
    worker_path: &'a Path,
) -> BoxFuture<'a, Result<V2WorkerFlow>> {
    Box::pin(async move {
        let dockerfile_path = worker_path.join("Dockerfile");
        let dockerfile_needs_update = if dockerfile_path.exists() {
            match std::fs::read_to_string(&dockerfile_path) {
                Ok(content) => content != ctx.canonical_dockerfile,
                Err(_) => true,
            }
        } else {
            true
        };

        let dockerignore_path = worker_path.join(".dockerignore");
        let dockerignore_needs_update = if dockerignore_path.exists() {
            match std::fs::read_to_string(&dockerignore_path) {
                Ok(content) => content != ctx.canonical_dockerignore,
                Err(_) => true,
            }
        } else {
            true
        };

        if dockerfile_needs_update || dockerignore_needs_update {
            match dockerfile::generate_dockerfile(&worker_path.to_path_buf()) {
                Ok(_) => {
                    ctx.dockerfiles_updated += 1;
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
                    let msg = format!("worker '{}' Dockerfile regeneration failed: {}", name, e);
                    println!("    {} Dockerfile regeneration failed: {}", "✗".red().bold(), e);
                    ctx.issues.push(msg);
                }
            }
        } else {
            println!(
                "    {} Dockerfile and .dockerignore up-to-date",
                "•".cyan()
            );
        }

        Ok(V2WorkerFlow::Continue)
    })
}

// -------------------------------------------PLUGIN STAGE------------------------------------------
// Input(s):
// - ctx (central config)
// -------------------------------------------------------------------------------------------------

/// Updates QGIS plugin if the installation exists. Ignores if not.
fn v2_patch_qgis_stage(ctx: &mut PatchV2Ctx) -> BoxFuture<'_, Result<()>> {
    Box::pin(async move {
        match plugins::patch_qgis().await {
            Ok(PluginPatchResult::NotInstalled) => {
                println!("  {} QGIS not installed on this machine — skipping", "•".cyan());
            }
            Ok(PluginPatchResult::UpToDate) => {
                println!("  {} QGIS plugin up-to-date", "✓".green().bold());
            }
            Ok(PluginPatchResult::Updated) => {
                ctx.plugins_updated += 1;
                println!("  {} QGIS plugin reinstalled (files were stale)", "✓".green().bold());
            }
            Ok(PluginPatchResult::Failed(e)) => {
                let msg = format!("QGIS plugin reinstall failed: {}", e);
                println!("  {} {}", "✗".red().bold(), msg);
                ctx.issues.push(msg);
            }
            Err(e) => {
                let msg = format!("QGIS plugin check error: {}", e);
                println!("  {} {}", "✗".red().bold(), msg);
                ctx.issues.push(msg);
            }
        }

        Ok(())
    })
}

/// Updates ArcGIS Pro plugin if the installation exists. Ignores if not.
fn v2_patch_arcgis_stage(ctx: &mut PatchV2Ctx) -> BoxFuture<'_, Result<()>> {
    Box::pin(async move {
        match plugins::patch_arcgis().await {
            Ok(PluginPatchResult::NotInstalled) => {
                println!("  {} ArcGIS not installed on this machine — skipping", "•".cyan());
            }
            Ok(PluginPatchResult::UpToDate) => {
                println!("  {} ArcGIS plugin up-to-date", "✓".green().bold());
            }
            Ok(PluginPatchResult::Updated) => {
                ctx.plugins_updated += 1;
                println!(
                    "  {} ArcGIS plugin reinstalled (files were stale). {}",
                    "✓".green().bold(),
                    "Please restart ArcGIS to reload the toolbox.".bold()
                );
            }
            Ok(PluginPatchResult::Failed(e)) => {
                let msg = format!("ArcGIS plugin reinstall failed: {}", e);
                println!("  {} {}", "✗".red().bold(), msg);
                ctx.issues.push(msg);
            }
            Err(e) => {
                let msg = format!("ArcGIS plugin check error: {}", e);
                println!("  {} {}", "✗".red().bold(), msg);
                ctx.issues.push(msg);
            }
        }

        Ok(())
    })
}
