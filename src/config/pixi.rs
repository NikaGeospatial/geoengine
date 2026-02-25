use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PixiConfig {
    /// Main workspace configuration of Pixi
    workspace: PixiWorkspaceConfig,
    /// Dependencies to be installed in the environment
    #[serde(default)]
    dependencies: HashMap<String, PixiDepSpec>,
    /// PyPI dependencies to be installed in the environment
    #[serde(skip_serializing_if = "Option::is_none", rename = "pypi-dependencies")]
    pypi_dependencies: Option<HashMap<String, PixiDepSpec>>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct PixiWorkspaceConfig {
    /// Name of project
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    /// Version of project
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<String>,
    /// Description of project
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    /// Channels to install from (required)
    channels: Vec<String>,
    /// Platforms that the project supports (required)
    platforms: Vec<String>
}

/// Represents a dependency specification in the Pixi ecosystem, which can be either
/// a simple string (version), or a detailed object.
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(untagged)]
pub enum PixiDepSpec {
    Simple(String),
    Detailed {
        version: Option<String>,
        channel: Option<String>,
        extras: Option<Vec<String>>,
        git: Option<String>,
    }
}

impl PixiConfig {
    /// Workspace config template
    fn workspace_config(name: &str) -> PixiWorkspaceConfig {
        PixiWorkspaceConfig {
            name: Some(name.to_string()),
            version: Some("0.1.0".to_string()),
            description: Some("A geoengine project".to_string()),
            channels: vec!["conda-forge".to_string()],
            platforms: vec!["linux-64".to_string(), "linux-aarch64".to_string()]
        }
    }

    /// Generate base template using workspace configuration (for Python + GDAL)
    pub fn py_template(name: &str) -> Self {
        PixiConfig {
            workspace: Self::workspace_config(name),
            dependencies: HashMap::from(
                [
                    ("python".to_string(), PixiDepSpec::Simple(">=3.11,<3.13".to_string())),
                    ("pip".to_string(), PixiDepSpec::Simple("*".to_string())),
                    ("gdal".to_string(), PixiDepSpec::Simple(">=3.9.0,<4".to_string())),
                    ("fiona".to_string(), PixiDepSpec::Simple(">=1.9,<2".to_string())),
                    ("shapely".to_string(), PixiDepSpec::Simple(">=2.0,<3".to_string())),
                    ("pyproj".to_string(), PixiDepSpec::Simple(">=3.6,<4".to_string())),
                    ("geopandas".to_string(), PixiDepSpec::Simple(">=0.14,<1".to_string())),
                    ("numpy".to_string(), PixiDepSpec::Simple(">=1.26,<3".to_string())),
                    ("scipy".to_string(), PixiDepSpec::Simple(">=1.11,<2".to_string())),
                    ("pandas".to_string(), PixiDepSpec::Simple(">=2.1,<3".to_string())),
                    ("zarr".to_string(), PixiDepSpec::Simple("*".to_string())),
                    ("xarray".to_string(), PixiDepSpec::Simple("*".to_string())),
                ]
            ),
            pypi_dependencies: None,
        }
    }

    /// Generate base template using workspace configuration (for R + GDAL)
    pub fn r_template(name: &str) -> Self {
        PixiConfig{
            workspace: Self::workspace_config(name),
            dependencies: HashMap::from(
                [
                    ("r-base".to_string(), PixiDepSpec::Simple(">=4.3".to_string())),
                    ("r-recommended".to_string(), PixiDepSpec::Simple("*".to_string())),
                    ("r-sf".to_string(), PixiDepSpec::Simple("*".to_string())),
                    ("r-terra".to_string(), PixiDepSpec::Simple("*".to_string())),
                    ("r-stars".to_string(), PixiDepSpec::Simple("*".to_string())),
                ]
            ),
            pypi_dependencies: None,
        }
    }
}