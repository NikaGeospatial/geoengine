use crate::config::{state, yaml_store};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// Worker configuration loaded from geoengine.yaml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerConfig {
    /// Worker name (required)
    pub name: String,

    /// Worker version
    pub version: String,

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

/// Relevant configuration fields for caching.
/// Excludes `name` (directory key) and `version` (tracked in map.json).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelevantWorkerConfig {
    pub description: Option<String>,
    pub command: Option<CommandConfig>,
    pub local_dir_mounts: Option<Vec<MountConfig>>,
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

    /// Accepted file extensions (only for type: file). Each entry must start with '.'
    /// (e.g., [".tif", ".geotiff"]). Omit or set to [".*"] to accept all file types.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filetypes: Option<Vec<String>>,
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

    /// Extract version-relevant fields (excludes name and version).
    pub fn get_relevant_fields(&self) -> RelevantWorkerConfig {
        RelevantWorkerConfig {
            description: self.description.clone(),
            command: self.command.clone(),
            local_dir_mounts: self.local_dir_mounts.clone(),
        }
    }

    /// Compute a SHA-256 hash of config content fields only (excludes name and version).
    /// Used as the dedup key for saves snapshots — if only the version changes,
    /// the hash stays the same and we reuse the existing snapshot file.
    pub fn config_content_hash(&self) -> String {
        let content_fields = serde_json::json!({
            "description": self.description,
            "command": self.command.as_ref().map(|c| serde_json::to_value(c).unwrap_or_default()),
            "local_dir_mounts": self.local_dir_mounts.as_ref().map(|m| serde_json::to_value(m).unwrap_or_default())
        });
        state::sha256_string(&content_fields.to_string())
    }

    /// Compute a SHA-256 hash of the build-relevant fields:
    /// name, version, command, and local_dir_mounts.
    /// Excludes description, plugins, and deploy which don't affect the Docker image.
    pub fn build_relevant_hash(&self) -> String {
        let build_fields = serde_json::json!({
            "name": self.name,
            "version": self.version,
            "command": self.command.as_ref().map(|c| serde_json::to_value(c).unwrap_or_default()),
            "local_dir_mounts": self.local_dir_mounts.as_ref().map(|m| serde_json::to_value(m).unwrap_or_default())
        });
        state::sha256_string(&build_fields.to_string())
    }

    /// Create a template configuration for a new worker
    pub fn template(name: &str) -> Self {
        WorkerConfig {
            name: name.to_string(),
            version: "1.0.0".to_string(),
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
                        filetypes: None,
                    },
                    InputParameter {
                        name: "output_folder".to_string(),
                        param_type: "folder".to_string(),
                        required: Some(true),
                        default: None,
                        description: Some("Output folder for results".to_string()),
                        enum_values: None,
                        readonly: Some(false),
                        filetypes: None,
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
                        filetypes: None,
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
            deploy: Some(DeployConfig { tenant_id: None }),
        }
    }
}

impl RelevantWorkerConfig {
    /// Load extracted version-relevant worker config from JSON save file.
    pub fn load_json(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read relevant config file: {}", path.display()))?;

        let config: RelevantWorkerConfig = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse relevant config file: {}", path.display()))?;

        Ok(config)
    }

    /// Reconstruct a full WorkerConfig from the snapshot.
    /// `name` and `version` must be supplied since they are not stored in the snapshot.
    pub fn reconstruct_full_config(&self, name: &str, version: &str) -> WorkerConfig {
        WorkerConfig {
            name: name.to_string(),
            version: version.to_string(),
            description: self.description.clone(),
            command: self.command.clone(),
            local_dir_mounts: self.local_dir_mounts.clone(),
            plugins: None,
            deploy: None,
        }
    }
}

/// Mapping between version and configuration file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionConfigMaps {
    pub worker: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub mappings: Option<HashMap<String, String>>,
}

impl VersionConfigMaps {
    pub fn load_from_worker(name: &str) -> Result<Self> {
        let path = yaml_store::get_worker_saves_dir(name)
            .with_context(|| "Failed to find worker's saves directory")?
            .join("map.json");

        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read map file: {}", path.display()))?;

        let config: VersionConfigMaps = serde_json::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", path.display()))?;

        Ok(config)
    }

    pub fn save_to_worker(&self, name: &str) -> Result<()> {
        let path = yaml_store::get_worker_saves_dir(name)
            .with_context(|| "Failed to find worker's saves directory")?
            .join("map.json");

        let content = serde_json::to_string_pretty(self)
            .context("Failed to serialise worker mappings to JSON")?;

        std::fs::write(path, content).context("Failed to write mappings file")?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config(description: Option<&str>) -> WorkerConfig {
        WorkerConfig {
            name: "example-worker".to_string(),
            version: "1.2.3".to_string(),
            description: description.map(str::to_string),
            command: Some(CommandConfig {
                program: "python".to_string(),
                script: "main.py".to_string(),
                inputs: Some(vec![InputParameter {
                    name: "input".to_string(),
                    param_type: "file".to_string(),
                    required: Some(true),
                    default: None,
                    description: Some("Input raster".to_string()),
                    enum_values: None,
                    readonly: Some(true),
                    filetypes: Some(vec![".tif".to_string()]),
                }]),
            }),
            local_dir_mounts: Some(vec![MountConfig {
                host_path: "./data".to_string(),
                container_path: "/data".to_string(),
                readonly: Some(true),
            }]),
            plugins: None,
            deploy: None,
        }
    }

    #[test]
    fn build_relevant_hash_ignores_description_changes() {
        let with_description = sample_config(Some("First description"));
        let updated_description = sample_config(Some("Updated description"));

        assert_eq!(
            with_description.build_relevant_hash(),
            updated_description.build_relevant_hash()
        );
    }

    #[test]
    fn config_content_hash_tracks_description_changes() {
        let with_description = sample_config(Some("First description"));
        let updated_description = sample_config(Some("Updated description"));

        assert_ne!(
            with_description.config_content_hash(),
            updated_description.config_content_hash()
        );
    }
}
