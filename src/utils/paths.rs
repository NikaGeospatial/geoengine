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
