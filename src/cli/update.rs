use anyhow::{bail, Context, Result};
use colored::Colorize;
use reqwest::Response;
use sha2::{Digest, Sha256};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

pub async fn update_geoengine() -> Result<()> {
    // --- 1. Detect installation method ---
    let method = detect_install_method().await;
    println!(
        "{}",
        format!("Detected install method: {}", method.label()).cyan()
    );

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
                .status()
                .await
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

    println!(
        "{}",
        format!("==> Downloading {} @ {}", archive_name, tag).blue()
    );

    // Separate clients: tight total timeout for small metadata fetches; connection-only
    // timeout for archive downloads so large binaries on slow links don't time out.
    let meta_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let archive_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()?;

    let expected_hash = fetch_expected_checksum(&archive_name, &tag, &meta_client).await?;
    let (archive_path, actual_hash) =
        download_to_temp_file(&archive_url, &archive_client, "geoengine_archive", "tar.gz")
            .await
            .with_context(|| format!("Failed to download {}", archive_url))?;

    verify_checksum(&archive_name, &expected_hash, &actual_hash)?;

    // Extract the binary and install script from the verified archive.
    // The script is bundled in the archive so it inherits the same integrity guarantee
    // as the binary — no separate network fetch is needed.
    let archive_for_spawn = archive_path.clone();
    let extract_result =
        tokio::task::spawn_blocking(move || extract_from_tar(&archive_for_spawn))
            .await
            .context("Extraction task panicked")?;
    let _ = std::fs::remove_file(&archive_path);
    let (tmp_binary, tmp_script) = extract_result?;

    println!(
        "{}",
        format!(
            "==> bash {} --local {}",
            tmp_script.display(),
            tmp_binary.display()
        )
        .blue()
    );
    let status = Command::new("bash")
        .arg(tmp_script.as_os_str())
        .arg("--local")
        .arg(&tmp_binary)
        .status()
        .await
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp_binary);
            let _ = std::fs::remove_file(&tmp_script);
        })
        .context("Failed to run install.sh")?;

    let _ = std::fs::remove_file(&tmp_binary);
    let _ = std::fs::remove_file(&tmp_script);

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

    println!(
        "{}",
        format!("==> Downloading {} @ {}", archive_name, tag).blue()
    );

    // Separate clients: tight total timeout for small metadata fetches; connection-only
    // timeout for archive downloads so large binaries on slow links don't time out.
    let meta_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?;
    let archive_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()?;

    let expected_hash = fetch_expected_checksum(&archive_name, &tag, &meta_client).await?;
    let (archive_path, actual_hash) =
        download_to_temp_file(&archive_url, &archive_client, "geoengine_archive", "zip")
            .await
            .with_context(|| format!("Failed to download {}", archive_url))?;

    verify_checksum(&archive_name, &expected_hash, &actual_hash)?;

    // Extract the binary and install script from the verified archive.
    let archive_for_spawn = archive_path.clone();
    let extract_result =
        tokio::task::spawn_blocking(move || extract_from_zip(&archive_for_spawn))
            .await
            .context("Extraction task panicked")?;
    let _ = std::fs::remove_file(&archive_path);
    let (tmp_binary, tmp_script) = extract_result?;

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
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp_binary);
            let _ = std::fs::remove_file(&tmp_script);
        })
        .context("Failed to run install.ps1")?;

    let _ = std::fs::remove_file(&tmp_binary);
    let _ = std::fs::remove_file(&tmp_script);

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

