use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};

use crate::docker::client::DockerClient;

#[derive(Subcommand)]
pub enum DeployCommands {
    /// Authenticate with GCP Artifact Registry
    Auth {
        /// GCP project ID
        #[arg(long, env = "GCP_PROJECT")]
        project: Option<String>,
    },

    /// Push an image to GCP Artifact Registry
    Push {
        /// Local image name/tag
        image: String,

        /// GCP project ID
        #[arg(long, env = "GCP_PROJECT")]
        project: String,

        /// GCP region (e.g., us-central1)
        #[arg(long, default_value = "us-central1")]
        region: String,

        /// Repository name in Artifact Registry
        #[arg(long, default_value = "geoengine")]
        repository: String,

        /// Remote image tag (defaults to local tag)
        #[arg(long)]
        tag: Option<String>,
    },

    /// Pull an image from GCP Artifact Registry
    Pull {
        /// Remote image name
        image: String,

        /// GCP project ID
        #[arg(long, env = "GCP_PROJECT")]
        project: String,

        /// GCP region
        #[arg(long, default_value = "us-central1")]
        region: String,

        /// Repository name
        #[arg(long, default_value = "geoengine")]
        repository: String,
    },

    /// List images in GCP Artifact Registry
    List {
        /// GCP project ID
        #[arg(long, env = "GCP_PROJECT")]
        project: String,

        /// GCP region
        #[arg(long, default_value = "us-central1")]
        region: String,

        /// Repository name
        #[arg(long, default_value = "geoengine")]
        repository: String,
    },
}

impl DeployCommands {
    pub async fn execute(self) -> Result<()> {
        match self {
            Self::Auth { project } => configure_auth(project.as_deref()).await,
            Self::Push {
                image,
                project,
                region,
                repository,
                tag,
            } => push_image(&image, &project, &region, &repository, tag.as_deref()).await,
            Self::Pull {
                image,
                project,
                region,
                repository,
            } => pull_image(&image, &project, &region, &repository).await,
            Self::List {
                project,
                region,
                repository,
            } => list_images(&project, &region, &repository).await,
        }
    }
}

async fn configure_auth(project: Option<&str>) -> Result<()> {
    println!("{} Configuring GCP authentication...", "=>".blue().bold());

    // Check if gcloud is installed
    which::which("gcloud").context(
        "gcloud CLI not found. Please install the Google Cloud SDK: https://cloud.google.com/sdk/docs/install",
    )?;

    // Run gcloud auth configure-docker
    let regions = [
        "us-central1",
        "us-east1",
        "us-west1",
        "europe-west1",
        "asia-east1",
    ];
    let registries: Vec<String> = regions
        .iter()
        .map(|r| format!("{}-docker.pkg.dev", r))
        .collect();

    let output = std::process::Command::new("gcloud")
        .args(["auth", "configure-docker", &registries.join(",")])
        .output()
        .context("Failed to run gcloud auth")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to configure Docker auth: {}", stderr);
    }

    println!(
        "{} Docker configured to use GCP Artifact Registry",
        "✓".green().bold()
    );

    // Optionally set default project
    if let Some(proj) = project {
        let output = std::process::Command::new("gcloud")
            .args(["config", "set", "project", proj])
            .output()
            .context("Failed to set GCP project")?;

        if output.status.success() {
            println!(
                "{} Default project set to: {}",
                "✓".green().bold(),
                proj.cyan()
            );
        }
    }

    println!("\nYou can now push images with:");
    println!(
        "  {}",
        "geoengine deploy push <image> --project <gcp-project>".cyan()
    );

    Ok(())
}

async fn push_image(
    image: &str,
    project: &str,
    region: &str,
    repository: &str,
    tag: Option<&str>,
) -> Result<()> {
    let client = DockerClient::new().await?;

    // Build the full GCP Artifact Registry path
    let remote_tag = tag.unwrap_or_else(|| image.split(':').last().unwrap_or("latest"));
    let image_name = image.split(':').next().unwrap_or(image);
    let remote_image = format!(
        "{}-docker.pkg.dev/{}/{}/{}:{}",
        region, project, repository, image_name, remote_tag
    );

    println!(
        "{} Pushing {} to {}...",
        "=>".blue().bold(),
        image.cyan(),
        remote_image.cyan()
    );

    // Tag the image
    client.tag_image(image, &remote_image).await?;

    // Push
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );
    pb.set_message("Pushing to Artifact Registry...");
    pb.enable_steady_tick(std::time::Duration::from_millis(100));

    client.push_image(&remote_image).await?;

    pb.finish_and_clear();
    println!(
        "{} Successfully pushed: {}",
        "✓".green().bold(),
        remote_image.cyan()
    );

    Ok(())
}

async fn pull_image(image: &str, project: &str, region: &str, repository: &str) -> Result<()> {
    let client = DockerClient::new().await?;

    let remote_image = format!(
        "{}-docker.pkg.dev/{}/{}/{}",
        region, project, repository, image
    );

    println!("{} Pulling {}...", "=>".blue().bold(), remote_image.cyan());

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );
    pb.set_message("Downloading from Artifact Registry...");
    pb.enable_steady_tick(std::time::Duration::from_millis(100));

    client.pull_image(&remote_image).await?;

    pb.finish_and_clear();
    println!(
        "{} Successfully pulled: {}",
        "✓".green().bold(),
        remote_image.cyan()
    );

    Ok(())
}

async fn list_images(project: &str, region: &str, repository: &str) -> Result<()> {
    println!(
        "{} Listing images in {}-docker.pkg.dev/{}/{}...",
        "=>".blue().bold(),
        region,
        project,
        repository
    );

    // Use gcloud to list images
    let output = std::process::Command::new("gcloud")
        .args([
            "artifacts",
            "docker",
            "images",
            "list",
            &format!("{}-docker.pkg.dev/{}/{}", region, project, repository),
            "--format=table(package,version,createTime)",
        ])
        .output()
        .context("Failed to run gcloud artifacts command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to list images: {}", stderr);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    println!("{}", stdout);

    Ok(())
}
