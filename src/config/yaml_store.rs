use crate::config::state;
use crate::config::worker::{VersionConfigMaps, WorkerConfig};
use crate::utils::paths;
use anyhow::{Context, Result};
use std::path::PathBuf;

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
    if old_name == new_name {
        return Ok(());
    }

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
        std::fs::remove_file(&old_path).with_context(|| {
            format!("Failed to remove old saved config: {}", old_path.display())
        })?;
    }
    Ok(())
}

/// Rename a saved saves directory from old_name to new_name.
/// Returns Ok(()) even if no saves directory exists for old_name (nothing to migrate).
pub fn rename_saves_dir(old_name: &str, new_name: &str) -> Result<()> {
    if old_name == new_name {
        return Ok(());
    }

    let old_path = get_worker_saves_dir(old_name)?;
    let new_path = get_worker_saves_dir(new_name)?;

    if old_path.exists() {
        std::fs::rename(&old_path, &new_path).with_context(|| {
            format!(
                "Failed to rename saves directory from '{}' to '{}'",
                old_path.display(),
                new_path.display()
            )
        })?;
    }
    Ok(())
}

/// Delete the saves directory for a worker (used during `geoengine delete`).
pub fn delete_saves_dir(worker_name: &str) -> Result<()> {
    let saves_path = get_worker_saves_dir(worker_name)?;
    if saves_path.exists() {
        std::fs::remove_dir_all(&saves_path).with_context(|| {
            format!("Failed to delete saves directory: {}", saves_path.display())
        })?;
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
        }
        None => Ok(true),
    }
}

/// Get a worker's saves cache directory
pub fn get_worker_saves_dir(worker_name: &str) -> Result<PathBuf> {
    Ok(paths::get_saves_dir()?.join(worker_name))
}

