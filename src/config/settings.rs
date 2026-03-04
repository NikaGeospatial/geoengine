use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

use crate::utils::paths;

/// Global GeoEngine settings stored in ~/.geoengine/settings.yaml
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Settings {
    /// Registered workers (name -> path)
    #[serde(default, alias = "projects")]
    pub workers: HashMap<String, PathBuf>,

    /// Default GCP project ID
    pub gcp_project: Option<String>,

    /// Default GCP region
    pub gcp_region: Option<String>,

    /// Global environment variables
    pub env: Option<HashMap<String, String>>,
}

impl Settings {
    /// Load settings from disk, creating default if not exists
    pub fn load() -> Result<Self> {
        let settings_path = paths::get_settings_file()?;

        if !settings_path.exists() {
            let settings = Self::default();
            settings.save()?;
            return Ok(settings);
        }

        let content = std::fs::read_to_string(&settings_path)
            .with_context(|| format!("Failed to read settings: {}", settings_path.display()))?;

        let settings: Settings = serde_yaml::from_str(&content)
            .with_context(|| "Failed to parse settings file")?;

        Ok(settings)
    }

    /// Save settings to disk
    pub fn save(&self) -> Result<()> {
        let settings_path = paths::get_settings_file()?;

        // Ensure directory exists
        if let Some(parent) = settings_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_yaml::to_string(self)?;
        std::fs::write(&settings_path, content)?;

        Ok(())
    }

    /// Register a new worker
    pub fn register_worker(&mut self, name: &str, path: &PathBuf) -> Result<()> {
        self.workers.insert(name.to_string(), path.clone());
        Ok(())
    }

    /// Find a worker whose registered path matches the given directory.
    /// Returns (name, path) if found.
    pub fn find_worker_by_path(&self, dir: &std::path::Path) -> Option<(String, PathBuf)> {
        let canonical = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
        for (name, path) in &self.workers {
            let registered = path.canonicalize().unwrap_or_else(|_| path.clone());
            if registered == canonical {
                return Some((name.clone(), registered));
            }
        }
        None
    }

    /// Unregister a worker
    pub fn unregister_worker(&mut self, name: &str) -> Result<()> {
        if self.workers.remove(name).is_none() {
            anyhow::bail!("Worker '{}' is not registered", name);
        }
        Ok(())
    }

    /// Get the path of a registered worker
    pub fn get_worker_path(&self, name: &str) -> Result<PathBuf> {
        // First check if it's a registered worker name
        if let Some(path) = self.workers.get(name) {
            return Ok(path.clone());
        }

        // Check if it's a path
        let path = PathBuf::from(name);
        if path.exists() && path.join("geoengine.yaml").exists() {
            return Ok(path.canonicalize()?);
        }

        anyhow::bail!(
            "Worker '{}' not found. Run 'geoengine apply' to register it.",
            name
        )
    }

    /// List all registered workers
    pub fn list_workers(&self) -> Vec<(&str, &PathBuf)> {
        self.workers
            .iter()
            .map(|(k, v)| (k.as_str(), v))
            .collect()
    }

    /// Set an environment variable
    pub fn set_env(&mut self, key: &str, value: &str) -> Result<()> {
        // Create the env map if it doesn't exist
        // If exists, remove the key first
        if self.env.is_none() {
            self.env = Some(HashMap::new());
        } else {
            let find = self.env.as_mut().unwrap().get(key);
            if find.is_some() {
                self.remove_env(key)?;
            }
        }
        // Add the new key-value pair
        self.env.as_mut().unwrap().insert(key.to_string(), value.to_string());
        Ok(())
    }

    /// Remove an environment variable
    pub fn remove_env(&mut self, key: &str) -> Result<()> {
        // Bail if the key doesn't exist
        if self.env.is_none() || self.env.as_ref().unwrap().get(key).is_none() {
            return Err(anyhow::anyhow!("Environment variable '{}' not found", key));
        }
        self.env.as_mut().unwrap().remove(key);
        if self.env.as_ref().unwrap().is_empty() {
            self.env = None;
        }
        Ok(())
    }

    /// List all environment variables
    pub fn list_env(&self) -> Option<HashMap<String, String>> {
        self.env.clone()
    }

    /// Get the value of an environment variable
    pub fn get_env(&self, key: &str) -> Option<&str> {
        self.env.as_ref().and_then(|env| env.get(key).map(|v| v.as_str()))
    }
}
