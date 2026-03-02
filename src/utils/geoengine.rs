use anyhow::Result;
use colored::Colorize;
use semver::Version;
use serde::Deserialize;
use std::time::Duration;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_REPO: &str = "NikaGeospatial/geoengine";
const APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

pub async fn check_for_update() -> Result<()> {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()?;

    let response = client
        .get(&url)
        .header("User-Agent", APP_USER_AGENT) // GitHub requires this
        .send()
        .await?
        .error_for_status()?;

    let release = response.json::<Release>().await?;
    let latest = release.tag_name.trim_start_matches('v');
    let current = CURRENT_VERSION.trim_start_matches('v');

    let latest_version = match Version::parse(latest) {
        Ok(version) => version,
        Err(error) => {
            tracing::debug!(
                "Skipping update check due to invalid release tag '{}': {}",
                release.tag_name,
                error
            );
            return Ok(());
        }
    };
    let current_version = match Version::parse(current) {
        Ok(version) => version,
        Err(error) => {
            tracing::debug!(
                "Skipping update check due to invalid current version '{}': {}",
                CURRENT_VERSION,
                error
            );
            return Ok(());
        }
    };

    if latest_version > current_version {
        eprintln!(
            "\n{}{}{}{}{}{}\n{}\n",
            "⚡ Update available: ".yellow().italic(),
            "v".yellow().bold().italic(),
            current.yellow().bold().italic(),
            " → ".yellow().italic(),
            "v".yellow().bold().italic(),
            latest.yellow().bold().italic(),
            "   Please update GeoEngine to the latest version!".yellow().italic()
        );
    }

    Ok(())
}
