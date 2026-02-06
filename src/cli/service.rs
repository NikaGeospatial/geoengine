use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use std::path::PathBuf;

use crate::config::settings::Settings;
use crate::proxy::server::ProxyServer;
use crate::utils::paths;

#[derive(Subcommand)]
pub enum ServiceCommands {
    /// Start the GeoEngine proxy service
    Start {
        /// HTTP port to listen on
        #[arg(short, long, default_value = "9876")]
        port: u16,

        /// Maximum number of concurrent containers
        #[arg(short, long, default_value = "4")]
        workers: usize,

        /// Run in foreground (default runs as daemon)
        #[arg(long)]
        foreground: bool,
    },

    /// Stop the GeoEngine proxy service
    Stop,

    /// Show proxy service status
    Status,

    /// View proxy service logs
    Logs {
        /// Number of lines to show
        #[arg(short, long, default_value = "50")]
        lines: usize,

        /// Follow log output
        #[arg(short, long)]
        follow: bool,
    },

    /// Register GeoEngine with a GIS application
    Register {
        /// GIS application to register with
        #[command(subcommand)]
        app: RegisterApp,
    },

    /// List running and queued jobs
    Jobs {
        /// Show all jobs including completed
        #[arg(short, long)]
        all: bool,
    },

    /// Cancel a running or queued job
    Cancel {
        /// Job ID to cancel
        job_id: String,
    },
}

#[derive(Subcommand)]
pub enum RegisterApp {
    /// Register with ArcGIS Pro
    Arcgis {
        /// Custom toolbox installation path
        #[arg(long)]
        path: Option<PathBuf>,
    },

    /// Register with QGIS
    Qgis {
        /// Custom plugin installation path
        #[arg(long)]
        path: Option<PathBuf>,
    },
}

impl ServiceCommands {
    pub async fn execute(self) -> Result<()> {
        match self {
            Self::Start {
                port,
                workers,
                foreground,
            } => start_service(port, workers, foreground).await,
            Self::Stop => stop_service().await,
            Self::Status => show_status().await,
            Self::Logs { lines, follow } => show_logs(lines, follow).await,
            Self::Register { app } => register_app(app).await,
            Self::Jobs { all } => list_jobs(all).await,
            Self::Cancel { job_id } => cancel_job(&job_id).await,
        }
    }
}

async fn start_service(port: u16, workers: usize, foreground: bool) -> Result<()> {
    let pid_file = paths::get_pid_file()?;
    let log_file = paths::get_log_file()?;

    // Check if already running
    if pid_file.exists() {
        if let Ok(pid_str) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                if is_process_running(pid) {
                    println!(
                        "{} Service is already running (PID: {})",
                        "!".yellow().bold(),
                        pid
                    );
                    println!("  Stop it first with: {}", "geoengine service stop".cyan());
                    return Ok(());
                }
            }
        }
        // Stale pid file, remove it
        std::fs::remove_file(&pid_file)?;
    }

    if foreground {
        println!(
            "{} Starting GeoEngine proxy service on port {}...",
            "=>".blue().bold(),
            port
        );
        println!("  Press Ctrl+C to stop\n");

        let server = ProxyServer::new(port, workers);
        server.run().await?;
    } else {
        // Fork to background
        println!(
            "{} Starting GeoEngine proxy service in background...",
            "=>".blue().bold()
        );

        let exe = std::env::current_exe()?;
        let child = std::process::Command::new(&exe)
            .args(["service", "start", "--port", &port.to_string(), "--workers", &workers.to_string(), "--foreground"])
            .stdout(std::fs::File::create(&log_file)?)
            .stderr(std::fs::File::create(&log_file)?)
            .spawn()
            .context("Failed to spawn background service")?;

        // Write PID file
        std::fs::write(&pid_file, child.id().to_string())?;

        println!(
            "{} Service started (PID: {})",
            "✓".green().bold(),
            child.id()
        );
        println!("  Listening on: http://localhost:{}", port);
        println!("  Logs: {}", log_file.display());
        println!("\n  Stop with: {}", "geoengine service stop".cyan());
    }

    Ok(())
}

