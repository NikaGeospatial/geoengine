use anyhow::{bail, Context, Result};
use colored::Colorize;
use sha2::{Digest, Sha256};
use std::process::Stdio;
use tokio::process::Command;

pub async fn update_geoengine() -> Result<()> {
    // --- 1. Detect installation method ---
    let method = detect_install_method().await;
    println!("{}", format!("Detected install method: {}", method.label()).cyan());

    // --- 2. Run the update ---
    match method {
        InstallMethod::Homebrew => update_via_homebrew().await?,
        InstallMethod::Shell => update_via_shell().await?,
        InstallMethod::PowerShell => update_via_powershell().await?,
    }

    // --- 3. Re-exec the newly installed binary to run the patch step ---
    println!("\n{}", "Running geoengine patch...".cyan());
    let exe = std::env::current_exe().context("Failed to determine current executable path")?;
    let status = Command::new(&exe)
        .arg("patch")
        .status()
        .await
        .with_context(|| format!("Failed to spawn `{} patch`", exe.display()))?;
    if !status.success() {
        bail!("`geoengine patch` exited with non-zero status — update aborted");
    }

    Ok(())
}

enum InstallMethod {
    Homebrew,
    Shell,
    PowerShell,
}

impl InstallMethod {
    fn label(&self) -> &str {
        match self {
            Self::Homebrew => "Homebrew",
            Self::Shell => "install.sh (curl)",
            Self::PowerShell => "install.ps1 (PowerShell)",
        }
    }
}

async fn detect_install_method() -> InstallMethod {
    if cfg!(target_os = "macos") {
        // macOS: prefer Homebrew if `brew` exists AND `brew list --formula geoengine` succeeds
        if which::which("brew").is_ok()
            && Command::new("brew")
                .args(["list", "--formula", "geoengine"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status().await
                .map(|s| s.success())
                .unwrap_or(false)
        {
            return InstallMethod::Homebrew;
        }
        return InstallMethod::Shell;
    }

    if cfg!(target_os = "windows") {
        return InstallMethod::PowerShell;
    }

    InstallMethod::Shell
}

async fn update_via_homebrew() -> Result<()> {
    println!("{}", "==> brew update".blue());
    run_command("brew", &["update"])
        .await
        .context("brew update failed")?;

    println!("{}", "==> brew upgrade geoengine".blue());
    run_command("brew", &["upgrade", "geoengine"])
        .await
        .context("brew upgrade geoengine failed")?;

    println!("{}", "✓ GeoEngine updated via Homebrew".green());
    Ok(())
}

async fn update_via_shell() -> Result<()> {
    let tag = latest_release_tag().await?;
    let script_url = format!(
        "https://raw.githubusercontent.com/NikaGeospatial/geoengine/{}/install/install.sh",
        tag
    );
    let checksum_url = format!(
        "https://raw.githubusercontent.com/NikaGeospatial/geoengine/{}/install/install.sh.sha256",
        tag
    );

    println!("{}", format!("==> Downloading install.sh @ {}", tag).blue());
    let script_bytes = fetch_bytes(&script_url)
        .await
        .context("Failed to download install.sh")?;

    println!("{}", "==> Verifying checksum".blue());
    let expected = fetch_text(&checksum_url)
        .await
        .context("Failed to download install.sh.sha256")?;
    verify_sha256(&script_bytes, expected.trim())
        .context("Checksum verification failed for install.sh")?;

    let tmp = write_temp_script(&script_bytes, "install", "sh")?;
    println!("{}", format!("==> bash {}", tmp.display()).blue());
    let status = Command::new("bash")
        .arg(tmp.as_os_str())
        .status()
        .await
        .context("Failed to run install.sh")?;

    if !status.success() {
        bail!("install.sh exited with non-zero status — update aborted");
    }

    println!("{}", "✓ GeoEngine updated via install.sh".green());
    Ok(())
}

async fn update_via_powershell() -> Result<()> {
    let tag = latest_release_tag().await?;
    let script_url = format!(
        "https://raw.githubusercontent.com/NikaGeospatial/geoengine/{}/install/install.ps1",
        tag
    );
    let checksum_url = format!(
        "https://raw.githubusercontent.com/NikaGeospatial/geoengine/{}/install/install.ps1.sha256",
        tag
    );

    println!("{}", format!("==> Downloading install.ps1 @ {}", tag).blue());
    let script_bytes = fetch_bytes(&script_url)
        .await
        .context("Failed to download install.ps1")?;

    println!("{}", "==> Verifying checksum".blue());
    let expected = fetch_text(&checksum_url)
        .await
        .context("Failed to download install.ps1.sha256")?;
    verify_sha256(&script_bytes, expected.trim())
        .context("Checksum verification failed for install.ps1")?;

    let tmp = write_temp_script(&script_bytes, "install", "ps1")?;
    println!("{}", format!("==> powershell {}", tmp.display()).blue());
    let status = Command::new("powershell")
        .args([
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            tmp.to_str()
                .context("Temp path contains non-UTF-8 characters")?,
        ])
        .status()
        .await
        .context("Failed to run install.ps1")?;

    if !status.success() {
        bail!("install.ps1 exited with non-zero status — update aborted");
    }

    println!("{}", "✓ GeoEngine updated via install.ps1".green());
    Ok(())
}

// ---------- helpers ----------

const GITHUB_API_LATEST: &str =
    "https://api.github.com/repos/NikaGeospatial/geoengine/releases/latest";
const APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));