/// Cache the current config and tag it to a version.
/// Uses `config_content_hash` (excludes name/version) as the dedup key.
pub fn cache_and_tag_config(worker_name: &str, version: &str) -> Result<()> {
    let saves_path = get_worker_saves_dir(worker_name)?;

    // Ensure saves directory exists
    std::fs::create_dir_all(&saves_path)
        .with_context(|| format!("Failed to create saves directory: {}", saves_path.display()))?;

    let current_config = load_saved_config(worker_name)?;
    let content_hash = current_config.config_content_hash();

    // Only write the snapshot file if no file with this hash exists (dedup)
    let snapshot_file = saves_path.join(format!("{}.json", content_hash));
    if !snapshot_file.exists() {
        let relevant_fields = current_config.get_relevant_fields();
        let content = serde_json::to_string_pretty(&relevant_fields)
            .context("Failed to serialize relevant fields to JSON")?;
        std::fs::write(&snapshot_file, content)
            .context("Failed to write config snapshot to cache")?;
    }

    // Load or create map.json, then add the version mapping
    let mut saves_map = match VersionConfigMaps::load_from_worker(worker_name) {
        Ok(map) => map,
        Err(_) => VersionConfigMaps {
            worker: worker_name.to_string(),
            mappings: None,
        },
    };
    let mut mappings = saves_map.mappings.unwrap_or_default();
    mappings.insert(version.to_string(), content_hash);
    saves_map.mappings = Some(mappings);
    saves_map.save_to_worker(worker_name)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::worker::{CommandConfig, InputParameter, MountConfig, PluginsConfig, WorkerConfig};
    use std::collections::HashMap;
    use std::env;
    use tempfile::TempDir;

    fn with_temp_home<F>(test: F) -> Result<()>
    where
        F: FnOnce() -> Result<()>,
    {
        let temp_dir = TempDir::new()?;
        let original_home = env::var("HOME").ok();

        env::set_var("HOME", temp_dir.path());

        let result = test();

        if let Some(home) = original_home {
            env::set_var("HOME", home);
        } else {
            env::remove_var("HOME");
        }

        result
    }

    fn create_test_config(name: &str, version: &str) -> WorkerConfig {
        WorkerConfig {
            name: name.to_string(),
            version: version.to_string(),
            description: Some("Test worker".to_string()),
            command: Some(CommandConfig {
                program: "python".to_string(),
                script: "main.py".to_string(),
                inputs: Some(vec![InputParameter {
                    name: "input-file".to_string(),
                    param_type: "file".to_string(),
                    required: Some(true),
                    default: None,
                    description: Some("Input file".to_string()),
                    enum_values: None,
                    readonly: Some(true),
                    filetypes: Some(vec![".tif".to_string()]),
                }]),
            }),
            local_dir_mounts: Some(vec![MountConfig {
                host_path: "./data".to_string(),
                container_path: "/data".to_string(),
                readonly: Some(false),
            }]),
            plugins: Some(PluginsConfig {
                arcgis: Some(false),
                qgis: Some(true),
            }),
            deploy: None,
        }
    }

    #[test]
    fn test_save_and_load_config() -> Result<()> {
        with_temp_home(|| {
            let config = create_test_config("test-worker", "1.0.0");

            save_config(&config)?;

            let loaded = load_saved_config("test-worker")?;

            assert_eq!(loaded.name, config.name);
            assert_eq!(loaded.version, config.version);
            assert_eq!(loaded.description, config.description);
            assert!(loaded.command.is_some());
            assert!(loaded.local_dir_mounts.is_some());

            Ok(())
        })
    }

    #[test]
    fn test_load_nonexistent_config_fails() -> Result<()> {
        with_temp_home(|| {
            let result = load_saved_config("nonexistent-worker");
            assert!(result.is_err());
            assert!(result.unwrap_err().to_string().contains("No applied configuration found"));

            Ok(())
        })
    }

    #[test]
    fn test_delete_saved_config() -> Result<()> {
        with_temp_home(|| {
            let config = create_test_config("delete-test", "1.0.0");

            save_config(&config)?;
            assert!(load_saved_config("delete-test").is_ok());

            delete_saved_config("delete-test")?;

            let result = load_saved_config("delete-test");
            assert!(result.is_err());

            Ok(())
        })
    }

    #[test]
    fn test_delete_nonexistent_config() -> Result<()> {
        with_temp_home(|| {
            // Should not error
            delete_saved_config("does-not-exist")?;
            Ok(())
        })
    }

    #[test]
    fn test_rename_saved_config() -> Result<()> {
        with_temp_home(|| {
            let config = create_test_config("old-name", "1.0.0");

            save_config(&config)?;

            rename_saved_config("old-name", "new-name")?;

            // Old name should not exist
            let old_result = load_saved_config("old-name");
            assert!(old_result.is_err());

            // New name should exist with updated name field
            let new_config = load_saved_config("new-name")?;
            assert_eq!(new_config.name, "new-name");
            assert_eq!(new_config.version, "1.0.0");
            assert_eq!(new_config.description, config.description);

            Ok(())
        })
    }

    #[test]
    fn test_rename_nonexistent_config() -> Result<()> {
        with_temp_home(|| {
            // Should not error
            rename_saved_config("does-not-exist", "new-name")?;
            Ok(())
        })
    }

    #[test]
    fn test_rename_same_name() -> Result<()> {
        with_temp_home(|| {
            let config = create_test_config("same-name", "1.0.0");

            save_config(&config)?;

            // Should not error when renaming to same name
            rename_saved_config("same-name", "same-name")?;

            let loaded = load_saved_config("same-name")?;
            assert_eq!(loaded.name, "same-name");

            Ok(())
        })
    }

    #[test]
    fn test_get_worker_saves_dir() -> Result<()> {
        with_temp_home(|| {
            let saves_dir = get_worker_saves_dir("test-worker")?;

            assert!(saves_dir.to_string_lossy().contains("test-worker"));
            assert!(saves_dir.to_string_lossy().contains(".geoengine"));
            assert!(saves_dir.to_string_lossy().contains("saves"));

            Ok(())
        })
    }

    #[test]
    fn test_cache_and_tag_config() -> Result<()> {
        with_temp_home(|| {
            let config = create_test_config("cache-test", "1.0.0");

            save_config(&config)?;

            cache_and_tag_config("cache-test", "1.0.0")?;

            // Verify map.json was created
            let saves_dir = get_worker_saves_dir("cache-test")?;
            let map_path = saves_dir.join("map.json");
            assert!(map_path.exists());

            // Load and verify mapping
            let map = VersionConfigMaps::load_from_worker("cache-test")?;
            assert_eq!(map.worker, "cache-test");
            assert!(map.mappings.is_some());

            let mappings = map.mappings.unwrap();
            assert!(mappings.contains_key("1.0.0"));

            // Verify snapshot file was created
            let hash = mappings.get("1.0.0").unwrap();
            let snapshot_path = saves_dir.join(format!("{}.json", hash));
            assert!(snapshot_path.exists());

            Ok(())
        })
    }

    #[test]
    fn test_cache_multiple_versions_same_content() -> Result<()> {
        with_temp_home(|| {
            let config1 = create_test_config("multi-test", "1.0.0");
            save_config(&config1)?;
            cache_and_tag_config("multi-test", "1.0.0")?;

            // Update version but keep content same
            let config2 = create_test_config("multi-test", "1.0.1");
            save_config(&config2)?;
            cache_and_tag_config("multi-test", "1.0.1")?;

            let map = VersionConfigMaps::load_from_worker("multi-test")?;
            let mappings = map.mappings.unwrap();

            // Both versions should map to the same hash (deduplication)
            let hash1 = mappings.get("1.0.0").unwrap();
            let hash2 = mappings.get("1.0.1").unwrap();
            assert_eq!(hash1, hash2);

            Ok(())
        })
    }

    #[test]
    fn test_rename_saves_dir() -> Result<()> {
        with_temp_home(|| {
            let config = create_test_config("old-worker", "1.0.0");
            save_config(&config)?;
            cache_and_tag_config("old-worker", "1.0.0")?;

            let old_saves = get_worker_saves_dir("old-worker")?;
            assert!(old_saves.exists());

            rename_saves_dir("old-worker", "new-worker")?;

            let new_saves = get_worker_saves_dir("new-worker")?;
            assert!(new_saves.exists());
            assert!(!old_saves.exists());

            // Verify map.json still exists in renamed directory
            let map_path = new_saves.join("map.json");
            assert!(map_path.exists());

            Ok(())
        })
    }

    #[test]
    fn test_delete_saves_dir() -> Result<()> {
        with_temp_home(|| {
            let config = create_test_config("delete-saves", "1.0.0");
            save_config(&config)?;
            cache_and_tag_config("delete-saves", "1.0.0")?;

            let saves_dir = get_worker_saves_dir("delete-saves")?;
            assert!(saves_dir.exists());

            delete_saves_dir("delete-saves")?;

            assert!(!saves_dir.exists());

            Ok(())
        })
    }

    #[test]
    fn test_delete_nonexistent_saves_dir() -> Result<()> {
        with_temp_home(|| {
            // Should not error
            delete_saves_dir("does-not-exist")?;
            Ok(())
        })
    }

    #[test]
    fn test_check_changed_config_no_previous_state() -> Result<()> {
        with_temp_home(|| {
            let temp_dir = TempDir::new()?;
            let worker_path = temp_dir.path().to_path_buf();

            // Create a geoengine.yaml
            std::fs::write(
                worker_path.join("geoengine.yaml"),
                "name: test\nversion: \"1.0.0\"\n",
            )?;

            // No previous state, should return true (changed)
            let changed = check_changed_config("test-worker", &worker_path)?;
            assert!(changed);

            Ok(())
        })
    }
}