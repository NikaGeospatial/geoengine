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
    /// RFC3339 timestamp of the last successful `geoengine build`
    #[serde(default)]
    pub built_at: Option<String>,
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
    /// Latest pushed image tag (non-dev)
    pub image_tag: Option<String>,
    /// Whether a local dev image (`geoengine-local-dev/<worker>:latest`) exists
    #[serde(default)]
    pub has_dev_image: bool,
    /// Whether a local pushed image (`geoengine-local/<worker>:<version>`) exists
    #[serde(default)]
    pub has_pushed_image: bool,
    /// Main script
    pub script: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::fs;
    use tempfile::TempDir;

    fn with_temp_home<F>(test: F) -> Result<()>
    where
        F: FnOnce() -> Result<()>,
    {
        let temp_dir = TempDir::new()?;
        let original_home = env::var("HOME").ok();

        env::set_var("HOME", temp_dir.path());

        let result = test();

        // Restore original HOME
        if let Some(home) = original_home {
            env::set_var("HOME", home);
        } else {
            env::remove_var("HOME");
        }

        result
    }

    #[test]
    fn test_sha256_bytes_deterministic() {
        let data = b"test data";
        let hash1 = sha256_bytes(data);
        let hash2 = sha256_bytes(data);
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA-256 produces 64 hex characters
    }

    #[test]
    fn test_sha256_string() {
        let data = "hello world";
        let hash = sha256_string(data);
        assert_eq!(hash.len(), 64);
        // Known SHA-256 hash of "hello world"
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_sha256_different_inputs() {
        let hash1 = sha256_string("test1");
        let hash2 = sha256_string("test2");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_save_and_load_state() -> Result<()> {
        with_temp_home(|| {
            let state = WorkerState {
                worker_name: "test-worker".to_string(),
                applied_at: "2024-01-01T00:00:00Z".to_string(),
                built_at: Some("2024-01-01T01:00:00Z".to_string()),
                yaml_build_hash: "abc123".to_string(),
                yaml_hash: Some("def456".to_string()),
                dockerfile_hash: Some("ghi789".to_string()),
                command_hash: Some("jkl012".to_string()),
                pushed_build_hash: Some("mno345".to_string()),
                image_tag: Some("1.0.0".to_string()),
                has_dev_image: true,
                has_pushed_image: false,
                script: Some("main.py".to_string()),
                plugins_arcgis: Some(false),
                plugins_qgis: Some(true),
            };

            save_state(&state)?;

            let loaded = load_state("test-worker")?;
            assert!(loaded.is_some());
            let loaded = loaded.unwrap();

            assert_eq!(loaded.worker_name, state.worker_name);
            assert_eq!(loaded.applied_at, state.applied_at);
            assert_eq!(loaded.built_at, state.built_at);
            assert_eq!(loaded.yaml_build_hash, state.yaml_build_hash);
            assert_eq!(loaded.image_tag, state.image_tag);
            assert_eq!(loaded.has_dev_image, state.has_dev_image);
            assert_eq!(loaded.has_pushed_image, state.has_pushed_image);

            Ok(())
        })
    }

    #[test]
    fn test_load_nonexistent_state() -> Result<()> {
        with_temp_home(|| {
            let result = load_state("nonexistent-worker")?;
            assert!(result.is_none());
            Ok(())
        })
    }

    #[test]
    fn test_delete_state() -> Result<()> {
        with_temp_home(|| {
            let state = WorkerState {
                worker_name: "delete-test".to_string(),
                applied_at: "2024-01-01T00:00:00Z".to_string(),
                built_at: None,
                yaml_build_hash: "hash".to_string(),
                yaml_hash: None,
                dockerfile_hash: None,
                command_hash: None,
                pushed_build_hash: None,
                image_tag: None,
                has_dev_image: false,
                has_pushed_image: false,
                script: None,
                plugins_arcgis: None,
                plugins_qgis: None,
            };

            save_state(&state)?;
            assert!(load_state("delete-test")?.is_some());

            delete_state("delete-test")?;
            assert!(load_state("delete-test")?.is_none());

            Ok(())
        })
    }

    #[test]
    fn test_delete_nonexistent_state() -> Result<()> {
        with_temp_home(|| {
            // Should not error when deleting nonexistent state
            delete_state("does-not-exist")?;
            Ok(())
        })
    }

    #[test]
    fn test_rename_state() -> Result<()> {
        with_temp_home(|| {
            let state = WorkerState {
                worker_name: "old-name".to_string(),
                applied_at: "2024-01-01T00:00:00Z".to_string(),
                built_at: None,
                yaml_build_hash: "hash".to_string(),
                yaml_hash: None,
                dockerfile_hash: None,
                command_hash: None,
                pushed_build_hash: None,
                image_tag: Some("1.0.0".to_string()),
                has_dev_image: true,
                has_pushed_image: false,
                script: Some("script.py".to_string()),
                plugins_arcgis: Some(true),
                plugins_qgis: Some(false),
            };

            save_state(&state)?;

            rename_state("old-name", "new-name")?;

            // Old state should not exist
            assert!(load_state("old-name")?.is_none());

            // New state should exist with updated name
            let loaded = load_state("new-name")?;
            assert!(loaded.is_some());
            let loaded = loaded.unwrap();
            assert_eq!(loaded.worker_name, "new-name");
            assert_eq!(loaded.image_tag, Some("1.0.0".to_string()));
            assert_eq!(loaded.script, Some("script.py".to_string()));

            Ok(())
        })
    }

    #[test]
    fn test_rename_nonexistent_state() -> Result<()> {
        with_temp_home(|| {
            // Should not error when renaming nonexistent state
            rename_state("does-not-exist", "new-name")?;
            Ok(())
        })
    }

    #[test]
    fn test_compute_file_hash() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let file_path = temp_dir.path().join("test.txt");

        fs::write(&file_path, "test content")?;

        let hash = compute_file_hash(&file_path)?;
        assert_eq!(hash.len(), 64);

        // Verify hash is deterministic
        let hash2 = compute_file_hash(&file_path)?;
        assert_eq!(hash, hash2);

        Ok(())
    }

    #[test]
    fn test_state_serialization_with_optional_fields() -> Result<()> {
        with_temp_home(|| {
            // Test with minimal required fields
            let minimal_state = WorkerState {
                worker_name: "minimal".to_string(),
                applied_at: "2024-01-01T00:00:00Z".to_string(),
                built_at: None,
                yaml_build_hash: "hash".to_string(),
                yaml_hash: None,
                dockerfile_hash: None,
                command_hash: None,
                pushed_build_hash: None,
                image_tag: None,
                has_dev_image: false,
                has_pushed_image: false,
                script: None,
                plugins_arcgis: None,
                plugins_qgis: None,
            };

            save_state(&minimal_state)?;
            let loaded = load_state("minimal")?.unwrap();

            assert_eq!(loaded.worker_name, minimal_state.worker_name);
            assert_eq!(loaded.built_at, None);
            assert_eq!(loaded.yaml_hash, None);
            assert_eq!(loaded.dockerfile_hash, None);

            Ok(())
        })
    }
}