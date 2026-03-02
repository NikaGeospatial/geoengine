use colored::Colorize;
use serde::Deserialize;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const GITHUB_REPO: &str = "NikaGeospatial/geoengine";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

pub async fn check_for_update() {
    let url = format!(
        "https://api.github.com/repos/{}/releases/latest",
        GITHUB_REPO
    );

    let client = reqwest::Client::new();

    let result = client
        .get(&url)
        .header("User-Agent", "my-cli-app") // GitHub requires this
        .send();

    match result.await {
        Ok(response) => {
            if let Ok(release) = response.json::<Release>().await {
                let latest = release.tag_name.trim_start_matches('v');
                let current = CURRENT_VERSION.trim_start_matches('v');

                if latest != current {
                    println!(
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
            }
        }
        Err(_) => {
            // Silently skip — no internet? no problem.
        }
    }
}