/// Download `checksums.txt` for `tag` and return the expected SHA256 for `archive_name`.
///
/// # Trust model
///
/// Both the archive and `checksums.txt` are downloaded from the same GitHub
/// release over HTTPS. This detects accidental corruption in transit or
/// storage, but does **not** protect against a fully compromised release: an
/// attacker who can push a malicious binary to the release page can also
/// replace `checksums.txt` with matching hashes. The primary security
/// guarantee here is transport integrity via HTTPS; the checksum file provides
/// an additional layer for detecting bit-rot or partial downloads.
///
/// The install script is bundled inside the release archive rather than
/// fetched separately, so it inherits the same integrity check as the binary.
async fn fetch_expected_checksum(
    archive_name: &str,
    tag: &str,
    req_client: &reqwest::Client,
) -> Result<String> {
    let checksums_url = format!(
        "https://github.com/NikaGeospatial/geoengine/releases/download/{}/checksums.txt",
        tag
    );

    println!("{}", "==> Fetching checksums.txt...".blue());
    let checksums_text = fetch_bytes(&checksums_url, req_client)
        .await
        .context("Failed to download checksums.txt")?;
    let checksums_text =
        std::str::from_utf8(&checksums_text).context("checksums.txt is not valid UTF-8")?;

    // Each line is either "<sha256>  <filename>" (sha256sum text mode)
    // or "<sha256> *<filename>" (sha256sum binary mode). Split on the first
    // run of whitespace and strip a leading '*' from the filename field.
    let expected_hash = checksums_text
        .lines()
        .find_map(|line| {
            let mut parts = line.splitn(2, "  ");
            let (hash, name) = if let (Some(h), Some(n)) = (parts.next(), parts.next()) {
                // Two-space separator (text mode).
                (h.trim(), n.trim())
            } else {
                // Fall back to single-space separator (binary mode: "<hash> *<name>").
                let mut parts = line.splitn(2, ' ');
                let h = parts.next()?.trim();
                let n = parts.next()?.trim().trim_start_matches('*');
                (h, n)
            };
            if name == archive_name {
                Some(hash.to_owned())
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

    if expected_hash.len() != 64 || !expected_hash.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!(
            "checksums.txt appears malformed — expected a 64-character hex hash for '{}', got '{}'",
            archive_name,
            expected_hash
        );
    }

    Ok(expected_hash)
}

fn verify_checksum(archive_name: &str, expected_hash: &str, actual_hash: &str) -> Result<()> {
    if !actual_hash.eq_ignore_ascii_case(expected_hash) {
        bail!(
            "Checksum mismatch for {}!\n  expected: {}\n  actual:   {}",
            archive_name,
            expected_hash,
            actual_hash
        );
    }

    println!(
        "{}",
        format!("✓ Checksum verified (sha256: {})", actual_hash).green()
    );
    Ok(())
}

/// Extract the `geoengine` binary and `install.sh` script from a `.tar.gz` archive
/// in a single pass. Returns `(binary_path, script_path)`.
///
/// Intended to be called via `tokio::task::spawn_blocking` since it performs
/// blocking I/O.
fn extract_from_tar(
    archive_path: &std::path::Path,
) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let file = std::fs::File::open(archive_path)
        .with_context(|| format!("Failed to open archive {}", archive_path.display()))?;
    let gz = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(gz);

    let pid = std::process::id();
    let binary_out = std::env::temp_dir().join(format!("geoengine_{}_bin", pid));
    let script_out = std::env::temp_dir().join(format!("geoengine_{}_install.sh", pid));

    let mut binary_done = false;
    let mut script_done = false;

    for entry in archive.entries().context("Failed to read tar entries")? {
        let mut entry = entry.context("Failed to read tar entry")?;
        let name = {
            let p = entry.path().context("Failed to read entry path")?;
            p.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned()
        };

        match name.as_str() {
            "geoengine" if !binary_done => {
                let mut out = std::fs::File::create(&binary_out)
                    .with_context(|| format!("Failed to create {}", binary_out.display()))?;
                std::io::copy(&mut entry, &mut out).inspect_err(|_| {
                    let _ = std::fs::remove_file(&binary_out);
                })?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        &binary_out,
                        std::fs::Permissions::from_mode(0o755),
                    )?;
                }
                binary_done = true;
            }
            "install.sh" if !script_done => {
                let mut out = std::fs::File::create(&script_out)
                    .with_context(|| format!("Failed to create {}", script_out.display()))?;
                std::io::copy(&mut entry, &mut out).inspect_err(|_| {
                    let _ = std::fs::remove_file(&script_out);
                })?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        &script_out,
                        std::fs::Permissions::from_mode(0o700),
                    )?;
                }
                script_done = true;
            }
            _ => {}
        }

        if binary_done && script_done {
            break;
        }
    }

    if !binary_done {
        if script_done {
            let _ = std::fs::remove_file(&script_out);
        }
        bail!("Binary 'geoengine' not found inside tar.gz archive");
    }
    if !script_done {
        let _ = std::fs::remove_file(&binary_out);
        bail!("Script 'install.sh' not found inside tar.gz archive");
    }

    Ok((binary_out, script_out))
}

