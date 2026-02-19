use anyhow::{Context, Result};
use colored::Colorize;
use std::path::PathBuf;

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

/// Debug helper that installs the QGIS plugin only when missing.
pub async fn debug_qgis() -> Result<()> {
    if verify_qgis_plugin_installed()? {
        println!(
            "{} QGIS plugin is already installed. No action taken.",
            "=>".yellow().bold()
        );
        return Ok(());
    }

    register_qgis(None).await
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

    let metadata_content = include_str!("../../plugins/qgis-ge/metadata.txt");
    std::fs::write(dir.join("metadata.txt"), metadata_content)?;

    Ok(())
}
