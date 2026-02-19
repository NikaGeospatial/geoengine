pub mod deploy;
pub mod image;
pub mod plugins;
pub mod worker;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "geoengine")]
#[command(author = "GeoEngine Team")]
#[command(version)]
#[command(about = "Docker-based isolated runtime manager for geospatial workloads", long_about = None)]
#[command(propagate_version = true)]
pub struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Enable verbose output
    #[arg(short, long, global = true)]
    verbose: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Manage Docker images (import, list, remove)
    Image {
        #[command(subcommand)]
        command: image::ImageCommands,
    },

    /// Initialize a new worker (creates geoengine.yaml)
    Init {
        /// Worker name, if not specified, uses current directory name.
        #[arg(short, long)]
        name: Option<String>,
    },

    /// Build the Docker image for a worker
    Build {
        /// Don't use cache when building
        #[arg(long)]
        no_cache: bool,

        /// Build the worker in the dev version
        #[arg(long)]
        dev: bool,

        /// Build arguments (format: KEY=VALUE)
        #[arg(long, value_name = "KEY=VALUE")]
        build_arg: Vec<String>,
    },

    /// Apply worker configuration: register if new, update plugins
    Apply {
        /// Worker name (or path to worker directory). Defaults to current directory.
        worker: Option<String>,
    },

    /// Delete a worker from GeoEngine
    Delete {
        /// Worker name to delete. If not provided, uses current directory's worker.
        #[arg(short, long)]
        name: Option<String>,
    },

    /// Run a worker's command with input parameters
    Run {
        /// Worker name (defaults to current directory's worker)
        worker: Option<String>,

        /// Input parameters (format: KEY=VALUE, repeatable)
        #[arg(short, long = "input", value_name = "KEY=VALUE")]
        inputs: Vec<String>,

        /// Emit structured JSON result to stdout (logs go to stderr)
        #[arg(long)]
        json: bool,
        
        /// Runs the latest dev version of the worker
        #[arg(long)]
        dev: bool,

        /// Extra arguments passed through to the container command
        #[arg(last = true)]
        args: Vec<String>,
    },

    /// List all registered workers
    Workers {
        /// Output as JSON (for programmatic use)
        #[arg(long)]
        json: bool,

        /// List only workers registered in the GIS plugin (takes in only "qgis" or "arcgis")
        #[arg(long)]
        gis: Option<String>,
    },
    
    /// Describe a specific worker
    Describe {
        /// Worker name (defaults to current directory's worker)
        worker: Option<String>,
        
        /// Output as JSON (for programmatic use)
        #[arg(long)]
        json: bool,
    },

    /// Check for differences in files, or a specific file
    Diff {
        /// Specify one of the following:
        /// - "all" to check all files in the worker directory
        /// - "config" to check only the geoengine.yaml file
        /// - "dockerfile" to check only the Dockerfile
        /// - "worker" to check only the worker directory itself
        #[arg(short, long)]
        file: Option<String>,
    },

    /// Deploy images to GCP Artifact Registry
    Deploy {
        #[command(subcommand)]
        command: deploy::DeployCommands,
    },

    /// Debug helper: install the QGIS plugin only if not already installed
    DebugQgis,
}

impl Cli {
    pub async fn execute(self) -> Result<()> {
        match self.command {
            Commands::Image { command } => command.execute().await,
            Commands::Init { name } => {
                worker::init_worker(name.as_deref()).await
            }
            Commands::Build {
                no_cache,
                dev,
                build_arg,
            } => worker::build_worker_local(no_cache, dev, &build_arg).await,
            Commands::Apply { worker } => {
                worker::apply_worker(worker.as_deref(), false).await
            }
            Commands::Delete { name } => worker::delete_worker(name.as_deref()).await,
            Commands::Run {
                worker,
                inputs,
                json,
                dev,
                args,
            } => worker::run_worker(worker.as_deref(), &inputs, json, dev, &args).await,
            Commands::Workers { json, gis } => worker::list_workers(json, gis).await,
            Commands::Describe { worker, json } => worker::describe_worker(worker.as_deref(), json).await,
            Commands::Diff { file } => worker::diff_worker(file.as_deref()).await,
            Commands::Deploy { command } => command.execute().await,
            Commands::DebugQgis => plugins::debug_qgis().await,
        }
    }
}