/// Return the tag name of the latest GitHub release (e.g. `"v0.4.3"`).
async fn latest_release_tag() -> Result<String> {
    #[derive(serde::Deserialize)]
    struct Release {
        tag_name: String,
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()?;

    let release: Release = client
        .get(GITHUB_API_LATEST)
        .header("User-Agent", APP_USER_AGENT)
        .send()
        .await
        .context("Failed to reach GitHub API")?
        .error_for_status()
        .context("GitHub API returned an error")?
        .json()
        .await
        .context("Failed to parse GitHub release JSON")?;

    Ok(release.tag_name)
}

/// Download a URL and return the raw bytes.
async fn fetch_bytes(url: &str) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;
    let bytes = client
        .get(url)
        .header("User-Agent", APP_USER_AGENT)
        .send()
        .await
        .with_context(|| format!("GET {} failed", url))?
        .error_for_status()
        .with_context(|| format!("GET {} returned an error status", url))?
        .bytes()
        .await
        .with_context(|| format!("Failed to read body from {}", url))?;
    Ok(bytes.to_vec())
}

/// Download a URL and return the body as a UTF-8 string.
async fn fetch_text(url: &str) -> Result<String> {
    let bytes = fetch_bytes(url).await?;
    String::from_utf8(bytes).context("Response body is not valid UTF-8")
}

/// Verify that `data` hashes to `expected_hex` (lowercase hex SHA-256).
fn verify_sha256(data: &[u8], expected_hex: &str) -> Result<()> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let actual = format!("{:x}", hasher.finalize());
    // Accept "sha256:<hex>" or bare "<hex>" in the checksum file.
    let expected = expected_hex
        .strip_prefix("sha256:")
        .unwrap_or(expected_hex);
    if actual != expected {
        bail!(
            "SHA-256 mismatch — expected {}, got {}",
            expected,
            actual
        );
    }
    Ok(())
}

/// Write `data` to a temporary file and return the path.
/// The caller is responsible for cleanup; the file persists until the OS
/// removes it (typically on reboot) or the process exits on most platforms.
fn write_temp_script(data: &[u8], prefix: &str, ext: &str) -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir();
    let name = format!("{}_{}.{}", prefix, std::process::id(), ext);
    let path = dir.join(name);
    std::fs::write(&path, data)
        .with_context(|| format!("Failed to write temp script to {}", path.display()))?;
    // Make executable on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("Failed to chmod {}", path.display()))?;
    }
    Ok(path)
}

/// Run a command, streaming its stdout/stderr to the terminal.
async fn run_command(program: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(program)
        .args(args)
        .status()
        .await
        .with_context(|| format!("Failed to execute `{}`", program))?;

    if !status.success() {
        bail!(
            "`{} {}` exited with non-zero status",
            program,
            args.join(" ")
        );
    }
    Ok(())
}
