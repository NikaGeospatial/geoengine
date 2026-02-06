use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Project configuration loaded from geoengine.yaml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    /// Project name (required)
    pub name: String,

    /// Project version
    pub version: Option<String>,

    /// Base Docker image to use
    pub base_image: Option<String>,

    /// Build configuration
    pub build: Option<BuildConfig>,

    /// Runtime configuration
    pub runtime: Option<RuntimeConfig>,

    /// Named scripts that can be run
    pub scripts: Option<HashMap<String, String>>,

    /// GIS integration configuration
    pub gis: Option<GisConfig>,

    /// Deployment configuration
    pub deploy: Option<DeployConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildConfig {
    /// Path to Dockerfile (relative to project root)
    pub dockerfile: Option<String>,

    /// Build context directory
    pub context: Option<String>,

    /// Build arguments
    pub args: Option<HashMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeConfig {
    /// Enable GPU passthrough
    #[serde(default)]
    pub gpu: bool,

    /// Memory limit (e.g., "8g", "512m")
    pub memory: Option<String>,

    /// Number of CPUs
    pub cpus: Option<f64>,

    /// Shared memory size (for PyTorch DataLoader)
    pub shm_size: Option<String>,

    /// Volume mounts
    pub mounts: Option<Vec<MountConfig>>,

    /// Environment variables
    pub environment: Option<HashMap<String, String>>,

    /// Working directory inside container
    pub workdir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountConfig {
    /// Host path (can be relative with ./)
    pub host: String,

    /// Container path
    pub container: String,

    /// Mount as read-only
    pub readonly: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GisConfig {
    /// Tools exposed to GIS applications
    pub tools: Option<Vec<GisTool>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GisTool {
    /// Tool identifier (used in API calls)
    pub name: String,

    /// Display label in GIS UI
    pub label: Option<String>,

    /// Tool description
    pub description: Option<String>,

    /// Script name to execute (from scripts section)
    pub script: String,

    /// Input parameters
    pub inputs: Option<Vec<ToolParameter>>,

    /// Output parameters
    pub outputs: Option<Vec<ToolParameter>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolParameter {
    /// Parameter name
    pub name: String,

    /// Display label
    pub label: Option<String>,

    /// Optional explicit mapping to script's input parameter. Otherwise, parameter name is used.
    pub map_to: Option<String>,

    /// Parameter type: raster, vector, string, int, float, bool
    #[serde(rename = "type")]
    pub param_type: String,

    /// Default value
    pub default: Option<serde_yaml::Value>,

    /// Whether the parameter is required
    pub required: Option<bool>,

    /// Description/help text
    pub description: Option<String>,

    /// For choice parameters, list of valid options
    pub choices: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployConfig {
    /// GCP project ID
    pub gcp_project: Option<String>,

    /// GCP region
    pub region: Option<String>,

    /// Artifact Registry repository name
    pub repository: Option<String>,
}

impl ProjectConfig {
    /// Load project configuration from a YAML file
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let config: ProjectConfig = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

        Ok(config)
    }

    /// Create a template configuration for a new project
    pub fn template(name: &str) -> Self {
        let mut scripts = HashMap::new();
        scripts.insert("default".to_string(), "python main.py".to_string());
        scripts.insert("train".to_string(), "python train.py".to_string());
        scripts.insert("process".to_string(), "Rscript process.R".to_string());

        let mut environment = HashMap::new();
        environment.insert("PYTHONUNBUFFERED".to_string(), "1".to_string());
        environment.insert("GDAL_DATA".to_string(), "/usr/share/gdal".to_string());
        environment.insert("PROJ_LIB".to_string(), "/usr/share/proj".to_string());

        let mut build_args = HashMap::new();
        build_args.insert("PYTHON_VERSION".to_string(), "3.11".to_string());

        ProjectConfig {
            name: name.to_string(),
            version: Some("1.0".to_string()),
            base_image: Some("nikaruntime/nika-runtime:latest".to_string()),
            build: Some(BuildConfig {
                dockerfile: Some("./Dockerfile".to_string()),
                context: Some(".".to_string()),
                args: Some(build_args),
            }),
            runtime: Some(RuntimeConfig {
                gpu: true,
                memory: Some("8g".to_string()),
                cpus: Some(4.0),
                shm_size: Some("2g".to_string()),
                mounts: Some(vec![
                    MountConfig {
                        host: "./data".to_string(),
                        container: "/data".to_string(),
                        readonly: Some(false),
                    },
                    MountConfig {
                        host: "./output".to_string(),
                        container: "/output".to_string(),
                        readonly: Some(false),
                    },
                ]),
                environment: Some(environment),
                workdir: Some("/workspace".to_string()),
            }),
            scripts: Some(scripts),
            gis: Some(GisConfig {
                tools: Some(vec![GisTool {
                    name: "example_tool".to_string(),
                    label: Some("Example Tool".to_string()),
                    description: Some("An example geoprocessing tool".to_string()),
                    script: "default".to_string(),
                    inputs: Some(vec![
                        ToolParameter {
                            name: "input_raster".to_string(),
                            label: Some("Input Raster".to_string()),
                            map_to: None,
                            param_type: "raster".to_string(),
                            default: None,
                            required: Some(true),
                            description: Some("Input raster file".to_string()),
                            choices: None,
                        },
                    ]),
                    outputs: Some(vec![
                        ToolParameter {
                            name: "output_raster".to_string(),
                            label: Some("Output Raster".to_string()),
                            map_to: None,
                            param_type: "raster".to_string(),
                            default: None,
                            required: Some(true),
                            description: Some("Output raster file".to_string()),
                            choices: None,
                        },
                    ]),
                }]),
            }),
            deploy: Some(DeployConfig {
                gcp_project: Some("your-gcp-project".to_string()),
                region: Some("us-central1".to_string()),
                repository: Some("geoengine".to_string()),
            }),
        }
    }
}
