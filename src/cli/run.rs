use anyhow::{Context, Result};
use clap::Args;
use colored::Colorize;
use std::collections::HashMap;
use std::path::PathBuf;

use crate::docker::client::DockerClient;
use crate::docker::gpu::GpuConfig;

#[derive(Args)]
pub struct RunArgs {
    /// Docker image to run
    pub image: String,

    /// Command to execute in the container
    #[arg(trailing_var_arg = true)]
    pub command: Vec<String>,

    /// Mount host directory to container (format: host:container or host:container:ro)
    #[arg(short, long, value_name = "HOST:CONTAINER")]
    pub mount: Vec<String>,

    /// Enable GPU passthrough (NVIDIA)
    #[arg(long)]
    pub gpu: bool,

    /// Set environment variables (format: KEY=VALUE)
    #[arg(short, long, value_name = "KEY=VALUE")]
    pub env: Vec<String>,

    /// Load environment variables from a file
    #[arg(long, value_name = "FILE")]
    pub env_file: Option<PathBuf>,

    /// Memory limit (e.g., 8g, 512m)
    #[arg(long)]
    pub memory: Option<String>,

    /// Number of CPUs to allocate
    #[arg(long)]
    pub cpus: Option<f64>,

    /// Shared memory size (e.g., 2g) - useful for PyTorch DataLoader
    #[arg(long)]
    pub shm_size: Option<String>,

    /// Working directory inside the container
    #[arg(short, long)]
    pub workdir: Option<String>,

    /// Run container in background (detached mode)
    #[arg(short, long)]
    pub detach: bool,

    /// Container name
    #[arg(long)]
    pub name: Option<String>,

    /// Remove container after exit
    #[arg(long)]
    pub rm: bool,

    /// Run as interactive TTY
    #[arg(short = 't', long)]
    pub tty: bool,
}

impl RunArgs {
    pub async fn execute(self) -> Result<()> {
        let client = DockerClient::new().await?;

        // Parse environment variables
        let mut env_vars = parse_env_vars(&self.env)?;

        // Load env file if specified
        if let Some(env_file) = &self.env_file {
            let file_vars = load_env_file(env_file)?;
            env_vars.extend(file_vars);
        }

        // Parse mount points
        let mounts = parse_mounts(&self.mount)?;

        // Configure GPU if requested
        let gpu_config = if self.gpu {
            Some(GpuConfig::detect().await?)
        } else {
            None
        };

        // Build container config
        let config = ContainerConfig {
            image: self.image.clone(),
            command: if self.command.is_empty() {
                None
            } else {
                Some(self.command.clone())
            },
            env_vars,
            mounts: mounts.clone(),
            gpu_config,
            memory: self.memory.clone(),
            cpus: self.cpus,
            shm_size: self.shm_size.clone(),
            workdir: self.workdir.clone(),
            name: self.name.clone(),
            remove_on_exit: self.rm,
            detach: self.detach,
            tty: self.tty,
        };

        if self.detach {
            println!(
                "{} Starting container in background...",
                "=>".blue().bold()
            );
            let container_id = client.run_container_detached(&config).await?;
            println!(
                "{} Container started: {}",
                "✓".green().bold(),
                container_id[..12].cyan()
            );
        } else {
            println!("{} Running container...", "=>".blue().bold());
            if self.gpu {
                println!("  {} GPU passthrough enabled", "•".yellow());
            }
            for (host, container, ro) in &mounts {
                let mode = if *ro { " (read-only)" } else { "" };
                println!("  {} Mount: {} -> {}{}", "•".yellow(), host, container, mode);
            }

            let exit_code = client.run_container_attached(&config).await?;

            if exit_code == 0 {
                println!("{} Container exited successfully", "✓".green().bold());
            } else {
                println!(
                    "{} Container exited with code {}",
                    "✗".red().bold(),
                    exit_code
                );
                std::process::exit(exit_code as i32);
            }
        }

        Ok(())
    }
}

pub struct ContainerConfig {
    pub image: String,
    pub command: Option<Vec<String>>,
    pub env_vars: HashMap<String, String>,
    pub mounts: Vec<(String, String, bool)>, // (host, container, readonly)
    pub gpu_config: Option<GpuConfig>,
    pub memory: Option<String>,
    pub cpus: Option<f64>,
    pub shm_size: Option<String>,
    pub workdir: Option<String>,
    pub name: Option<String>,
    pub remove_on_exit: bool,
    pub detach: bool,
    pub tty: bool,
}

fn parse_env_vars(env_args: &[String]) -> Result<HashMap<String, String>> {
    let mut vars = HashMap::new();

    for arg in env_args {
        let parts: Vec<&str> = arg.splitn(2, '=').collect();
        if parts.len() != 2 {
            anyhow::bail!("Invalid environment variable format: {}. Expected KEY=VALUE", arg);
        }
        vars.insert(parts[0].to_string(), parts[1].to_string());
    }

    Ok(vars)
}

fn parse_mounts(mount_args: &[String]) -> Result<Vec<(String, String, bool)>> {
    let mut mounts = Vec::new();

    for arg in mount_args {
        let parts: Vec<&str> = arg.split(':').collect();
        match parts.len() {
            2 => {
                let host = resolve_path(parts[0])?;
                mounts.push((host, parts[1].to_string(), false));
            }
            3 => {
                let host = resolve_path(parts[0])?;
                let readonly = parts[2] == "ro";
                mounts.push((host, parts[1].to_string(), readonly));
            }
            _ => {
                anyhow::bail!(
                    "Invalid mount format: {}. Expected host:container or host:container:ro",
                    arg
                );
            }
        }
    }

    Ok(mounts)
}

fn resolve_path(path: &str) -> Result<String> {
    let path = if path.starts_with("./") || path.starts_with("../") || !path.starts_with('/') {
        std::env::current_dir()
            .context("Failed to get current directory")?
            .join(path)
    } else {
        PathBuf::from(path)
    };

    path.canonicalize()
        .with_context(|| format!("Path does not exist: {}", path.display()))?
        .to_str()
        .map(|s| s.to_string())
        .context("Path contains invalid UTF-8")
}

fn load_env_file(path: &PathBuf) -> Result<HashMap<String, String>> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read env file: {}", path.display()))?;

    let mut vars = HashMap::new();

    for line in content.lines() {
        let line = line.trim();

        // Skip empty lines and comments
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let parts: Vec<&str> = line.splitn(2, '=').collect();
        if parts.len() == 2 {
            let key = parts[0].trim();
            let value = parts[1].trim().trim_matches('"').trim_matches('\'');
            vars.insert(key.to_string(), value.to_string());
        }
    }

    Ok(vars)
}
