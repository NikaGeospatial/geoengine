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

    // Determine the platform-specific archive name.
    let platform = current_platform()?;
    let archive_name = format!("geoengine-{}.tar.gz", platform);
    let archive_url = format!(
        "https://github.com/NikaGeospatial/geoengine/releases/download/{}/{}",
        tag, archive_name
    );

    println!("{}", format!("==> Downloading {} @ {}", archive_name, tag).blue());
    let archive_bytes = fetch_bytes(&archive_url)
        .await
        .with_context(|| format!("Failed to download {}", archive_url))?;

    verify_checksum(&archive_bytes, &archive_name, &tag).await?;

    // Write archive to a temp file and hand it to the install script via --local
    // so the script handles sudo / PATH setup without re-downloading.
    let tmp_archive = write_temp_file(&archive_bytes, "geoengine", "tar.gz")?;

    // Extract the binary from the archive into a sibling temp file.
    let tmp_binary = extract_binary_from_tar(&tmp_archive)?;

    let script_url = format!(
        "https://raw.githubusercontent.com/NikaGeospatial/geoengine/{}/install/install.sh",
        tag
    );
    println!("{}", format!("==> Downloading install.sh @ {}", tag).blue());
    let script_bytes = fetch_bytes(&script_url)
        .await
        .context("Failed to download install.sh")?;

    let tmp_script = write_temp_script(&script_bytes, "install", "sh")?;
    println!(
        "{}",
        format!("==> bash {} --local {}", tmp_script.display(), tmp_binary.display()).blue()
    );
    let status = Command::new("bash")
        .arg(tmp_script.as_os_str())
        .arg("--local")
        .arg(&tmp_binary)
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

    let platform = current_platform()?;
    let archive_name = format!("geoengine-{}.zip", platform);
    let archive_url = format!(
        "https://github.com/NikaGeospatial/geoengine/releases/download/{}/{}",
        tag, archive_name
    );

    println!("{}", format!("==> Downloading {} @ {}", archive_name, tag).blue());
    let archive_bytes = fetch_bytes(&archive_url)
        .await
        .with_context(|| format!("Failed to download {}", archive_url))?;

    verify_checksum(&archive_bytes, &archive_name, &tag).await?;

    let tmp_archive = write_temp_file(&archive_bytes, "geoengine", "zip")?;

    // Extract geoengine.exe from the zip into a temp directory.
    let tmp_binary = extract_binary_from_zip(&tmp_archive)?;

    let script_url = format!(
        "https://raw.githubusercontent.com/NikaGeospatial/geoengine/{}/install/install.ps1",
        tag
    );
    println!("{}", format!("==> Downloading install.ps1 @ {}", tag).blue());
    let script_bytes = fetch_bytes(&script_url)
        .await
        .context("Failed to download install.ps1")?;

    let tmp_script = write_temp_script(&script_bytes, "install", "ps1")?;
    println!(
        "{}",
        format!(
            "==> powershell {} -LocalBinary {}",
            tmp_script.display(),
            tmp_binary.display()
        )
        .blue()
    );
    let status = Command::new("powershell")
        .args([
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            tmp_script
                .to_str()
                .context("Temp path contains non-UTF-8 characters")?,
            "-LocalBinary",
            tmp_binary
                .to_str()
                .context("Temp binary path contains non-UTF-8 characters")?,
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

/// Return the platform string that matches the release artifact naming convention.
fn current_platform() -> Result<&'static str> {
    // cfg! values are resolved at compile time.
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return Ok("linux-x86_64");
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return Ok("linux-aarch64");
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Ok("darwin-x86_64");
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Ok("darwin-aarch64");
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return Ok("windows-x86_64");
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    return Ok("windows-aarch64");
    #[allow(unreachable_code)]
    {
        bail!("Unsupported platform — cannot determine release artifact name")
    }
}

/// Download `checksums.txt` for `tag`, find the line for `archive_name`,
/// and verify that the SHA256 of `data` matches.
async fn verify_checksum(data: &[u8], archive_name: &str, tag: &str) -> Result<()> {
    let checksums_url = format!(
        "https://github.com/NikaGeospatial/geoengine/releases/download/{}/checksums.txt",
        tag
    );

    println!("{}", "==> Fetching checksums.txt...".blue());
    let checksums_text = fetch_bytes(&checksums_url)
        .await
        .context("Failed to download checksums.txt")?;
    let checksums_text =
        std::str::from_utf8(&checksums_text).context("checksums.txt is not valid UTF-8")?;

    // Each line: "<sha256>  <filename>"
    let expected_hash = checksums_text
        .lines()
        .find_map(|line| {
            let mut parts = line.splitn(2, "  ");
            let hash = parts.next()?;
            let name = parts.next()?.trim();
            if name == archive_name {
                Some(hash.trim().to_owned())
            } else {
                None
            }
        })
        .with_context(|| {
            format!(
                "No checksum entry found for '{}' in checksums.txt",
                archive_name
            )
        })?;

    let actual_hash = format!("{:x}", Sha256::digest(data));

    if actual_hash != expected_hash {
        bail!(
            "Checksum mismatch for {}!\n  expected: {}\n  actual:   {}",
            archive_name,
            expected_hash,
            actual_hash
        );
    }

    println!("{}", format!("✓ Checksum verified ({})", &actual_hash[..16]).green());
    Ok(())
}

/// Write `data` to a temporary file and return the path.
fn write_temp_file(data: &[u8], prefix: &str, ext: &str) -> Result<std::path::PathBuf> {
    let dir = std::env::temp_dir();
    let name = format!("{}_{}.{}", prefix, std::process::id(), ext);
    let path = dir.join(name);
    std::fs::write(&path, data)
        .with_context(|| format!("Failed to write temp file to {}", path.display()))?;
    Ok(path)
}

/// Extract the `geoengine` binary from a `.tar.gz` archive at `archive_path`
/// and write it to a sibling temp file. Returns the path to the binary.
fn extract_binary_from_tar(archive_path: &std::path::Path) -> Result<std::path::PathBuf> {
    use std::io::Read;

    let file = std::fs::File::open(archive_path)
        .with_context(|| format!("Failed to open archive {}", archive_path.display()))?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    let out_path = std::env::temp_dir().join(format!("geoengine_{}_bin", std::process::id()));

    for entry in archive.entries().context("Failed to read tar entries")? {
        let mut entry = entry.context("Failed to read tar entry")?;
        let path = entry.path().context("Failed to read entry path")?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name == "geoengine" {
            let mut buf = Vec::new();
            entry.read_to_end(&mut buf).context("Failed to read binary from tar")?;
            std::fs::write(&out_path, &buf)
                .with_context(|| format!("Failed to write binary to {}", out_path.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(0o755))?;
            }
            return Ok(out_path);
        }
    }

    bail!("Binary 'geoengine' not found inside archive {}", archive_path.display())
}

/// Extract `geoengine.exe` from a `.zip` archive at `archive_path`
/// and write it to a sibling temp file. Returns the path to the binary.
fn extract_binary_from_zip(archive_path: &std::path::Path) -> Result<std::path::PathBuf> {
    let file = std::fs::File::open(archive_path)
        .with_context(|| format!("Failed to open archive {}", archive_path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("Failed to read zip archive")?;

    let out_path =
        std::env::temp_dir().join(format!("geoengine_{}_bin.exe", std::process::id()));

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).context("Failed to read zip entry")?;
        if entry.name() == "geoengine.exe" {
            let mut buf = Vec::new();
            std::io::copy(&mut entry, &mut buf).context("Failed to read binary from zip")?;
            std::fs::write(&out_path, &buf)
                .with_context(|| format!("Failed to write binary to {}", out_path.display()))?;
            return Ok(out_path);
        }
    }

    bail!("Binary 'geoengine.exe' not found inside archive {}", archive_path.display())
}

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
