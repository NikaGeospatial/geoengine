use anyhow::{Context, Result};
use std::process::Command;

/// GPU configuration for container execution
#[derive(Debug, Clone)]
pub struct GpuConfig {
    /// Type of GPU detected
    pub gpu_type: GpuType,

    /// Number of GPUs available
    pub count: usize,

    /// GPU device names
    pub devices: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GpuType {
    /// NVIDIA GPU with CUDA support
    Nvidia,
    /// Apple Metal (macOS)
    Metal,
    /// No GPU available
    None,
}

impl GpuConfig {
    /// Detect available GPUs on the system
    pub async fn detect() -> Result<Self> {
        // Try NVIDIA first (Linux/Windows/WSL2)
        if let Ok(config) = detect_nvidia().await {
            return Ok(config);
        }

        // Try Metal on macOS
        #[cfg(target_os = "macos")]
        if let Ok(config) = detect_metal().await {
            return Ok(config);
        }

        // No GPU found
        Ok(GpuConfig {
            gpu_type: GpuType::None,
            count: 0,
            devices: vec![],
        })
    }

    /// Check if GPU is available
    pub fn is_available(&self) -> bool {
        self.gpu_type != GpuType::None && self.count > 0
    }

    /// Check if this is an NVIDIA GPU (supports CUDA in Docker)
    pub fn is_nvidia(&self) -> bool {
        self.gpu_type == GpuType::Nvidia
    }
}

/// Detect NVIDIA GPUs using nvidia-smi
async fn detect_nvidia() -> Result<GpuConfig> {
    // Check if nvidia-smi is available
    which::which("nvidia-smi").context("nvidia-smi not found")?;

    // Run nvidia-smi to get GPU info
    let output = Command::new("nvidia-smi")
        .args(["--query-gpu=name,uuid", "--format=csv,noheader"])
        .output()
        .context("Failed to run nvidia-smi")?;

    if !output.status.success() {
        anyhow::bail!("nvidia-smi failed");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let devices: Vec<String> = stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            line.split(',')
                .next()
                .unwrap_or("Unknown GPU")
                .trim()
                .to_string()
        })
        .collect();

    if devices.is_empty() {
        anyhow::bail!("No NVIDIA GPUs found");
    }

    // Verify NVIDIA Container Toolkit is installed
    verify_nvidia_docker()?;

    Ok(GpuConfig {
        gpu_type: GpuType::Nvidia,
        count: devices.len(),
        devices,
    })
}

/// Verify NVIDIA Container Toolkit is properly configured
fn verify_nvidia_docker() -> Result<()> {
    // Check for nvidia-container-toolkit or nvidia-docker
    let has_toolkit =
        which::which("nvidia-container-cli").is_ok() || which::which("nvidia-docker").is_ok();

    if !has_toolkit {
        // Check if the Docker runtime is configured
        let output = Command::new("docker")
            .args(["info", "--format", "{{.Runtimes}}"])
            .output();

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            if !stdout.contains("nvidia") {
                tracing::warn!(
                    "NVIDIA Container Toolkit may not be installed. \
                    GPU passthrough might not work. \
                    See: https://docs.nvidia.com/datacenter/cloud-native/container-toolkit/install-guide.html"
                );
            }
        }
    }

    Ok(())
}

/// Detect Metal GPU on macOS
#[cfg(target_os = "macos")]
async fn detect_metal() -> Result<GpuConfig> {
    // Use system_profiler to check for Metal-capable GPU
    let output = Command::new("system_profiler")
        .args(["SPDisplaysDataType", "-json"])
        .output()
        .context("Failed to run system_profiler")?;

    if !output.status.success() {
        anyhow::bail!("system_profiler failed");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse JSON to find GPU info
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&stdout) {
        if let Some(displays) = json.get("SPDisplaysDataType").and_then(|d| d.as_array()) {
            let mut devices = Vec::new();

            for display in displays {
                if let Some(name) = display.get("sppci_model").and_then(|n| n.as_str()) {
                    // Check if Metal is supported
                    let metal_support = display
                        .get("spdisplays_metal")
                        .and_then(|m| m.as_str())
                        .map(|s| s.contains("Supported"))
                        .unwrap_or(false);

                    if metal_support {
                        devices.push(name.to_string());
                    }
                }
            }

            if !devices.is_empty() {
                return Ok(GpuConfig {
                    gpu_type: GpuType::Metal,
                    count: devices.len(),
                    devices,
                });
            }
        }
    }

    anyhow::bail!("No Metal-capable GPU found")
}

/// Print GPU information
pub async fn print_gpu_info() -> Result<()> {
    let config = GpuConfig::detect().await?;

    match config.gpu_type {
        GpuType::Nvidia => {
            println!("GPU Type: NVIDIA (CUDA)");
            println!("GPU Count: {}", config.count);
            println!("Devices:");
            for (i, device) in config.devices.iter().enumerate() {
                println!("  [{}] {}", i, device);
            }
        }
        GpuType::Metal => {
            println!("GPU Type: Apple Metal");
            println!("Note: CUDA is not available on macOS. PyTorch will use MPS backend.");
            println!("Devices:");
            for device in &config.devices {
                println!("  - {}", device);
            }
        }
        GpuType::None => {
            println!("No GPU detected");
        }
    }

    Ok(())
}
