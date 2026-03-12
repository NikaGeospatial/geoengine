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

    /// Compute a SHA-256 hash of the version-relevant fields:
    /// name, version, description, command, and local_dir_mounts.
    /// Excludes plugins and deploy which don't affect the Docker image.
    pub fn build_relevant_hash(&self) -> String {
        let build_fields = serde_json::json!({
            "name": self.name,
            "version": self.version,
            "description": self.description,
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
    use std::env;
    use std::fs;
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

    #[test]
    fn test_worker_config_template() {
        let config = WorkerConfig::template("test-worker");

        assert_eq!(config.name, "test-worker");
        assert_eq!(config.version, "1.0.0");
        assert!(config.description.is_some());
        assert!(config.command.is_some());
        assert!(config.local_dir_mounts.is_some());
        assert!(config.plugins.is_some());

        let command = config.command.unwrap();
        assert_eq!(command.program, "python");
        assert_eq!(command.script, "main.py");
        assert!(command.inputs.is_some());

        let inputs = command.inputs.unwrap();
        assert!(!inputs.is_empty());
        assert_eq!(inputs[0].name, "input_file");
        assert_eq!(inputs[0].param_type, "file");
    }

    #[test]
    fn test_worker_config_load() -> Result<()> {
        let temp_dir = TempDir::new()?;
        let config_path = temp_dir.path().join("geoengine.yaml");

        let yaml_content = r#"
name: test-worker
version: "1.2.3"
description: "Test worker description"
command:
  program: python
  script: main.py
  inputs:
    - name: input-file
      type: file
      required: true
      description: "Input file"
      readonly: true
      filetypes:
        - .tif
        - .tiff
local_dir_mounts:
  - host_path: ./data
    container_path: /data
    readonly: false
plugins:
  arcgis: false
  qgis: true
"#;

        fs::write(&config_path, yaml_content)?;

        let config = WorkerConfig::load(&config_path)?;

        assert_eq!(config.name, "test-worker");
        assert_eq!(config.version, "1.2.3");
        assert_eq!(config.description, Some("Test worker description".to_string()));

        let command = config.command.unwrap();
        assert_eq!(command.program, "python");
        assert_eq!(command.script, "main.py");

        let inputs = command.inputs.unwrap();
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0].name, "input-file");
        assert_eq!(inputs[0].param_type, "file");
        assert_eq!(inputs[0].required, Some(true));
        assert_eq!(inputs[0].readonly, Some(true));
        assert_eq!(inputs[0].filetypes, Some(vec![".tif".to_string(), ".tiff".to_string()]));

        let mounts = config.local_dir_mounts.unwrap();
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].host_path, "./data");
        assert_eq!(mounts[0].container_path, "/data");

        let plugins = config.plugins.unwrap();
        assert_eq!(plugins.arcgis, Some(false));
        assert_eq!(plugins.qgis, Some(true));

        Ok(())
    }

    #[test]
    fn test_config_content_hash_consistency() {
        let config = WorkerConfig {
            name: "worker1".to_string(),
            version: "1.0.0".to_string(),
            description: Some("Test".to_string()),
            command: Some(CommandConfig {
                program: "python".to_string(),
                script: "main.py".to_string(),
                inputs: None,
            }),
            local_dir_mounts: None,
            plugins: None,
            deploy: None,
        };

        let hash1 = config.config_content_hash();
        let hash2 = config.config_content_hash();
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64);
    }

    #[test]
    fn test_config_content_hash_ignores_name_and_version() {
        let config1 = WorkerConfig {
            name: "worker1".to_string(),
            version: "1.0.0".to_string(),
            description: Some("Test".to_string()),
            command: Some(CommandConfig {
                program: "python".to_string(),
                script: "main.py".to_string(),
                inputs: None,
            }),
            local_dir_mounts: None,
            plugins: None,
            deploy: None,
        };

        let config2 = WorkerConfig {
            name: "worker2".to_string(),
            version: "2.0.0".to_string(),
            description: Some("Test".to_string()),
            command: Some(CommandConfig {
                program: "python".to_string(),
                script: "main.py".to_string(),
                inputs: None,
            }),
            local_dir_mounts: None,
            plugins: None,
            deploy: None,
        };

        // Content hash should be same since only name/version differ
        assert_eq!(config1.config_content_hash(), config2.config_content_hash());
    }

    #[test]
    fn test_build_relevant_hash_includes_name_and_version() {
        let config1 = WorkerConfig {
            name: "worker1".to_string(),
            version: "1.0.0".to_string(),
            description: Some("Test".to_string()),
            command: None,
            local_dir_mounts: None,
            plugins: None,
            deploy: None,
        };

        let config2 = WorkerConfig {
            name: "worker2".to_string(),
            version: "1.0.0".to_string(),
            description: Some("Test".to_string()),
            command: None,
            local_dir_mounts: None,
            plugins: None,
            deploy: None,
        };

        // Build hash should differ when name differs
        assert_ne!(config1.build_relevant_hash(), config2.build_relevant_hash());
    }

    #[test]
    fn test_get_relevant_fields() {
        let config = WorkerConfig {
            name: "worker".to_string(),
            version: "1.0.0".to_string(),
            description: Some("Test".to_string()),
            command: Some(CommandConfig {
                program: "python".to_string(),
                script: "main.py".to_string(),
                inputs: None,
            }),
            local_dir_mounts: Some(vec![MountConfig {
                host_path: "./data".to_string(),
                container_path: "/data".to_string(),
                readonly: Some(true),
            }]),
            plugins: Some(PluginsConfig {
                arcgis: Some(true),
                qgis: Some(false),
            }),
            deploy: Some(DeployConfig {
                tenant_id: Some("test-tenant".to_string()),
            }),
        };

        let relevant = config.get_relevant_fields();

        assert_eq!(relevant.description, Some("Test".to_string()));
        assert!(relevant.command.is_some());
        assert!(relevant.local_dir_mounts.is_some());

        let command = relevant.command.unwrap();
        assert_eq!(command.program, "python");
        assert_eq!(command.script, "main.py");

        let mounts = relevant.local_dir_mounts.unwrap();
        assert_eq!(mounts.len(), 1);
        assert_eq!(mounts[0].host_path, "./data");
    }

    #[test]
    fn test_relevant_config_reconstruct() {
        let relevant = RelevantWorkerConfig {
            description: Some("Test worker".to_string()),
            command: Some(CommandConfig {
                program: "python".to_string(),
                script: "script.py".to_string(),
                inputs: None,
            }),
            local_dir_mounts: Some(vec![MountConfig {
                host_path: "./data".to_string(),
                container_path: "/data".to_string(),
                readonly: Some(false),
            }]),
        };

        let full_config = relevant.reconstruct_full_config("my-worker", "2.0.0");

        assert_eq!(full_config.name, "my-worker");
        assert_eq!(full_config.version, "2.0.0");
        assert_eq!(full_config.description, Some("Test worker".to_string()));
        assert!(full_config.command.is_some());
        assert!(full_config.local_dir_mounts.is_some());
        assert!(full_config.plugins.is_none());
        assert!(full_config.deploy.is_none());
    }

    #[test]
    fn test_version_config_maps_save_and_load() -> Result<()> {
        with_temp_home(|| {
            let mut mappings = std::collections::HashMap::new();
            mappings.insert("1.0.0".to_string(), "hash1".to_string());
            mappings.insert("1.0.1".to_string(), "hash2".to_string());

            let map = VersionConfigMaps {
                worker: "test-worker".to_string(),
                mappings: Some(mappings),
            };

            // Create saves dir
            let saves_dir = yaml_store::get_worker_saves_dir("test-worker")?;
            std::fs::create_dir_all(&saves_dir)?;

            map.save_to_worker("test-worker")?;

            let loaded = VersionConfigMaps::load_from_worker("test-worker")?;

            assert_eq!(loaded.worker, "test-worker");
            assert!(loaded.mappings.is_some());

            let loaded_mappings = loaded.mappings.unwrap();
            assert_eq!(loaded_mappings.len(), 2);
            assert_eq!(loaded_mappings.get("1.0.0"), Some(&"hash1".to_string()));
            assert_eq!(loaded_mappings.get("1.0.1"), Some(&"hash2".to_string()));

            Ok(())
        })
    }

    #[test]
    fn test_input_parameter_with_filetypes() {
        let input = InputParameter {
            name: "input-raster".to_string(),
            param_type: "file".to_string(),
            required: Some(true),
            default: None,
            description: Some("Input raster file".to_string()),
            enum_values: None,
            readonly: Some(true),
            filetypes: Some(vec![".tif".to_string(), ".geotiff".to_string()]),
        };

        assert_eq!(input.name, "input-raster");
        assert_eq!(input.param_type, "file");
        assert_eq!(input.readonly, Some(true));
        assert!(input.filetypes.is_some());

        let filetypes = input.filetypes.unwrap();
        assert_eq!(filetypes.len(), 2);
        assert!(filetypes.contains(&".tif".to_string()));
    }

    #[test]
    fn test_input_parameter_enum_type() {
        let input = InputParameter {
            name: "format".to_string(),
            param_type: "enum".to_string(),
            required: Some(false),
            default: Some(serde_yaml::Value::String("tiff".to_string())),
            description: Some("Output format".to_string()),
            enum_values: Some(vec!["tiff".to_string(), "png".to_string(), "jpeg".to_string()]),
            readonly: None,
            filetypes: None,
        };

        assert_eq!(input.param_type, "enum");
        assert!(input.enum_values.is_some());

        let enum_vals = input.enum_values.unwrap();
        assert_eq!(enum_vals.len(), 3);
        assert!(enum_vals.contains(&"tiff".to_string()));
    }
}