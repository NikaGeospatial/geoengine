use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;

use crate::config::worker::VersionConfigMaps;
use crate::config::yaml_store::{self, get_worker_saves_dir};
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
            Self::Import { tarfile, tag } => import_image(&client, &tarfile, tag.as_deref()).await,
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

    println!("{}{}", " ".repeat(38), "PUSHED IMAGES".bold());
    println!(
        "{:<41} {:<8} {:<20} {:<15} {}",
        "WORKER".bold(),
        "VERSION".bold(),
        "IMAGE ID".bold(),
        "SIZE".bold(),
        "CREATED".bold()
    );
    println!("{}", "-".repeat(100));

    for image in &images {
        let tags = image
            .repo_tags
            .iter()
            .filter(|t| t.starts_with("geoengine-local/"))
            .map(|s| {
                s.as_str()
                    .trim_start_matches("geoengine-local/")
                    .split_once(":")
                    .unwrap_or((s, "latest"))
            })
            .collect::<Vec<(&str, &str)>>();
        let id = short_image_id(&image.id);
        let size = format_size(image.size);
        let created = format_timestamp(image.created);

        for (worker, ver) in tags {
            println!(
                "{:<41} {:<8} {:<20} {:<15} {}",
                worker, ver, id, size, created
            );
        }
    }

    println!();
    println!("{}{}", " ".repeat(40), "DEV IMAGES".bold());
    println!(
        "{:<50} {:<20} {:<15} {}",
        "WORKER".bold(),
        "IMAGE ID".bold(),
        "SIZE".bold(),
        "CREATED".bold()
    );
    println!("{}", "-".repeat(100));

    for image in &images {
        let worker = image
            .repo_tags
            .iter()
            .filter(|t| t.starts_with("geoengine-local-dev/"))
            .map(|s| {
                s.as_str()
                    .trim_start_matches("geoengine-local-dev/")
                    .trim_end_matches(":latest")
            })
            .collect::<Vec<&str>>();
        let id = short_image_id(&image.id);
        let size = format_size(image.size);
        let created = format_timestamp(image.created);

        for tag in worker {
            println!("{:<50} {:<20} {:<15} {}", tag, id, size, created);
        }
    }

    Ok(())
}

async fn remove_image(client: &DockerClient, image: &str, force: bool) -> Result<()> {
    println!("{} Removing image {}...", "=>".blue().bold(), image.cyan());

    // Capture release tags before removal so metadata cleanup still works when
    // Docker no longer reports tags for the deleted image.
    let tags_to_clean = if image.starts_with("geoengine-local/") {
        vec![image.to_string()]
    } else {
        client
            .list_images(None, false)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter(|img| {
                img.id == image
                    || short_image_id(&img.id) == image
                    || img.repo_tags.iter().any(|tag| tag.contains(image))
            })
            .flat_map(|img| img.repo_tags.into_iter())
            .filter(|tag| tag.starts_with("geoengine-local/"))
            .collect::<Vec<_>>()
    };

    client
        .remove_image(image, force)
        .await
        .context("Failed to remove image")?;

    println!(
        "{} Successfully removed image: {}",
        "✓".green().bold(),
        image.cyan()
    );

    for tag in &tags_to_clean {
        if let Some(rest) = tag.strip_prefix("geoengine-local/") {
            if let Some((worker_name, version)) = rest.split_once(':') {
                if let Err(err) = remove_version_from_saves(worker_name, version) {
                    eprintln!(
                        "  {} Image was removed, but failed to clean saved version metadata for '{}': {}",
                        "!".yellow().bold(),
                        worker_name,
                        err
                    );
                }
            }
        }
    }

    Ok(())
}

fn remove_version_from_saves(worker_name: &str, version: &str) -> Result<()> {
    let saves_dir = get_worker_saves_dir(worker_name).with_context(|| {
        format!(
            "Failed to resolve saves directory for worker '{}'",
            worker_name
        )
    })?;

    // Acquire exclusive lock for the full load → modify → save cycle, using the
    // same lock key as cache_and_tag_config to prevent concurrent interleaving.
    let _lock = match yaml_store::lock_worker_saves_map(worker_name) {
        Ok(l) => l,
        Err(_) => {
            // If the saves directory doesn't exist yet there is nothing to remove.
            return Ok(());
        }
    };

    let mut map = match VersionConfigMaps::load_from_worker(worker_name) {
        Ok(m) => m,
        Err(err) if yaml_store::is_not_found_error(&err) => return Ok(()),
        Err(err) => {
            return Err(err).context(format!(
                "Failed to load saved version metadata for worker '{}'",
                worker_name
            ));
        }
    };

    let mut mappings = map.mappings.unwrap_or_default();

    // Get the hash before removing so we can check if it's still referenced
    let removed_hash = match mappings.remove(version) {
        Some(hash) => hash,
        None => return Ok(()),
    };
    map.mappings = if mappings.is_empty() {
        None
    } else {
        Some(mappings.clone())
    };

    map.save_to_worker(worker_name).with_context(|| {
        format!(
            "Failed to update saved version metadata for worker '{}'",
            worker_name
        )
    })?;

    println!(
        "  {} Removed version '{}' from saves map for worker '{}'",
        "✓".green().bold(),
        version,
        worker_name
    );

    // If the removed hash is no longer referenced by any other version, delete the snapshot
    let still_referenced = mappings.values().any(|h| h == &removed_hash);
    if !still_referenced {
        let snapshot = saves_dir.join(format!("{}.json", removed_hash));
        if snapshot.exists() {
            if let Err(e) = std::fs::remove_file(&snapshot) {
                eprintln!(
                    "  {} Failed to delete snapshot file: {}",
                    "!".yellow().bold(),
                    e
                );
            } else {
                let short_hash = &removed_hash[..12.min(removed_hash.len())];
                println!(
                    "  {} Deleted unreferenced config snapshot ({})",
                    "✓".green().bold(),
                    short_hash
                );
            }
        }
    }

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
