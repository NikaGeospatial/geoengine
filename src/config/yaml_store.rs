use anyhow::{Context, Result};
use std::path::PathBuf;
use crate::config::state;
use crate::config::worker::WorkerConfig;
use crate::utils::paths;

/// Get the directory for saved worker configs (~/.geoengine/configs)
fn get_configs_dir() -> Result<PathBuf> {
    let configs_dir = paths::get_config_dir()?.join("configs");
    std::fs::create_dir_all(&configs_dir)?;
    Ok(configs_dir)
}

/// Get the path to a saved worker config JSON file
fn config_path(worker_name: &str) -> Result<PathBuf> {
    Ok(get_configs_dir()?.join(format!("{}.json", worker_name)))
}

/// Save a WorkerConfig as JSON after `geoengine apply`.
pub fn save_config(config: &WorkerConfig) -> Result<()> {
    let path = config_path(&config.name)?;
    let json = serde_json::to_string_pretty(config)
        .context("Failed to serialize worker config to JSON")?;
    std::fs::write(&path, json)
        .with_context(|| format!("Failed to write saved config: {}", path.display()))?;
    Ok(())
}

/// Load the saved (applied) WorkerConfig for a worker.
///
/// All commands that consume worker configuration should call this instead of
/// reading the raw YAML directly.  If no saved config exists the user must run
/// `geoengine apply` first.
pub fn load_saved_config(worker_name: &str) -> Result<WorkerConfig> {
    let path = config_path(worker_name)?;
    if !path.exists() {
        anyhow::bail!(
            "No applied configuration found for worker '{}'.\n\
             Run 'geoengine apply' to save the configuration first.",
            worker_name
        );
    }
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read saved config: {}", path.display()))?;
    let config: WorkerConfig = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse saved config: {}", path.display()))?;
    Ok(config)
}

/// Delete the saved config for a worker (used during `geoengine delete`).
pub fn delete_saved_config(worker_name: &str) -> Result<()> {
    let path = config_path(worker_name)?;
    if path.exists() {
        std::fs::remove_file(&path)
            .with_context(|| format!("Failed to delete saved config: {}", path.display()))?;
    }
    Ok(())
}

/// Rename a saved config file from old_name to new_name.
/// Returns Ok(()) even if no saved config exists for old_name (nothing to migrate).
pub fn rename_saved_config(old_name: &str, new_name: &str) -> Result<()> {
    let old_path = config_path(old_name)?;
    let new_path = config_path(new_name)?;

    if old_path.exists() {
        // Load, update the name field, and save under the new name
        let content = std::fs::read_to_string(&old_path)
            .with_context(|| format!("Failed to read saved config: {}", old_path.display()))?;
        let mut config: WorkerConfig = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse saved config: {}", old_path.display()))?;
        config.name = new_name.to_string();
        let json = serde_json::to_string_pretty(&config)
            .context("Failed to serialize worker config to JSON")?;
        std::fs::write(&new_path, json)
            .with_context(|| format!("Failed to write saved config: {}", new_path.display()))?;
        std::fs::remove_file(&old_path)
            .with_context(|| format!("Failed to remove old saved config: {}", old_path.display()))?;
    }
    Ok(())
}

/// Compare saved config for a worker with new config, check if it changed and return true if it did.
pub fn check_changed_config(worker_name: &str, worker_path: &PathBuf) -> Result<bool> {
    let worker_state = state::load_state(worker_name)?;
    match worker_state {
        Some(s) => {
            let old_hash = s.yaml_hash.unwrap_or("".to_string());
            let new_hash = state::compute_file_hash(&worker_path.join("geoengine.yaml"))?;
            Ok(old_hash != new_hash)
        },
        None => Ok(true)
    }
}