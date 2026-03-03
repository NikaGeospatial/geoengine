use anyhow::{Context, Result};
use colored::Colorize;
use std::path::PathBuf;

use crate::config::state;

/// Install the GeoEngine plugin into ArcGIS Pro's toolbox directory.
pub async fn register_arcgis(custom_path: Option<PathBuf>) -> Result<()> {
    println!(
        "{} Registering GeoEngine with ArcGIS Pro...",
        "=>".blue().bold()
    );

    let toolbox_dir = if let Some(path) = custom_path {
        path
    } else {
        find_arcgis_toolbox_dir()?
    };

    std::fs::create_dir_all(&toolbox_dir)?;

    write_arcgis_plugin(&toolbox_dir)?;
    println!(
        "{} Installed GeoEngine toolbox to: {}",
        "✓".green().bold(),
        toolbox_dir.display()
    );

    Ok(())
}

/// Install the GeoEngine plugin into QGIS's plugin directory.
pub async fn register_qgis(custom_path: Option<PathBuf>) -> Result<()> {
    println!(
        "{} Registering GeoEngine with QGIS...",
        "=>".blue().bold()
    );

    let plugin_dir = if let Some(path) = custom_path {
        path
    } else {
        find_qgis_plugin_dir()?
    };

    let geoengine_dir = plugin_dir.join("geoengine");
    std::fs::create_dir_all(&geoengine_dir)?;

    write_qgis_plugin(&geoengine_dir)?;

    println!(
        "{} Installed GeoEngine plugin to: {}",
        "✓".green().bold(),
        geoengine_dir.display()
    );

    Ok(())
}


fn missing_files(base: &PathBuf, required: &[&str]) -> Vec<String> {
    required
        .iter()
        .filter_map(|f| {
            let p = base.join(f);
            if p.exists() { None } else { Some((*f).to_string()) }
        })
        .collect()
}

/// Check if the GeoEngine plugin is installed in the ArcGIS Pro toolbox directory.
pub fn verify_arcgis_plugin_installed() -> Result<bool> {
    let arcgis_dir = find_arcgis_toolbox_dir()?;
    let arcgis_required = ["GeoEngineTools.pyt", "geoengine_client.py"];
    let arcgis_missing = missing_files(&arcgis_dir, &arcgis_required);
    Ok(arcgis_missing.is_empty())
}

/// Check if the GeoEngine plugin is installed in the QGIS plugin directory.
pub fn verify_qgis_plugin_installed() -> Result<bool> {
    let qgis_dir = find_qgis_plugin_dir()?.join("geoengine");
    let qgis_required = ["__init__.py", "geoengine_plugin.py", "geoengine_provider.py", "metadata.txt"];
    let qgis_missing = missing_files(&qgis_dir, &qgis_required);
    Ok(qgis_missing.is_empty())
}

/// Patch outcome returned to callers (used by `geoengine patch`).
pub enum PluginPatchResult {
    /// The plugin directory parent does not exist — GIS not installed on this machine.
    NotInstalled,
    /// All installed files already match the canonical embedded content.
    UpToDate,
    /// At least one file was stale; the plugin was reinstalled successfully.
    Updated,
    /// Reinstall was attempted but failed.
    Failed(anyhow::Error),
}


/// Check the installed QGIS plugin against the canonical embedded files and reinstall
/// if any file is missing or has a different hash. If QGIS is not installed on this
/// machine (parent directory absent), returns `PluginPatchResult::NotInstalled`.
pub async fn patch_qgis() -> Result<PluginPatchResult> {
    let plugin_dir = match find_qgis_plugin_dir() {
        Ok(d) => d,
        Err(_) => return Ok(PluginPatchResult::NotInstalled),
    };

    // QGIS is considered present when the *parent* of the plugins dir exists.
    if !plugin_dir.parent().map_or(false, |p| p.exists()) {
        return Ok(PluginPatchResult::NotInstalled);
    }

    let geoengine_dir = plugin_dir.join("geoengine");

    // Canonical content (embedded at compile time)
    let canonical: &[(&str, &str)] = &[
        ("__init__.py",          include_str!("../../plugins/qgis-ge/__init__.py")),
        ("geoengine_plugin.py",  include_str!("../../plugins/qgis-ge/geoengine_plugin.py")),
        ("geoengine_provider.py",include_str!("../../plugins/qgis-ge/geoengine_provider.py")),
        ("geoengine_widgets.py", include_str!("../../plugins/qgis-ge/geoengine_widgets.py")),
        ("metadata.txt",         include_str!("../../plugins/qgis-ge/metadata.txt")),
    ];

    let needs_update = canonical.iter().any(|(filename, expected)| {
        let path = geoengine_dir.join(filename);
        match std::fs::read_to_string(&path) {
            Ok(content) => state::sha256_string(&content) != state::sha256_string(expected),
            Err(_) => true, // missing counts as stale
        }
    });

    if !needs_update {
        return Ok(PluginPatchResult::UpToDate);
    }

    // Reinstall: wipe existing dir first (same as debug-qgis)
    if geoengine_dir.exists() {
        std::fs::remove_dir_all(&geoengine_dir).with_context(|| {
            format!(
                "Failed to remove existing QGIS plugin directory: {}",
                geoengine_dir.display()
            )
        })?;
    }
    std::fs::create_dir_all(&geoengine_dir)?;

    match write_qgis_plugin(&geoengine_dir) {
        Ok(_) => Ok(PluginPatchResult::Updated),
        Err(e) => Ok(PluginPatchResult::Failed(e)),
    }
}