/// Extract `geoengine.exe` and `install.ps1` from a `.zip` archive.
/// Returns `(binary_path, script_path)`.
///
/// Intended to be called via `tokio::task::spawn_blocking` since it performs
/// blocking I/O.
fn extract_from_zip(
    archive_path: &std::path::Path,
) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let file = std::fs::File::open(archive_path)
        .with_context(|| format!("Failed to open archive {}", archive_path.display()))?;
    let mut zip = zip::ZipArchive::new(file).context("Failed to read zip archive")?;

    let pid = std::process::id();
    let binary_out = std::env::temp_dir().join(format!("geoengine_{}_bin.exe", pid));
    let script_out = std::env::temp_dir().join(format!("geoengine_{}_install.ps1", pid));

    let mut binary_done = false;
    let mut script_done = false;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).context("Failed to read zip entry")?;
        // Use file_name() so entries stored as `./geoengine.exe` or inside a
        // subdirectory are matched the same way as bare `geoengine.exe`.
        let name = std::path::Path::new(entry.name())
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();

        match name.as_str() {
            "geoengine.exe" if !binary_done => {
                let mut out = std::fs::File::create(&binary_out)
                    .with_context(|| format!("Failed to create {}", binary_out.display()))?;
                std::io::copy(&mut entry, &mut out)
                    .inspect_err(|_| {
                        let _ = std::fs::remove_file(&binary_out);
                    })
                    .context("Failed to extract binary from zip")?;
                binary_done = true;
            }
            "install.ps1" if !script_done => {
                let mut out = std::fs::File::create(&script_out)
                    .with_context(|| format!("Failed to create {}", script_out.display()))?;
                std::io::copy(&mut entry, &mut out)
                    .inspect_err(|_| {
                        let _ = std::fs::remove_file(&script_out);
                    })
                    .context("Failed to extract install.ps1 from zip")?;
                script_done = true;
            }
            _ => {}
        }

        if binary_done && script_done {
            break;
        }
    }

    if !binary_done {
        if script_done {
            let _ = std::fs::remove_file(&script_out);
        }
        bail!("Binary 'geoengine.exe' not found inside zip archive");
    }
    if !script_done {
        let _ = std::fs::remove_file(&binary_out);
        bail!("Script 'install.ps1' not found inside zip archive");
    }

    Ok((binary_out, script_out))
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
async fn fetch_bytes(url: &str, client: &reqwest::Client) -> Result<Vec<u8>> {
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

/// Download a URL to a temp file while computing SHA256. Returns (path, hash).
async fn download_to_temp_file(
    url: &str,
    client: &reqwest::Client,
    prefix: &str,
    ext: &str,
) -> Result<(std::path::PathBuf, String)> {
    let response = client
        .get(url)
        .header("User-Agent", APP_USER_AGENT)
        .send()
        .await
        .with_context(|| format!("GET {} failed", url))?
        .error_for_status()
        .with_context(|| format!("GET {} returned an error status", url))?;

    let (path, mut file) = create_temp_file(prefix, ext).await?;
    let mut hasher = Sha256::new();

    let download_result: Result<String> = async {
        stream_response_to_file(response, &path, &mut file, &mut hasher, url).await?;
        file.flush()
            .await
            .with_context(|| format!("Failed to flush {}", path.display()))?;
        Ok(format!("{:x}", hasher.finalize()))
    }
    .await;

    match download_result {
        Ok(hash) => Ok((path, hash)),
        Err(err) => {
            let _ = tokio::fs::remove_file(&path).await;
            Err(err)
        }
    }
}

async fn stream_response_to_file(
    mut response: Response,
    path: &std::path::Path,
    file: &mut tokio::fs::File,
    hasher: &mut Sha256,
    url: &str,
) -> Result<()> {
    while let Some(chunk) = response
        .chunk()
        .await
        .with_context(|| format!("Failed to read body from {}", url))?
    {
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .with_context(|| format!("Failed to write to {}", path.display()))?;
    }
    Ok(())
}

async fn create_temp_file(
    prefix: &str,
    ext: &str,
) -> Result<(std::path::PathBuf, tokio::fs::File)> {
    let dir = std::env::temp_dir();
    let pid = std::process::id();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    for attempt in 0..10u32 {
        let name = format!("{}_{}_{}_{}.{}", prefix, pid, now, attempt, ext);
        let path = dir.join(name);
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(file) => return Ok((path, file)),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("Failed to create temp file {}", path.display()))
            }
        }
    }

    bail!("Failed to create a unique temp file in {}", dir.display())
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
