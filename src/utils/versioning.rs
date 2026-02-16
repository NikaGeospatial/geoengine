use regex::Regex;
use semver::Version;
use std::cmp::Ordering;
use std::sync::OnceLock;
use crate::docker::client::DockerClient;

pub async fn get_latest_worker_version(worker_name: &str, client: &DockerClient) -> Option<String> {
    let images = client
        .list_images(Some(&format!("geoengine-local/{}", worker_name)), true)
        .await
        .ok()?;
    let prefix = format!("geoengine-local/{}:", worker_name);

    images
        .into_iter()
        .flat_map(|image| image.repo_tags.into_iter())
        .filter_map(|tag| {
            if !tag.starts_with(&prefix) {
                return None;
            }
            let (_, version_str) = tag.rsplit_once(':')?;
            let parsed = Version::parse(version_str).ok()?;
            Some((parsed, version_str.to_string()))
        })
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, version_str)| version_str)
}

pub async fn get_latest_worker_version_clientless(worker_name: &str) -> Option<String> {
    let client = DockerClient::new().await.ok()?;
    get_latest_worker_version(worker_name, &client).await
}

fn semver_regex() -> &'static Regex {
    static SEMVER_RE: OnceLock<Regex> = OnceLock::new();
    SEMVER_RE.get_or_init(|| {
        Regex::new(r"^\d+\.\d+\.\d+$").expect("SEMVER_RE regex must compile")
    })
}

pub fn validate_version(version: &str) -> Result<(), String> {
    if !semver_regex().is_match(version) {
        Err(format!("Invalid version '{}'. Version numbers should follow semantic versioning.", version))
    } else {
        Ok(())
    }
}

pub fn compare_versions(v1: &str, v2: &str) -> Result<Ordering, String> {
    validate_version(v1)?;
    validate_version(v2)?;
    let ver1 = match Version::parse(v1) {
        Ok(v) => v,
        Err(_) => return Err(format!("Invalid version '{}'. Please ensure your version number follows 'MAJOR.MINOR.PATCH'.", v1))
    };
    let ver2 = match Version::parse(v2) {
        Ok(v) => v,
        Err(_) => return Err(format!("Invalid version '{}'. Please ensure your version number follows 'MAJOR.MINOR.PATCH'.", v2))
    };
    Ok(ver1.cmp(&ver2))
}

/// Compare provided version with worker's built image version, throw an Error if version doesn't follow semantic versioning.
pub async fn compare_worker_version(worker_name: &str, version: &str, client: &DockerClient) -> Result<Ordering, String> {
    validate_version(version)?;
    let latest_version = get_latest_worker_version(worker_name, client).await;
    match latest_version {
        Some(latest) => {
            compare_versions(version, &latest)
        },
        None => Ok(Ordering::Greater)
    }
}
