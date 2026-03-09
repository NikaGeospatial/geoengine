use anyhow::{Context, Result};
use colored::Colorize;
use std::path::{Path, PathBuf};

use crate::config::state;

/// A single file belonging to an embedded GeoEngine plugin.
pub struct PluginFile {
    /// The plugin's subdirectory name inside `plugins/` (e.g. `"qgis-ge"`).
    pub plugin: &'static str,
    /// The file name within that subdirectory (e.g. `"metadata.txt"`).
    pub file: &'static str,
    /// The file's content, embedded at compile time.
    pub content: &'static str,
}

// All GeoEngine plugin files, auto-generated at compile time from the `plugins/` directory.
// Adding, renaming, or removing a plugin subdirectory or file is picked up automatically —
// no changes to this file are required.
include!(concat!(env!("OUT_DIR"), "/plugins_embedded.rs"));

const ARCGIS_GE_PLUGIN: &str = "arcgis-ge";
const QGIS_GE_PLUGIN: &str = "qgis-ge";

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

    write_plugin_files(ARCGIS_GE_PLUGIN, &toolbox_dir)?;
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

    write_plugin_files(QGIS_GE_PLUGIN, &geoengine_dir)?;

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
    let required: Vec<&str> = PLUGIN_FILES.iter()
        .filter(|pf| pf.plugin == ARCGIS_GE_PLUGIN)
        .map(|pf| pf.file)
        .collect();
    Ok(missing_files(&arcgis_dir, &required).is_empty())
}

/// Check if the GeoEngine plugin is installed in the QGIS plugin directory.
pub fn verify_qgis_plugin_installed() -> Result<bool> {
    let qgis_dir = find_qgis_plugin_dir()?.join("geoengine");
    let required: Vec<&str> = PLUGIN_FILES.iter()
        .filter(|pf| pf.plugin == QGIS_GE_PLUGIN)
        .map(|pf| pf.file)
        .collect();
    Ok(missing_files(&qgis_dir, &required).is_empty())
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

    let canonical: Vec<&PluginFile> = PLUGIN_FILES.iter()
        .filter(|pf| pf.plugin == QGIS_GE_PLUGIN)
        .collect();

    let needs_update = canonical.iter().any(|pf| {
        let path = geoengine_dir.join(pf.file);
        match std::fs::read_to_string(&path) {
            Ok(content) => state::sha256_string(&content) != state::sha256_string(pf.content),
            Err(_) => true, // missing counts as stale
        }
    });

    if !needs_update {
        return Ok(PluginPatchResult::UpToDate);
    }

    // Reinstall: wipe existing dir first (same as register_qgis)
    if geoengine_dir.exists() {
        std::fs::remove_dir_all(&geoengine_dir).with_context(|| {
            format!(
                "Failed to remove existing QGIS plugin directory: {}",
                geoengine_dir.display()
            )
        })?;
    }
    std::fs::create_dir_all(&geoengine_dir)?;

    match write_plugin_files(QGIS_GE_PLUGIN, &geoengine_dir) {
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

    let canonical: Vec<&PluginFile> = PLUGIN_FILES.iter()
        .filter(|pf| pf.plugin == ARCGIS_GE_PLUGIN)
        .collect();

    let needs_update = canonical.iter().any(|pf| {
        let path = toolbox_dir.join(pf.file);
        match std::fs::read_to_string(&path) {
            Ok(content) => state::sha256_string(&content) != state::sha256_string(pf.content),
            Err(_) => true,
        }
    });

    if !needs_update {
        return Ok(PluginPatchResult::UpToDate);
    }

    if toolbox_dir.exists() {
        for entry in std::fs::read_dir(&toolbox_dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = match path.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            let is_geoengine_related = name.to_ascii_lowercase().contains("geoengine");
            let is_current_arcgis_file = canonical.iter().any(|pf| pf.file == name);
            if !is_geoengine_related && !is_current_arcgis_file {
                continue;
            }
            if path.is_dir() {
                std::fs::remove_dir_all(&path).with_context(|| {
                    format!(
                        "Failed to remove existing ArcGIS plugin directory: {}",
                        path.display()
                    )
                })?;
            } else {
                std::fs::remove_file(&path).with_context(|| {
                    format!(
                        "Failed to remove existing ArcGIS plugin file: {}",
                        path.display()
                    )
                })?;
            }
        }
    }
    std::fs::create_dir_all(&toolbox_dir)?;

    match write_plugin_files(ARCGIS_GE_PLUGIN, &toolbox_dir) {
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

/// Writes all embedded files for the given plugin to `dir`.
fn write_plugin_files(plugin: &str, dir: &Path) -> Result<()> {
    for pf in PLUGIN_FILES.iter().filter(|pf| pf.plugin == plugin) {
        let dst = dir.join(pf.file);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(dst, pf.content)?;
    }
    Ok(())
}
