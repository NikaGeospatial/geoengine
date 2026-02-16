use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::utils::paths;

/// Snapshot of a worker's state at last apply
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerState {
    pub worker_name: String,
    pub applied_at: String,
    /// SHA-256 of build-relevant config fields (name, version, command, local_dir_mounts)
    pub yaml_build_hash: String,
    /// SHA-256 of full geoengine.yaml contents (if present)
    pub yaml_hash: Option<String>,
    /// SHA-256 of Dockerfile contents (if present)
    pub dockerfile_hash: Option<String>,
    #[serde(default)]
    pub command_hash: Option<String>,
    /// Concatenated YAML build hash, dockerfile hash, and command hash for pushed build (if present)
    pub pushed_build_hash: Option<String>,
    pub image_tag: Option<String>,
    pub plugins_arcgis: Option<bool>,
    pub plugins_qgis: Option<bool>,
}

/// Load previously saved state for a worker
pub fn load_state(worker_name: &str) -> Result<Option<WorkerState>> {
    let state_dir = paths::get_state_dir()?;
    let state_file = state_dir.join(format!("{}.yaml", worker_name));

    if !state_file.exists() {
        return Ok(None);
    }

    let content = std::fs::read_to_string(&state_file)
        .with_context(|| format!("Failed to read state file: {}", state_file.display()))?;

    let state: WorkerState = serde_yaml::from_str(&content)
        .with_context(|| format!("Failed to parse state file: {}", state_file.display()))?;

    Ok(Some(state))
}

/// Save worker state after apply
pub fn save_state(state: &WorkerState) -> Result<()> {
    let state_dir = paths::get_state_dir()?;
    let state_file = state_dir.join(format!("{}.yaml", state.worker_name));

    let content = serde_yaml::to_string(state)?;
    std::fs::write(&state_file, content)
        .with_context(|| format!("Failed to write state file: {}", state_file.display()))?;

    Ok(())
}

/// Delete worker state file
pub fn delete_state(worker_name: &str) -> Result<()> {
    let state_dir = paths::get_state_dir()?;
    let state_file = state_dir.join(format!("{}.yaml", worker_name));

    if state_file.exists() {
        std::fs::remove_file(&state_file)
            .with_context(|| format!("Failed to delete state file: {}", state_file.display()))?;
    }

    Ok(())
}

/// Rename a worker's state file from old_name to new_name, updating
/// the `worker_name` field inside the state. Returns Ok(()) even if
/// no state file exists for old_name (nothing to migrate).
pub fn rename_state(old_name: &str, new_name: &str) -> Result<()> {
    let state_dir = paths::get_state_dir()?;
    let old_file = state_dir.join(format!("{}.yaml", old_name));
    let new_file = state_dir.join(format!("{}.yaml", new_name));

    if old_file.exists() {
        let content = std::fs::read_to_string(&old_file)
            .with_context(|| format!("Failed to read state file: {}", old_file.display()))?;
        let mut state: WorkerState = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse state file: {}", old_file.display()))?;
        state.worker_name = new_name.to_string();
        let new_content = serde_yaml::to_string(&state)?;
        std::fs::write(&new_file, new_content)
            .with_context(|| format!("Failed to write state file: {}", new_file.display()))?;
        std::fs::remove_file(&old_file)
            .with_context(|| format!("Failed to remove old state file: {}", old_file.display()))?;
    }
    Ok(())
}

/// Compute SHA-256 hash of a file's contents
pub fn compute_file_hash(path: &Path) -> Result<String> {
    let content = std::fs::read(path)
        .with_context(|| format!("Failed to read file for hashing: {}", path.display()))?;
    Ok(sha256_bytes(&content))
}

/// Compute SHA-256 hash of a byte slice
pub fn sha256_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Compute SHA-256 hash of a string
pub fn sha256_string(data: &str) -> String {
    sha256_bytes(data.as_bytes())
}
