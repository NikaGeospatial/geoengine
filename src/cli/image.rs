use anyhow::{Context, Result};
use clap::Subcommand;
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;

use crate::config::worker::VersionConfigMaps;
use crate::config::yaml_store::get_worker_saves_dir;
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

    client
        .remove_image(image, force)
        .await
        .context("Failed to remove image")?;

    println!(
        "{} Successfully removed image: {}",
        "✓".green().bold(),
        image.cyan()
    );

    // Clean up the version mapping from saves if this is a geoengine-local release image
    if let Some(rest) = image.strip_prefix("geoengine-local/") {
        if let Some((worker_name, version)) = rest.split_once(':') {
            remove_version_from_saves(worker_name, version);
        }
    }

    Ok(())
}

fn remove_version_from_saves(worker_name: &str, version: &str) {
    let saves_dir = match get_worker_saves_dir(worker_name) {
        Ok(d) => d,
        Err(_) => return,
    };

    let mut map = match VersionConfigMaps::load_from_worker(worker_name) {
        Ok(m) => m,
        Err(_) => return, // no saves for this worker; nothing to clean
    };

    let mut mappings = map.mappings.unwrap_or_default();

    // Get the hash before removing so we can check if it's still referenced
    let removed_hash = mappings.remove(version);
    map.mappings = if mappings.is_empty() {
        None
    } else {
        Some(mappings.clone())
    };

    if let Err(e) = map.save_to_worker(worker_name) {
        eprintln!("{} Failed to update saves map: {}", "!".yellow().bold(), e);
        return;
    }

    println!(
        "  {} Removed version '{}' from saves map for worker '{}'",
        "✓".green().bold(),
        version,
        worker_name
    );

    // If the removed hash is no longer referenced by any other version, delete the snapshot
    if let Some(hash) = removed_hash {
        let still_referenced = mappings.values().any(|h| h == &hash);
        if !still_referenced {
            let snapshot = saves_dir.join(format!("{}.json", hash));
            if snapshot.exists() {
                if let Err(e) = std::fs::remove_file(&snapshot) {
                    eprintln!(
                        "  {} Failed to delete snapshot file: {}",
                        "!".yellow().bold(),
                        e
                    );
                } else {
                    let short_hash = &hash[..12.min(hash.len())];
                    println!(
                        "  {} Deleted unreferenced config snapshot ({})",
                        "✓".green().bold(),
                        short_hash
                    );
                }
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_size_bytes() {
        assert_eq!(format_size(500), "500 B");
    }

    #[test]
    fn test_format_size_kilobytes() {
        assert_eq!(format_size(1024), "1.00 KB");
        assert_eq!(format_size(2048), "2.00 KB");
        assert_eq!(format_size(1536), "1.50 KB");
    }

    #[test]
    fn test_format_size_megabytes() {
        assert_eq!(format_size(1024 * 1024), "1.00 MB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.00 MB");
        assert_eq!(format_size(1536 * 1024), "1.50 MB");
    }

    #[test]
    fn test_format_size_gigabytes() {
        assert_eq!(format_size(1024 * 1024 * 1024), "1.00 GB");
        assert_eq!(format_size(2 * 1024 * 1024 * 1024), "2.00 GB");
        assert_eq!(format_size(3584 * 1024 * 1024), "3.50 GB");
    }

    #[test]
    fn test_format_size_zero() {
        assert_eq!(format_size(0), "0 B");
    }

    #[test]
    fn test_format_timestamp() {
        // Test a known timestamp: 2024-01-01 00:00:00 UTC
        let timestamp = 1704067200;
        let formatted = format_timestamp(timestamp);
        assert_eq!(formatted, "2024-01-01 00:00");
    }

    #[test]
    fn test_format_timestamp_recent() {
        // Test a more recent timestamp: 2025-01-15 12:30:00 UTC
        let timestamp = 1736944200;
        let formatted = format_timestamp(timestamp);
        assert_eq!(formatted, "2025-01-15 12:30");
    }

    #[test]
    fn test_format_timestamp_invalid() {
        // Test with invalid timestamp
        let formatted = format_timestamp(-1);
        assert_eq!(formatted, "Unknown");
    }

    #[test]
    fn test_short_image_id_with_prefix() {
        let id = "sha256:abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
        let short = short_image_id(id);
        assert_eq!(short, "abcdef123456");
        assert_eq!(short.len(), 12);
    }

    #[test]
    fn test_short_image_id_without_prefix() {
        let id = "abcdef1234567890abcdef1234567890";
        let short = short_image_id(id);
        assert_eq!(short, "abcdef123456");
        assert_eq!(short.len(), 12);
    }

    #[test]
    fn test_short_image_id_short_input() {
        let id = "abc123";
        let short = short_image_id(id);
        assert_eq!(short, "abc123");
    }

    #[test]
    fn test_short_image_id_empty() {
        let id = "";
        let short = short_image_id(id);
        assert_eq!(short, "<none>");
    }

    #[test]
    fn test_short_image_id_only_prefix() {
        let id = "sha256:";
        let short = short_image_id(id);
        assert_eq!(short, "<none>");
    }

    #[test]
    fn test_remove_version_from_saves_basic() {
        // This is a unit test that verifies the logic without Docker/filesystem side effects
        // The function itself modifies the filesystem, so we test it indirectly
        let worker_name = "test-worker";
        let version = "1.0.0";

        // The function should handle gracefully when worker doesn't exist
        remove_version_from_saves(worker_name, version);
        // If it doesn't panic, the test passes
    }

    #[test]
    fn test_format_size_boundary_values() {
        // Test boundary between KB and MB
        let kb_boundary = 1024 * 1024 - 1;
        let result = format_size(kb_boundary);
        assert!(result.contains("KB"));

        let mb_boundary = 1024 * 1024;
        let result = format_size(mb_boundary);
        assert!(result.contains("MB"));
    }

    #[test]
    fn test_format_size_boundary_gb() {
        // Test boundary between MB and GB
        let mb_boundary = 1024 * 1024 * 1024 - 1;
        let result = format_size(mb_boundary);
        assert!(result.contains("MB"));

        let gb_boundary = 1024 * 1024 * 1024;
        let result = format_size(gb_boundary);
        assert!(result.contains("GB"));
    }

    #[test]
    fn test_short_image_id_exactly_12_chars() {
        let id = "123456789012";
        let short = short_image_id(id);
        assert_eq!(short, "123456789012");
        assert_eq!(short.len(), 12);
    }

    #[test]
    fn test_short_image_id_unicode() {
        // Test with unicode characters
        let id = "🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥🔥extra";
        let short = short_image_id(id);
        // Takes first 12 unicode characters
        assert_eq!(short.chars().count(), 12);
    }
}