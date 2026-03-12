use anyhow::Result;
use colored::Colorize;
use semver::Version;
use std::collections::HashSet;
use std::path::PathBuf;

use crate::cli::plugins::{self, PluginPatchResult};
use crate::config::settings::Settings;
use crate::config::state;
use crate::config::worker::{RelevantWorkerConfig, VersionConfigMaps, WorkerConfig};
use crate::config::yaml_store;
use crate::docker::dockerfile;
use crate::utils::paths;

fn print_summary(
    workers_checked: usize,
    dockerfiles_updated: usize,
    plugins_updated: usize,
    skills_updated: usize,
    migrations_applied: usize,
    issues: &[String],
) {
    let issue_count = issues.len();
    println!(
        "{} {} worker{} checked, {} Dockerfile{} updated, {} plugin{} updated, {} skill{} synced, {} migration{} applied, {} issue{} found.",
        "Patch complete:".bold(),
        workers_checked,
        if workers_checked == 1 { "" } else { "s" },
        dockerfiles_updated,
        if dockerfiles_updated == 1 { "" } else { "s" },
        plugins_updated,
        if plugins_updated == 1 { "" } else { "s" },
        skills_updated,
        if skills_updated == 1 { "" } else { "s" },
        migrations_applied,
        if migrations_applied == 1 { "" } else { "s" },
        issue_count,
        if issue_count == 1 { "" } else { "s" },
    );
    if dockerfiles_updated > 0
        || plugins_updated > 0
        || skills_updated > 0
        || migrations_applied > 0
        || issue_count > 0
    {
        println!();
        println!("{}", "TO-DOs:".bold());
        if issue_count > 0 {
            println!("  {} Fix worker issues above.", "✗".red().bold())
        }
        if dockerfiles_updated > 0 {
            println!(
                "  {} Rebuild worker{} with updated Dockerfile{}.",
                "!".yellow().bold(),
                if dockerfiles_updated == 1 { "" } else { "s" },
                if dockerfiles_updated == 1 { "" } else { "s" },
            )
        }
        if plugins_updated > 0 {
            println!(
                "  {} Restart updated GIS platform{} to get latest plugin{}.",
                "!".yellow().bold(),
                if plugins_updated == 1 { "" } else { "s" },
                if plugins_updated == 1 { "" } else { "s" },
            )
        }
        if skills_updated > 0 {
            println!(
                "  {} Restart agents to get latest skill{}.",
                "!".yellow().bold(),
                if skills_updated == 1 { "" } else { "s" },
            )
        }
        if migrations_applied > 0 {
            println!(
                "  {} Versions tagged from current config snapshots — run 'geoengine build' to create fresh snapshots for the current configs.",
                "!".yellow().bold(),
            )
        }
    }
}

fn print_patch_warnings(warnings: &HashSet<String>) {
    if warnings.is_empty() {
        return;
    }
    println!();
    println!("{}", "Patch warnings:".bold());
    for warning in warnings {
        println!("  {} {}", "!".yellow().bold(), warning);
    }
}

use crate::docker::client::DockerClient;
use crate::utils::versioning::get_latest_worker_version;
use anyhow::anyhow;
use futures::future::BoxFuture;
use std::path::Path;

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
type V2SkillsCheckFn = for<'a> fn(&'a mut PatchV2Ctx) -> BoxFuture<'a, Result<()>>;

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
    warnings: HashSet<String>,
    dockerfiles_updated: usize,
    plugins_updated: usize,
    skills_updated: usize,
    migrations_applied: usize,
    workers_checked: usize,
    settings: Option<Settings>,
    canonical_dockerfile: String,
    canonical_dockerignore: String,
}