async fn stop_service() -> Result<()> {
    let pid_file = paths::get_pid_file()?;

    if !pid_file.exists() {
        println!("{} Service is not running", "!".yellow().bold());
        return Ok(());
    }

    let pid_str = std::fs::read_to_string(&pid_file)?;
    let pid: u32 = pid_str
        .trim()
        .parse()
        .context("Invalid PID in pid file")?;

    println!("{} Stopping service (PID: {})...", "=>".blue().bold(), pid);

    // Send SIGTERM
    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        kill(Pid::from_raw(pid as i32), Signal::SIGTERM).ok();
    }

    #[cfg(windows)]
    {
        std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/F"])
            .output()
            .ok();
    }

    // Remove PID file
    std::fs::remove_file(&pid_file)?;

    println!("{} Service stopped", "✓".green().bold());

    Ok(())
}

async fn show_status() -> Result<()> {
    let pid_file = paths::get_pid_file()?;

    if !pid_file.exists() {
        println!("{} Service is {} running", "●".red(), "not".red().bold());
        return Ok(());
    }

    let pid_str = std::fs::read_to_string(&pid_file)?;
    let pid: u32 = pid_str.trim().parse().context("Invalid PID")?;

    if is_process_running(pid) {
        println!("{} Service is {}", "●".green(), "running".green().bold());
        println!("  PID: {}", pid);

        // Try to get port from settings
        let settings = Settings::load()?;
        if let Some(port) = settings.service_port {
            println!("  URL: http://localhost:{}", port);

            // Check health endpoint
            let health_url = format!("http://localhost:{}/api/health", port);
            match reqwest_health_check(&health_url).await {
                Ok(true) => println!("  Health: {}", "OK".green()),
                Ok(false) => println!("  Health: {}", "UNHEALTHY".red()),
                Err(_) => println!("  Health: {}", "UNKNOWN".yellow()),
            }
        }
    } else {
        println!("{} Service is {} (stale PID file)", "●".yellow(), "not running".yellow().bold());
        std::fs::remove_file(&pid_file)?;
    }

    Ok(())
}