/// Check the installed ArcGIS plugin against the canonical embedded files and reinstall
/// if any file is missing or has a different hash. If ArcGIS is not installed on this
/// machine (toolbox parent directory absent), returns `PluginPatchResult::NotInstalled`.
pub async fn patch_arcgis() -> Result<PluginPatchResult> {
    let toolbox_dir = match find_arcgis_toolbox_dir() {
        Ok(d) => d,
        Err(_) => return Ok(PluginPatchResult::NotInstalled),
    };

    // ArcGIS is considered present when the parent of the Toolboxes dir exists.
    if !toolbox_dir.parent().map_or(false, |p| p.exists()) {
        return Ok(PluginPatchResult::NotInstalled);
    }

    // Canonical content (embedded at compile time)
    let canonical: &[(&str, &str)] = &[
        ("GeoEngineTools.pyt",  include_str!("../../plugins/arcgis-ge/GeoEngineTools.pyt")),
        ("geoengine_client.py", include_str!("../../plugins/arcgis-ge/geoengine_client.py")),
    ];

    let needs_update = canonical.iter().any(|(filename, expected)| {
        let path = toolbox_dir.join(filename);
        match std::fs::read_to_string(&path) {
            Ok(content) => state::sha256_string(&content) != state::sha256_string(expected),
            Err(_) => true,
        }
    });

    if !needs_update {
        return Ok(PluginPatchResult::UpToDate);
    }

    std::fs::create_dir_all(&toolbox_dir)?;

    match write_arcgis_plugin(&toolbox_dir) {
        Ok(_) => Ok(PluginPatchResult::Updated),
        Err(e) => Ok(PluginPatchResult::Failed(e)),
    }
}

fn find_arcgis_toolbox_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;

    let candidates = [
        home.join("Documents").join("ArcGIS").join("Toolboxes"),
        home.join("ArcGIS").join("Toolboxes"),
    ];

    for candidate in &candidates {
        if candidate.parent().map(|p| p.exists()).unwrap_or(false) {
            return Ok(candidate.clone());
        }
    }

    Ok(candidates[0].clone())
}

fn find_qgis_plugin_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not find home directory")?;

    #[cfg(target_os = "windows")]
    let plugin_dir = home
        .join("AppData")
        .join("Roaming")
        .join("QGIS")
        .join("QGIS3")
        .join("profiles")
        .join("default")
        .join("python")
        .join("plugins");

    #[cfg(target_os = "macos")]
    let plugin_dir = home
        .join("Library")
        .join("Application Support")
        .join("QGIS")
        .join("QGIS3")
        .join("profiles")
        .join("default")
        .join("python")
        .join("plugins");

    #[cfg(target_os = "linux")]
    let plugin_dir = home
        .join(".local")
        .join("share")
        .join("QGIS")
        .join("QGIS3")
        .join("profiles")
        .join("default")
        .join("python")
        .join("plugins");

    Ok(plugin_dir)
}

fn write_arcgis_plugin(dir: &PathBuf) -> Result<()> {
    let toolbox_content = include_str!("../../plugins/arcgis-ge/GeoEngineTools.pyt");
    std::fs::write(dir.join("GeoEngineTools.pyt"), toolbox_content)?;

    let client_content = include_str!("../../plugins/arcgis-ge/geoengine_client.py");
    std::fs::write(dir.join("geoengine_client.py"), client_content)?;

    Ok(())
}

fn write_qgis_plugin(dir: &PathBuf) -> Result<()> {
    let init_content = include_str!("../../plugins/qgis-ge/__init__.py");
    std::fs::write(dir.join("__init__.py"), init_content)?;

    let plugin_content = include_str!("../../plugins/qgis-ge/geoengine_plugin.py");
    std::fs::write(dir.join("geoengine_plugin.py"), plugin_content)?;

    let provider_content = include_str!("../../plugins/qgis-ge/geoengine_provider.py");
    std::fs::write(dir.join("geoengine_provider.py"), provider_content)?;

    let widgets_content = include_str!("../../plugins/qgis-ge/geoengine_widgets.py");
    std::fs::write(dir.join("geoengine_widgets.py"), widgets_content)?;

    let metadata_content = include_str!("../../plugins/qgis-ge/metadata.txt");
    std::fs::write(dir.join("metadata.txt"), metadata_content)?;

    Ok(())
}