impl PatchV2Ctx {
    fn new() -> Self {
        Self {
            issues: Vec::new(),
            warnings: HashSet::new(),
            dockerfiles_updated: 0,
            plugins_updated: 0,
            skills_updated: 0,
            migrations_applied: 0,
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

    fn add_warning(&mut self, msg: impl Into<String>) {
        self.warnings.insert(msg.into());
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

/// Sequential running of skills stage.
async fn v2_run_skills_checks(ctx: &mut PatchV2Ctx) -> Result<()> {
    for check in V2_SKILLS_CHECKS {
        check(ctx).await?;
    }
    Ok(())
}

// =================================================================================================
//                                 STAGE-WISE FUNCTION SIGNATURE REGISTRY
// =================================================================================================

/// Check global GeoEngine-related settings.
const V2_GLOBAL_CHECKS: &[V2GlobalCheckFn] = &[v2_load_settings];

/// Check worker-specific states.
const V2_STATE_CHECKS: &[V2StateCheckFn] = &[
    v2_migrate_state_image_flags_from_docker,
    v2_validate_state_parse,
    v2_validate_state_orphan,
    v2_remove_state_image_tag_no_dev,
];

/// Check worker-specific configuration save files (saved geoengine.yaml).
const V2_CONFIG_CHECKS: &[V2ConfigCheckFn] = &[v2_validate_config_parse, v2_validate_config_orphan];

/// Check worker-specific artifacts.
const V2_WORKER_CHECKS: &[V2WorkerCheckFn] = &[
    v2_validate_worker_path,
    v2_validate_worker_yaml,
    v2_validate_worker_pixi,
    v2_patch_worker_docker_artifacts,
    v2_generate_worker_saves,
    v2_validate_worker_saves,
];

/// Check GIS plugin artifacts.
const V2_PLUGIN_CHECKS: &[V2PluginCheckFn] = &[v2_patch_qgis_stage, v2_patch_arcgis_stage];

/// Sync agent skills from the GeoEngine skills/ directory.
const V2_SKILLS_CHECKS: &[V2SkillsCheckFn] = &[v2_patch_claude_skills, v2_patch_codex_skills];
// Define these above functions below patch_all_v2()

/// v2 pipeline for `geoengine patch` implemented as staged function registries.
/// Register a function in the respective stage above, then define them below this function.
/// Runs all patches sequentially.
pub async fn patch_all_v2() -> Result<()> {
    let mut ctx = PatchV2Ctx::new();

    println!("{}", "Checking global artifacts...".bold());
    if matches!(
        v2_run_global_checks(&mut ctx).await?,
        V2PatchFlow::AbortPatch
    ) {
        return Ok(());
    }

    let state_dir = paths::get_state_dir()?;
    let state_entries: Vec<PathBuf> = std::fs::read_dir(&state_dir)?
        .filter_map(|e| e.ok().map(|d| d.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "yaml"))
        .collect();

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

    println!("\n{}", "Checking agent skills...".bold());
    v2_run_skills_checks(&mut ctx).await?;

    print_patch_warnings(&ctx.warnings);
    println!();
    print_summary(
        ctx.workers_checked,
        ctx.dockerfiles_updated,
        ctx.plugins_updated,
        ctx.skills_updated,
        ctx.migrations_applied,
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
                print_summary(
                    ctx.workers_checked,
                    ctx.dockerfiles_updated,
                    0,
                    0,
                    0,
                    &ctx.issues,
                );
                Ok(V2PatchFlow::AbortPatch)
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

/// # Migration from v0.4.5.
/// Derive `has_dev_image` and `has_pushed_image` from local Docker image tags.
fn v2_migrate_state_image_flags_from_docker<'a>(
    ctx: &'a mut PatchV2Ctx,
    stem: &'a str,
    worker_state: &'a mut state::WorkerState,
) -> BoxFuture<'a, Result<V2StateFlow>> {
    Box::pin(async move {
        let docker = match DockerClient::new().await {
            Ok(docker) => docker,
            Err(e) => {
                println!(
                    "  {} Could not connect to Docker while patching state/{}.yaml image flags: {}. Skipping.",
                    "!".yellow().bold(),
                    stem,
                    e
                );
                return Ok(V2StateFlow::Continue);
            }
        };

        let images = match docker.list_images(Some(stem), true).await {
            Ok(images) => images,
            Err(e) => {
                println!(
                    "  {} Failed to list Docker images while patching state/{}.yaml image flags: {}. Skipping.",
                    "!".yellow().bold(),
                    stem,
                    e
                );
                return Ok(V2StateFlow::Continue);
            }
        };

        let dev_tag = format!("geoengine-local-dev/{}:latest", stem);
        let pushed_prefix = format!("geoengine-local/{}:", stem);
        let has_dev_image = images
            .iter()
            .flat_map(|img| img.repo_tags.iter())
            .any(|tag| tag == &dev_tag);
        let has_pushed_image = images
            .iter()
            .flat_map(|img| img.repo_tags.iter())
            .any(|tag| tag.starts_with(&pushed_prefix));

        if worker_state.has_dev_image != has_dev_image
            || worker_state.has_pushed_image != has_pushed_image
        {
            worker_state.has_dev_image = has_dev_image;
            worker_state.has_pushed_image = has_pushed_image;
            state::save_state(worker_state)?;
            ctx.migrations_applied += 1;
            println!(
                "  {} Updated image flags in state/{}.yaml (dev: {}, pushed: {})",
                "✓".green().bold(),
                stem,
                has_dev_image,
                has_pushed_image
            );
        }

        Ok(V2StateFlow::Continue)
    })
}

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
    Box::pin(async move {
        let image_tag = worker_state.image_tag.as_ref();
        if image_tag.is_some_and(|t| t.as_str().contains("dev")) {
            let mut updated_image_tag = false;
            match DockerClient::new().await {
                Ok(docker) => {
                    let latest_built_ver =
                        get_latest_worker_version(worker_state.worker_name.as_ref(), &docker).await;

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
            println!(
                "    {} Path not found: {}",
                "✗".red().bold(),
                worker_path.display()
            );
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
                    println!(
                        "    {} Dockerfile regeneration failed: {}",
                        "✗".red().bold(),
                        e
                    );
                    ctx.issues.push(msg);
                }
            }
        } else {
            println!("    {} Dockerfile and .dockerignore up-to-date", "•".cyan());
        }

        Ok(V2WorkerFlow::Continue)
    })
}

/// # Migration from v0.4.5.
/// Ensures the saves directory and map.json exist for a worker, then tags all
/// found release Docker image versions to the current saved config snapshot.
fn v2_generate_worker_saves<'a>(
    ctx: &'a mut PatchV2Ctx,
    name: &'a str,
    _worker_path: &'a Path,
) -> BoxFuture<'a, Result<V2WorkerFlow>> {
    Box::pin(async move {
        let saves_dir = yaml_store::get_worker_saves_dir(name)?;
        let map_path = saves_dir.join("map.json");

        // Skip migration entirely once map.json already exists.
        if map_path.exists() {
            return Ok(V2WorkerFlow::Continue);
        }

        // Initialize saves dir + map.json, then tag existing release images once.
        std::fs::create_dir_all(&saves_dir)?;
        VersionConfigMaps {
            worker: name.to_string(),
            mappings: None,
        }
        .save_to_worker(name)?;
        println!("    {} Initialized saves directory", "✓".green().bold());

        // Connect to Docker to discover release images
        let docker = match DockerClient::new().await {
            Ok(d) => d,
            Err(e) => {
                println!(
                    "    {} Could not connect to Docker; skipping version tagging for '{}': {}",
                    "!".yellow().bold(),
                    name,
                    e
                );
                return Ok(V2WorkerFlow::Continue);
            }
        };

        // Collect all valid semver release versions from local Docker images
        let prefix = format!("geoengine-local/{}:", name);
        let images = docker
            .list_images(Some(&format!("geoengine-local/{}", name)), true)
            .await
            .unwrap_or_default();

        let mut versions: Vec<String> = images
            .iter()
            .flat_map(|img| img.repo_tags.iter())
            .filter(|tag| tag.starts_with(&prefix))
            .filter_map(|tag| tag.split(':').last().map(str::to_string))
            .filter(|v| Version::parse(v).is_ok())
            .collect();
        versions.sort_by(|a, b| Version::parse(a).unwrap().cmp(&Version::parse(b).unwrap()));
        versions.dedup();

        if versions.is_empty() {
            println!(
                "    {} No release images found; skipping version tagging.",
                "•".cyan()
            );
            return Ok(V2WorkerFlow::Continue);
        }

        // Tag every found version to the current saved config snapshot
        let count = versions.len();
        for version in &versions {
            yaml_store::cache_and_tag_config(name, version)?;
        }

        let version_list = versions.join(", ");
        println!(
            "    {} Tagged {} version{} to current config snapshot: {}",
            "✓".green().bold(),
            count,
            if count == 1 { "" } else { "s" },
            version_list.yellow()
        );

        ctx.add_warning(
            "Versions were tagged to the current saved config snapshot. \
             If config is mismatched to version, please delete that version's image.",
        );
        ctx.migrations_applied += 1;

        Ok(V2WorkerFlow::Continue)
    })
}

/// Validates the canonical structure of the saves directory for a worker.
/// Runs after v2_generate_worker_saves so the saves dir is guaranteed to exist.
fn v2_validate_worker_saves<'a>(
    ctx: &'a mut PatchV2Ctx,
    name: &'a str,
    _worker_path: &'a Path,
) -> BoxFuture<'a, Result<V2WorkerFlow>> {
    Box::pin(async move {
        let saves_dir = yaml_store::get_worker_saves_dir(name)?;
        let map_path = saves_dir.join("map.json");

        // Parse map.json
        let map = match VersionConfigMaps::load_from_worker(name) {
            Ok(m) => m,
            Err(e) => {
                let msg = format!("saves/{}/map.json failed to parse: {}", name, e);
                println!("    {} {}", "✗".red().bold(), msg);
                ctx.issues.push(msg);
                return Ok(V2WorkerFlow::Continue);
            }
        };

        // Validate worker field
        if map.worker != name {
            let msg = format!(
                "saves/{}/map.json worker field mismatch: expected '{}', got '{}'",
                name, name, map.worker
            );
            println!("    {} {}", "!".yellow().bold(), msg);
            ctx.issues.push(msg);
        }

        let mappings = map.mappings.unwrap_or_default();

        // Check every referenced snapshot exists and parses
        let mut referenced_hashes: HashSet<String> = HashSet::new();
        let mut saves_valid = true;
        for (version, hash) in &mappings {
            referenced_hashes.insert(hash.clone());
            let snapshot_path = saves_dir.join(format!("{}.json", hash));
            if !snapshot_path.exists() {
                let msg = format!(
                    "saves/{}/{}.json missing (referenced by version {})",
                    name, hash, version
                );
                println!("    {} {}", "✗".red().bold(), msg);
                ctx.issues.push(msg);
                saves_valid = false;
            } else if let Err(e) = RelevantWorkerConfig::load_json(&snapshot_path) {
                let msg = format!("saves/{}/{}.json failed to parse: {}", name, hash, e);
                println!("    {} {}", "✗".red().bold(), msg);
                ctx.issues.push(msg);
                saves_valid = false;
            }
        }

        // Check for unreferenced snapshot files
        if let Ok(entries) = std::fs::read_dir(&saves_dir) {
            for entry in entries.filter_map(|e| e.ok()) {
                let path = entry.path();
                if path == map_path {
                    continue;
                }
                if path.extension().is_some_and(|e| e == "json") {
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    if !referenced_hashes.contains(&stem) {
                        let msg = format!(
                            "saves/{}/{}.json is unreferenced (orphaned snapshot)",
                            name, stem
                        );
                        println!("    {} {}", "!".yellow().bold(), msg);
                        ctx.issues.push(msg);
                        saves_valid = false;
                    }
                }
            }
        }

        if saves_valid && map.worker == name {
            println!("    {} saves/{}/  valid", "✓".green().bold(), name);
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
                println!(
                    "  {} QGIS not installed on this machine — skipping",
                    "•".cyan()
                );
            }
            Ok(PluginPatchResult::UpToDate) => {
                println!("  {} QGIS plugin up-to-date", "✓".green().bold());
            }
            Ok(PluginPatchResult::Updated) => {
                ctx.plugins_updated += 1;
                println!(
                    "  {} QGIS plugin reinstalled (files were stale)",
                    "✓".green().bold()
                );
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
                println!(
                    "  {} ArcGIS not installed on this machine — skipping",
                    "•".cyan()
                );
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

// -------------------------------------------SKILLS STAGE------------------------------------------
// Input(s):
// - ctx (central config)
// -------------------------------------------------------------------------------------------------

/// An embedded GeoEngine skill (one entry per skill subdirectory).
///
/// `files` contains every file within the skill directory, collected recursively at compile
/// time.  Each tuple is `(relative_path, content)` where `relative_path` is relative to the
/// skill's own root (e.g. `"SKILL.md"` or `"assets/diagram.svg"`).
struct SkillEntry {
    /// The skill's subdirectory name (e.g. `"use-geoengine"`).
    skill: &'static str,
    /// All files belonging to this skill, embedded at compile time.
    files: &'static [(&'static str, &'static str)],
}

// All GeoEngine skills, auto-generated at compile time from the `skills/` directory.
// Adding, renaming, or removing a skill subdirectory or any file within one is picked up
// automatically — no changes to this file are required.
include!(concat!(env!("OUT_DIR"), "/skills_embedded.rs"));

/// Computes a deterministic hash over all files in a `SkillEntry`.
///
/// Files are processed in their stored order (which is alphabetical by relative path,
/// guaranteed by `build.rs`).  For each file the relative path and content are both
/// mixed in so that renames are detected even when content is unchanged.
fn skill_directory_hash(entry: &SkillEntry) -> String {
    use crate::config::state;
    // Concatenate "path\0content\0" for every file in sorted order.
    let mut combined = String::new();
    for (rel_path, content) in entry.files {
        combined.push_str(rel_path);
        combined.push('\0');
        combined.push_str(content);
        combined.push('\0');
    }
    state::sha256_string(&combined)
}

/// Computes the same hash for the on-disk version of a skill directory.
///
/// Walks `skill_dst` recursively and hashes every file's relative path and
/// content in sorted path order. Returns `None` if the directory is missing or
/// any entry is unreadable/non-UTF8/non-regular-file.
fn installed_skill_hash(skill_dst: &std::path::Path) -> Option<String> {
    use crate::config::state;
    if !skill_dst.is_dir() {
        return None;
    }

    let mut files: Vec<(String, String)> = Vec::new();
    let mut pending_dirs = vec![skill_dst.to_path_buf()];

    while let Some(dir) = pending_dirs.pop() {
        for dir_entry in std::fs::read_dir(&dir).ok()? {
            let dir_entry = dir_entry.ok()?;
            let path = dir_entry.path();
            let file_type = dir_entry.file_type().ok()?;
            if file_type.is_dir() {
                pending_dirs.push(path);
                continue;
            }
            if !file_type.is_file() {
                return None;
            }

            let rel_path = path.strip_prefix(skill_dst).ok()?;
            let rel_path = rel_path.to_str()?.replace('\\', "/");
            let content = std::fs::read_to_string(path).ok()?;
            files.push((rel_path, content));
        }
    }

    files.sort_by(|(a, _), (b, _)| a.cmp(b));

    let mut combined = String::new();
    for (rel_path, content) in files {
        combined.push_str(&rel_path);
        combined.push('\0');
        combined.push_str(content.as_str());
        combined.push('\0');
    }
    Some(state::sha256_string(&combined))
}

/// Syncs all GeoEngine skills (embedded at compile time) into an agent's skills directory.
///
/// For each `SkillEntry` in `SKILLS`:
///   - If absent in the agent dir → write all its files (preserving subdirectory structure).
///   - If present but the directory hash differs → overwrite all its files.
///   - If present and hash matches → skip.
///
/// Any skill subdirectory already in the agent dir is pruned only if it is known to be
/// GeoEngine-managed (via `.geoengine-managed-skills` or a `geoengine-` prefix) and is no
/// longer in `SKILLS`, so user/third-party skill folders are never deleted.
///
/// Returns the number of skills that were added, updated, or removed.
fn sync_skills_to_agent(agent_skills_dir: &std::path::Path) -> Result<usize> {
    use std::collections::HashSet;

    let mut changed = 0usize;
    let manifest_path = agent_skills_dir.join(".geoengine-managed-skills");
    let previously_managed: HashSet<String> = std::fs::read_to_string(&manifest_path)
        .ok()
        .map(|content| {
            content
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default();

    // --- 1. Upsert: write or overwrite skills that are new or stale ---
    let skill_names: Vec<&str> = SKILLS.iter().map(|s| s.skill).collect();

    for entry in SKILLS {
        let skill_dst = agent_skills_dir.join(entry.skill);

        let canonical_hash = skill_directory_hash(entry);
        let installed_hash = installed_skill_hash(&skill_dst);

        let needs_update = installed_hash.as_deref() != Some(canonical_hash.as_str());

        if needs_update {
            if let Ok(metadata) = std::fs::symlink_metadata(&skill_dst) {
                if metadata.is_dir() {
                    std::fs::remove_dir_all(&skill_dst)?;
                } else {
                    std::fs::remove_file(&skill_dst)?;
                }
            }

            // Write every file, creating intermediate subdirectories as needed.
            for (rel_path, content) in entry.files {
                let dst = skill_dst.join(rel_path);
                if let Some(parent) = dst.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&dst, content)?;
            }
            changed += 1;
        }
    }

    // --- 2. Prune: remove only GeoEngine-managed skill dirs no longer in the embedded set ---
    if agent_skills_dir.exists() {
        let installed_skill_dirs: Vec<PathBuf> = std::fs::read_dir(agent_skills_dir)?
            .filter_map(|e| e.ok().map(|d| d.path()))
            .filter(|p| p.is_dir())
            .collect();

        for installed in installed_skill_dirs {
            let dir_name = match installed.file_name().and_then(|n| n.to_str()) {
                Some(n) => n.to_string(),
                None => continue,
            };
            // Skip hidden directories (e.g. .system inside Codex skills).
            if dir_name.starts_with('.') {
                continue;
            }
            let is_geoengine_managed =
                previously_managed.contains(&dir_name) || dir_name.starts_with("geoengine-");
            if !is_geoengine_managed {
                continue;
            }
            if !skill_names.contains(&dir_name.as_str()) {
                std::fs::remove_dir_all(&installed)?;
                changed += 1;
            }
        }
    }

    // Persist currently managed skill names for safe future pruning.
    let manifest_content = if skill_names.is_empty() {
        String::new()
    } else {
        format!("{}\n", skill_names.join("\n"))
    };
    std::fs::write(manifest_path, manifest_content)?;

    Ok(changed)
}

/// Syncs GeoEngine skills into Claude's skills directory (`~/.claude/skills`).
/// Skipped entirely if `~/.claude` does not exist (Claude not installed).
fn v2_patch_claude_skills(ctx: &mut PatchV2Ctx) -> BoxFuture<'_, Result<()>> {
    Box::pin(async move {
        let home = match dirs::home_dir() {
            Some(h) => h,
            None => {
                println!(
                    "  {} Could not determine home directory — skipping Claude skills",
                    "•".cyan()
                );
                return Ok(());
            }
        };

        let claude_dir = home.join(".claude");
        if !claude_dir.exists() {
            println!(
                "  {} Claude not installed on this machine — skipping",
                "•".cyan()
            );
            return Ok(());
        }

        let agent_skills_dir = claude_dir.join("skills");
        std::fs::create_dir_all(&agent_skills_dir)?;

        match sync_skills_to_agent(&agent_skills_dir) {
            Ok(0) => {
                println!("  {} Claude skills up-to-date", "✓".green().bold());
            }
            Ok(n) => {
                ctx.skills_updated += n;
                println!(
                    "  {} Claude skills updated ({} skill{} synced)",
                    "✓".green().bold(),
                    n,
                    if n == 1 { "" } else { "s" },
                );
            }
            Err(e) => {
                let msg = format!("Claude skills sync failed: {}", e);
                println!("  {} {}", "✗".red().bold(), msg);
                ctx.issues.push(msg);
            }
        }

        Ok(())
    })
}

/// Syncs GeoEngine skills into Codex's skills directory (`~/.codex/skills`).
/// Skipped entirely if `~/.codex` does not exist (Codex not installed).
fn v2_patch_codex_skills(ctx: &mut PatchV2Ctx) -> BoxFuture<'_, Result<()>> {
    Box::pin(async move {
        let home = match dirs::home_dir() {
            Some(h) => h,
            None => {
                println!(
                    "  {} Could not determine home directory — skipping Codex skills",
                    "•".cyan()
                );
                return Ok(());
            }
        };

        let codex_dir = home.join(".codex");
        if !codex_dir.exists() {
            println!(
                "  {} Codex not installed on this machine — skipping",
                "•".cyan()
            );
            return Ok(());
        }

        let agent_skills_dir = codex_dir.join("skills");
        std::fs::create_dir_all(&agent_skills_dir)?;

        match sync_skills_to_agent(&agent_skills_dir) {
            Ok(0) => {
                println!("  {} Codex skills up-to-date", "✓".green().bold());
            }
            Ok(n) => {
                ctx.skills_updated += n;
                println!(
                    "  {} Codex skills updated ({} skill{} synced)",
                    "✓".green().bold(),
                    n,
                    if n == 1 { "" } else { "s" },
                );
            }
            Err(e) => {
                let msg = format!("Codex skills sync failed: {}", e);
                println!("  {} {}", "✗".red().bold(), msg);
                ctx.issues.push(msg);
            }
        }

        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_skill_directory_hash_deterministic() {
        let entry = SkillEntry {
            skill: "test-skill",
            files: &[
                ("file1.txt", "content1"),
                ("file2.txt", "content2"),
            ],
        };

        let hash1 = skill_directory_hash(&entry);
        let hash2 = skill_directory_hash(&entry);

        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA-256 hash length
    }

    #[test]
    fn test_skill_directory_hash_different_content() {
        let entry1 = SkillEntry {
            skill: "skill1",
            files: &[("file.txt", "content1")],
        };

        let entry2 = SkillEntry {
            skill: "skill2",
            files: &[("file.txt", "content2")],
        };

        let hash1 = skill_directory_hash(&entry1);
        let hash2 = skill_directory_hash(&entry2);

        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_skill_directory_hash_different_filenames() {
        let entry1 = SkillEntry {
            skill: "skill",
            files: &[("file1.txt", "content")],
        };

        let entry2 = SkillEntry {
            skill: "skill",
            files: &[("file2.txt", "content")],
        };

        let hash1 = skill_directory_hash(&entry1);
        let hash2 = skill_directory_hash(&entry2);

        // Hash should differ even if content is same but filename differs
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_skill_directory_hash_empty_files() {
        let entry = SkillEntry {
            skill: "empty-skill",
            files: &[],
        };

        let hash = skill_directory_hash(&entry);
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_installed_skill_hash_nonexistent() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let nonexistent = temp_dir.path().join("does-not-exist");

        let hash = installed_skill_hash(&nonexistent);
        assert!(hash.is_none());
    }

    #[test]
    fn test_installed_skill_hash_empty_dir() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let empty_dir = temp_dir.path().join("empty");
        std::fs::create_dir(&empty_dir).unwrap();

        let hash = installed_skill_hash(&empty_dir);
        assert!(hash.is_some());
        let hash = hash.unwrap();
        assert_eq!(hash.len(), 64);
    }

    #[test]
    fn test_installed_skill_hash_with_files() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let skill_dir = temp_dir.path().join("test-skill");
        std::fs::create_dir(&skill_dir).unwrap();

        std::fs::write(skill_dir.join("file1.txt"), "content1").unwrap();
        std::fs::write(skill_dir.join("file2.txt"), "content2").unwrap();

        let hash = installed_skill_hash(&skill_dir);
        assert!(hash.is_some());

        // Hash should be deterministic
        let hash2 = installed_skill_hash(&skill_dir);
        assert_eq!(hash, hash2);
    }

    #[test]
    fn test_patch_v2_ctx_new() {
        let ctx = PatchV2Ctx::new();

        assert_eq!(ctx.issues.len(), 0);
        assert_eq!(ctx.warnings.len(), 0);
        assert_eq!(ctx.dockerfiles_updated, 0);
        assert_eq!(ctx.plugins_updated, 0);
        assert_eq!(ctx.skills_updated, 0);
        assert_eq!(ctx.migrations_applied, 0);
        assert_eq!(ctx.workers_checked, 0);
        assert!(ctx.settings.is_none());
        assert!(!ctx.canonical_dockerfile.is_empty());
        assert!(!ctx.canonical_dockerignore.is_empty());
    }

    #[test]
    fn test_patch_v2_ctx_add_warning() {
        let mut ctx = PatchV2Ctx::new();

        ctx.add_warning("Warning 1");
        ctx.add_warning("Warning 2");
        ctx.add_warning("Warning 1"); // Duplicate

        assert_eq!(ctx.warnings.len(), 2); // HashSet deduplicates
        assert!(ctx.warnings.contains("Warning 1"));
        assert!(ctx.warnings.contains("Warning 2"));
    }

    #[test]
    fn test_patch_v2_ctx_settings_not_loaded() {
        let ctx = PatchV2Ctx::new();

        let result = ctx.settings();
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("settings were not loaded"));
    }

    #[test]
    fn test_print_summary_no_issues() {
        // This test just verifies the function doesn't panic
        print_summary(5, 2, 1, 0, 0, &[]);
    }

    #[test]
    fn test_print_summary_with_issues() {
        let issues = vec![
            "Issue 1".to_string(),
            "Issue 2".to_string(),
        ];
        print_summary(10, 3, 2, 1, 0, &issues);
    }

    #[test]
    fn test_print_patch_warnings_empty() {
        let warnings = HashSet::new();
        print_patch_warnings(&warnings);
    }

    #[test]
    fn test_print_patch_warnings_with_items() {
        let mut warnings = HashSet::new();
        warnings.insert("Warning 1".to_string());
        warnings.insert("Warning 2".to_string());
        print_patch_warnings(&warnings);
    }

    #[test]
    fn test_sync_skills_to_agent_creates_manifest() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let agent_dir = temp_dir.path().join("agent");
        std::fs::create_dir(&agent_dir).unwrap();

        // This will create the manifest file even if SKILLS is empty
        let result = sync_skills_to_agent(&agent_dir);
        assert!(result.is_ok());

        let manifest_path = agent_dir.join(".geoengine-managed-skills");
        assert!(manifest_path.exists());
    }
}