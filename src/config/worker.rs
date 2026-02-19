use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use crate::config::state;

/// Worker configuration loaded from geoengine.yaml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    /// Worker name (required)
    pub name: String,

    /// Worker version
    pub version: Option<String>,

    /// Worker description
    pub description: Option<String>,

    /// Command configuration (entrypoint + inputs)
    pub command: Option<CommandConfig>,

    /// Local directory mounts
    pub local_dir_mounts: Option<Vec<MountConfig>>,

    /// GIS plugin registration
    pub plugins: Option<PluginsConfig>,

    /// Deployment configuration
    pub deploy: Option<DeployConfig>,
}

/// Command configuration defining the entrypoint and input parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandConfig {
    /// Program to run (e.g., "python")
    pub program: String,

    /// Script to execute (e.g., "main.py")
    pub script: String,

    /// Input parameter definitions
    pub inputs: Option<Vec<InputParameter>>,
}

/// Input parameter definition for a worker command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputParameter {
    /// Parameter name (becomes --name flag)
    pub name: String,

    /// Parameter type: file, folder, datetime, string, number, boolean, enum
    #[serde(rename = "type")]
    pub param_type: String,

    /// Whether the parameter is required
    pub required: Option<bool>,

    /// Default value (optional)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<serde_yaml::Value>,

    /// Description/help text
    pub description: Option<String>,

    /// Possible values (only for type: enum)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,

    /// Mark as readonly (only for types folder and file, defaults to true)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readonly: Option<bool>,
}

/// Volume mount configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountConfig {
    /// Host path (can be relative with ./)
    pub host_path: String,

    /// Container path
    pub container_path: String,

    /// Mount as read-only
    pub readonly: Option<bool>,
}

/// GIS plugin registration settings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginsConfig {
    /// Register with ArcGIS Pro
    pub arcgis: Option<bool>,

    /// Register with QGIS
    pub qgis: Option<bool>,
}

/// Deployment configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployConfig {
    /// Tenant ID (placeholder for future use)
    pub tenant_id: Option<String>,
}

impl WorkerConfig {
    /// Load worker configuration from a YAML file
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path.display()))?;

        let config: WorkerConfig = serde_yaml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

        Ok(config)
    }

    /// Compute a SHA-256 hash of only the build-relevant fields:
    /// name, version, command, and local_dir_mounts.
    /// This excludes description, plugins, and deploy which don't affect the Docker image.
    pub fn build_relevant_hash(&self) -> String {
        let build_fields = serde_json::json!({
            "name": self.name,
            "command": self.command.as_ref().map(|c| serde_json::to_value(c).unwrap_or_default()),
            "local_dir_mounts": self.local_dir_mounts.as_ref().map(|m| serde_json::to_value(m).unwrap_or_default()),
        });
        state::sha256_string(&build_fields.to_string())
    }

    /// Create a template configuration for a new worker
    pub fn template(name: &str) -> Self {
        WorkerConfig {
            name: name.to_string(),
            version: Some("1.0".to_string()),
            description: Some("A geoengine worker".to_string()),
            command: Some(CommandConfig {
                program: "python".to_string(),
                script: "main.py".to_string(),
                inputs: Some(vec![
                    InputParameter {
                        name: "input_file".to_string(),
                        param_type: "file".to_string(),
                        required: Some(true),
                        default: None,
                        description: Some("Input file to process".to_string()),
                        enum_values: None,
                        readonly: Some(true),
                    },
                    InputParameter {
                        name: "output_folder".to_string(),
                        param_type: "folder".to_string(),
                        required: Some(true),
                        default: None,
                        description: Some("Output folder for results".to_string()),
                        enum_values: None,
                        readonly: Some(false),
                    },
                    InputParameter {
                        name: "format".to_string(),
                        param_type: "enum".to_string(),
                        required: Some(false),
                        default: Some(serde_yaml::Value::String("geotiff".to_string())),
                        description: Some("Output format".to_string()),
                        enum_values: Some(vec![
                            "geotiff".to_string(),
                            "png".to_string(),
                            "jpeg".to_string(),
                        ]),
                        readonly: None,
                    },
                ]),
            }),
            local_dir_mounts: Some(vec![
                MountConfig {
                    host_path: "./data".to_string(),
                    container_path: "/data".to_string(),
                    readonly: Some(false),
                },
                MountConfig {
                    host_path: "./output".to_string(),
                    container_path: "/output".to_string(),
                    readonly: Some(false),
                },
            ]),
            plugins: Some(PluginsConfig {
                arcgis: Some(false),
                qgis: Some(false),
            }),
            deploy: Some(DeployConfig {
                tenant_id: None,
            }),
        }
    }
}