async fn reqwest_health_check(url: &str) -> Result<bool> {
    // Simple TCP check since we don't have reqwest
    use std::net::TcpStream;
    use std::time::Duration;

    let addr = url
        .trim_start_matches("http://")
        .trim_end_matches("/api/health");

    match TcpStream::connect_timeout(
        &addr.parse().unwrap_or_else(|_| "127.0.0.1:9876".parse().unwrap()),
        Duration::from_secs(2),
    ) {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

async fn show_logs(lines: usize, follow: bool) -> Result<()> {
    let log_file = paths::get_log_file()?;

    if !log_file.exists() {
        println!("{} No log file found", "!".yellow().bold());
        return Ok(());
    }

    if follow {
        // Use tail -f
        let mut child = std::process::Command::new("tail")
            .args(["-f", "-n", &lines.to_string()])
            .arg(&log_file)
            .spawn()
            .context("Failed to run tail")?;

        child.wait()?;
    } else {
        let output = std::process::Command::new("tail")
            .args(["-n", &lines.to_string()])
            .arg(&log_file)
            .output()
            .context("Failed to read logs")?;

        println!("{}", String::from_utf8_lossy(&output.stdout));
    }

    Ok(())
}

async fn register_app(app: RegisterApp) -> Result<()> {
    match app {
        RegisterApp::Arcgis { path } => register_arcgis(path).await,
        RegisterApp::Qgis { path } => register_qgis(path).await,
    }
}

async fn register_arcgis(custom_path: Option<PathBuf>) -> Result<()> {
    println!(
        "{} Registering GeoEngine with ArcGIS Pro...",
        "=>".blue().bold()
    );

    // Find ArcGIS toolbox directory
    let toolbox_dir = if let Some(path) = custom_path {
        path
    } else {
        find_arcgis_toolbox_dir()?
    };

    // Ensure directory exists
    std::fs::create_dir_all(&toolbox_dir)?;

    // Copy toolbox files
    let exe_dir = std::env::current_exe()?
        .parent()
        .unwrap()
        .to_path_buf();

    let plugin_dir = exe_dir.join("plugins").join("arcgis");

    // If plugins not in exe dir, try relative to cargo manifest
    let plugin_dir = if plugin_dir.exists() {
        plugin_dir
    } else {
        // Embedded plugins - write them directly
        write_arcgis_plugin(&toolbox_dir)?;
        println!(
            "{} Installed GeoEngine toolbox to: {}",
            "✓".green().bold(),
            toolbox_dir.display()
        );
        println!("\nIn ArcGIS Pro:");
        println!("  1. Open the Catalog pane");
        println!("  2. Navigate to Toolboxes > My Toolboxes");
        println!("  3. You should see 'GeoEngine Tools'");
        return Ok(());
    };

    // Copy plugin files
    for entry in std::fs::read_dir(&plugin_dir)? {
        let entry = entry?;
        let dest = toolbox_dir.join(entry.file_name());
        std::fs::copy(entry.path(), &dest)?;
    }

    println!(
        "{} Installed GeoEngine toolbox to: {}",
        "✓".green().bold(),
        toolbox_dir.display()
    );

    Ok(())
}

async fn register_qgis(custom_path: Option<PathBuf>) -> Result<()> {
    println!(
        "{} Registering GeoEngine with QGIS...",
        "=>".blue().bold()
    );

    // Find QGIS plugin directory
    let plugin_dir = if let Some(path) = custom_path {
        path
    } else {
        find_qgis_plugin_dir()?
    };

    let geoengine_dir = plugin_dir.join("geoengine");
    std::fs::create_dir_all(&geoengine_dir)?;

    // Write plugin files
    write_qgis_plugin(&geoengine_dir)?;

    println!(
        "{} Installed GeoEngine plugin to: {}",
        "✓".green().bold(),
        geoengine_dir.display()
    );

    println!("\nIn QGIS:");
    println!("  1. Go to Plugins > Manage and Install Plugins");
    println!("  2. Find 'GeoEngine' in the Installed tab");
    println!("  3. Enable the plugin");
    println!("  4. Tools will appear in Processing Toolbox under 'GeoEngine'");

    Ok(())
}

fn find_arcgis_toolbox_dir() -> Result<PathBuf> {
    // Common ArcGIS Pro toolbox locations
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

    // Default to first candidate
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
    // Write the Python toolbox
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

async fn list_jobs(all: bool) -> Result<()> {
    let settings = Settings::load()?;
    let port = settings.service_port.unwrap_or(9876);

    println!("{} Fetching jobs from service...", "=>".blue().bold());

    // Make HTTP request to service
    let url = format!(
        "http://localhost:{}/api/jobs{}",
        port,
        if all { "?all=true" } else { "" }
    );

    let output = std::process::Command::new("curl")
        .args(["-s", &url])
        .output()
        .context("Failed to connect to service. Is it running?")?;

    if !output.status.success() {
        anyhow::bail!("Service not responding. Start it with: geoengine service start");
    }

    let response = String::from_utf8_lossy(&output.stdout);

    // Parse and display jobs
    if let Ok(jobs) = serde_json::from_str::<Vec<serde_json::Value>>(&response) {
        if jobs.is_empty() {
            println!("{}", "No jobs found".yellow());
            return Ok(());
        }

        println!(
            "{:<36} {:<15} {:<20} {}",
            "JOB ID".bold(),
            "STATUS".bold(),
            "PROJECT".bold(),
            "TOOL".bold()
        );
        println!("{}", "-".repeat(85));

        for job in jobs {
            let id = job["id"].as_str().unwrap_or("?");
            let status = job["status"].as_str().unwrap_or("unknown");
            let project = job["project"].as_str().unwrap_or("?");
            let tool = job["tool"].as_str().unwrap_or("?");

            let status_colored = match status {
                "running" => status.yellow().to_string(),
                "completed" => status.green().to_string(),
                "failed" => status.red().to_string(),
                "queued" => status.cyan().to_string(),
                _ => status.to_string(),
            };

            println!("{:<36} {:<15} {:<20} {}", id, status_colored, project, tool);
        }
    } else {
        println!("{}", response);
    }

    Ok(())
}

async fn cancel_job(job_id: &str) -> Result<()> {
    let settings = Settings::load()?;
    let port = settings.service_port.unwrap_or(9876);

    println!("{} Cancelling job {}...", "=>".blue().bold(), job_id);

    let url = format!("http://localhost:{}/api/jobs/{}", port, job_id);

    let output = std::process::Command::new("curl")
        .args(["-s", "-X", "DELETE", &url])
        .output()
        .context("Failed to connect to service")?;

    if output.status.success() {
        println!("{} Job cancelled", "✓".green().bold());
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to cancel job: {}", error);
    }

    Ok(())
}

fn is_process_running(pid: u32) -> bool {
    #[cfg(unix)]
    {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::Pid;
        kill(Pid::from_raw(pid as i32), Signal::SIGCONT).is_ok()
    }

    #[cfg(windows)]
    {
        use sysinfo::{ProcessRefreshKind, System};
        let mut sys = System::new();
        sys.refresh_processes_specifics(ProcessRefreshKind::new());
        sys.process(sysinfo::Pid::from(pid as usize)).is_some()
    }

    #[cfg(not(any(unix, windows)))]
    {
        false
    }
}
