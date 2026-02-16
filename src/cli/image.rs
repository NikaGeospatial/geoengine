use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;

use crate::docker::client::DockerClient;

#[derive(Subcommand)]
pub enum ImageCommands {
    /// Import a Docker image from a tar file (for air-gapped environments)
    Import {
        /// Path to the tar file containing the Docker image
        tarfile: PathBuf,

        /// Tag to apply to the imported image
        #[arg(short, long)]
        tag: Option<String>,
    },

    /// List all Docker images under geoengine
    List {
        /// Filter by image name
        #[arg(short, long)]
        filter: Option<String>,

        /// Show all images including intermediate layers
        #[arg(short, long)]
        all: bool,
    },

    /// Remove a Docker image
    Remove {
        /// Image name, ID, or tag to remove
        image: String,

        /// Force removal even if containers are using the image
        #[arg(short, long)]
        force: bool,
    },
}

impl ImageCommands {
    pub async fn execute(self) -> Result<()> {
        let client = DockerClient::new().await?;

        match self {
            Self::Import { tarfile, tag } => {
                import_image(&client, &tarfile, tag.as_deref()).await
            }
            Self::List { filter, all } => list_images(&client, filter.as_deref(), all).await,
            Self::Remove { image, force } => remove_image(&client, &image, force).await,
        }
    }
}

async fn import_image(client: &DockerClient, tarfile: &PathBuf, tag: Option<&str>) -> Result<()> {
    println!(
        "{} Importing image from {}...",
        "=>".blue().bold(),
        tarfile.display()
    );

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );
    pb.set_message("Loading image...");
    pb.enable_steady_tick(std::time::Duration::from_millis(100));

    let image_id = client
        .import_image(tarfile, tag)
        .await
        .context("Failed to import image")?;

    pb.finish_and_clear();
    println!(
        "{} Successfully imported image: {}",
        "✓".green().bold(),
        image_id.cyan()
    );

    Ok(())
}

async fn list_images(client: &DockerClient, filter: Option<&str>, all: bool) -> Result<()> {
    let images = client
        .list_images(filter, all)
        .await
        .context("Failed to list images")?;

    if images.is_empty() {
        println!("{}", "No images found".yellow());
        return Ok(());
    }

    println!("{}{}",
        " ".repeat(38),
        "PUSHED IMAGES".bold()
    );
    println!(
        "{:<50} {:<20} {:<15} {}",
        "REPOSITORY:TAG".bold(),
        "IMAGE ID".bold(),
        "SIZE".bold(),
        "CREATED".bold()
    );
    println!("{}", "-".repeat(100));

    for image in &images {
        let repo_tag = image
            .repo_tags
            .iter()
            .filter(|t| !t.starts_with("geoengine-local-dev/"))
            .map(|s| s.as_str())
            .collect::<Vec<&str>>();
        let id = short_image_id(&image.id);
        let size = format_size(image.size);
        let created = format_timestamp(image.created);

        for tag in repo_tag {
            println!("{:<50} {:<20} {:<15} {}", tag, id, size, created);
        }
    }

    println!();
    println!("{}{}",
         " ".repeat(40),
         "DEV IMAGES".bold()
    );
    println!(
        "{:<50} {:<20} {:<15} {}",
        "REPOSITORY:TAG".bold(),
        "IMAGE ID".bold(),
        "SIZE".bold(),
        "CREATED".bold()
    );
    println!("{}", "-".repeat(100));

    for image in &images {
        let repo_tag = image
            .repo_tags
            .iter()
            .filter(|t| t.starts_with("geoengine-local-dev/"))
            .map(|s| s.as_str())
            .collect::<Vec<&str>>();
        let id = short_image_id(&image.id);
        let size = format_size(image.size);
        let created = format_timestamp(image.created);

        for tag in repo_tag {
            println!("{:<50} {:<20} {:<15} {}", tag, id, size, created);
        }
    }

    Ok(())
}

async fn remove_image(client: &DockerClient, image: &str, force: bool) -> Result<()> {
    println!("{} Removing image {}...", "=>".blue().bold(), image.cyan());

    client
        .remove_image(image, force)
        .await
        .context("Failed to remove image")?;

    println!(
        "{} Successfully removed image: {}",
        "✓".green().bold(),
        image.cyan()
    );

    Ok(())
}

fn format_size(bytes: i64) -> String {
    const KB: i64 = 1024;
    const MB: i64 = KB * 1024;
    const GB: i64 = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_timestamp(timestamp: i64) -> String {
    use chrono::{DateTime, Utc};
    let dt = DateTime::<Utc>::from_timestamp(timestamp, 0);
    dt.map(|d| d.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "Unknown".to_string())
}

fn short_image_id(image_id: &str) -> String {
    let trimmed = image_id.strip_prefix("sha256:").unwrap_or(image_id);
    let short: String = trimmed.chars().take(12).collect();
    if short.is_empty() {
        "<none>".to_string()
    } else {
        short
    }
}
