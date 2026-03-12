use anyhow::{Context, Result};
use std::path::PathBuf;

/// Get the GeoEngine configuration directory (~/.geoengine)
pub fn get_config_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;
    let config_dir = home.join(".geoengine");
    std::fs::create_dir_all(&config_dir)?;
    Ok(config_dir)
}

/// Get the settings file path
pub fn get_settings_file() -> Result<PathBuf> {
    Ok(get_config_dir()?.join("settings.yaml"))
}

/// Get temporary directory for file transfers
pub fn get_temp_dir() -> Result<PathBuf> {
    let temp_dir = get_config_dir()?.join("tmp");
    std::fs::create_dir_all(&temp_dir)?;
    Ok(temp_dir)
}

/// Get the state directory for worker apply state tracking
pub fn get_state_dir() -> Result<PathBuf> {
    let state_dir = get_config_dir()?.join("state");
    std::fs::create_dir_all(&state_dir)?;
    Ok(state_dir)
}

/// Get saves directory for per-worker config caching
pub fn get_saves_dir() -> Result<PathBuf> {
    let saves_dir = get_config_dir()?.join("saves");
    std::fs::create_dir_all(&saves_dir)?;
    Ok(saves_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use tempfile::TempDir;

    fn with_temp_home<F>(test: F)
    where
        F: FnOnce() -> Result<()>,
    {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let original_home = env::var("HOME").ok();

        env::set_var("HOME", temp_dir.path());

        let result = test();

        // Restore original HOME
        if let Some(home) = original_home {
            env::set_var("HOME", home);
        } else {
            env::remove_var("HOME");
        }

        result.expect("Test failed");
    }

    #[test]
    fn test_get_config_dir_creates_directory() {
        with_temp_home(|| {
            let config_dir = get_config_dir()?;
            assert!(config_dir.exists());
            assert!(config_dir.ends_with(".geoengine"));
            Ok(())
        });
    }

    #[test]
    fn test_get_settings_file() {
        with_temp_home(|| {
            let settings_file = get_settings_file()?;
            assert!(settings_file.ends_with("settings.yaml"));
            assert!(settings_file.parent().unwrap().exists());
            Ok(())
        });
    }

    #[test]
    fn test_get_temp_dir_creates_directory() {
        with_temp_home(|| {
            let temp_dir = get_temp_dir()?;
            assert!(temp_dir.exists());
            assert!(temp_dir.ends_with("tmp"));
            Ok(())
        });
    }

    #[test]
    fn test_get_state_dir_creates_directory() {
        with_temp_home(|| {
            let state_dir = get_state_dir()?;
            assert!(state_dir.exists());
            assert!(state_dir.ends_with("state"));
            Ok(())
        });
    }

    #[test]
    fn test_get_saves_dir_creates_directory() {
        with_temp_home(|| {
            let saves_dir = get_saves_dir()?;
            assert!(saves_dir.exists());
            assert!(saves_dir.ends_with("saves"));
            Ok(())
        });
    }

    #[test]
    fn test_all_paths_under_config_dir() {
        with_temp_home(|| {
            let config_dir = get_config_dir()?;
            let settings_file = get_settings_file()?;
            let temp_dir = get_temp_dir()?;
            let state_dir = get_state_dir()?;
            let saves_dir = get_saves_dir()?;

            assert!(settings_file.starts_with(&config_dir));
            assert!(temp_dir.starts_with(&config_dir));
            assert!(state_dir.starts_with(&config_dir));
            assert!(saves_dir.starts_with(&config_dir));
            Ok(())
        });
    }

    #[test]
    fn test_idempotent_directory_creation() {
        with_temp_home(|| {
            // Call twice to ensure no error on second call
            let dir1 = get_config_dir()?;
            let dir2 = get_config_dir()?;
            assert_eq!(dir1, dir2);
            assert!(dir1.exists());
            Ok(())
        });
    }
}