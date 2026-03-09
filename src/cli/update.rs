use anyhow::{bail, Context, Result};
use colored::Colorize;
use indicatif::{ProgressBar, ProgressStyle};
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

#[derive(Clone, Copy)]
enum ScriptUpdateKind {
    Shell,
    PowerShell,
}

impl ScriptUpdateKind {
    fn archive_ext(self) -> &'static str {
        match self {
            Self::Shell => "tar.gz",
            Self::PowerShell => "zip",
        }
    }

    fn script_name(self) -> &'static str {
        match self {
            Self::Shell => "install.sh",
            Self::PowerShell => "install.ps1",
        }
    }

    fn success_message(self) -> &'static str {
        match self {
            Self::Shell => "✓ GeoEngine updated via install.sh",
            Self::PowerShell => "✓ GeoEngine updated via install.ps1",
        }
    }

    fn extractor(self) -> fn(&std::path::Path) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
        match self {
            Self::Shell => extract_from_tar,
            Self::PowerShell => extract_from_zip,
        }
    }
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
    update_via_script(ScriptUpdateKind::Shell).await
}

async fn update_via_powershell() -> Result<()> {
    update_via_script(ScriptUpdateKind::PowerShell).await
}

async fn update_via_script(kind: ScriptUpdateKind) -> Result<()> {
    // Separate clients: tight total timeout for small metadata fetches; connection-only
    // timeout for archive downloads so large binaries on slow links don't time out.
    let meta_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(
            METADATA_REQUEST_TIMEOUT_SECS,
        ))
        .build()?;
    let archive_client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(ARCHIVE_CONNECT_TIMEOUT_SECS))
        .build()?;

    let tag = latest_release_tag(&meta_client).await?;

    let platform = current_platform()?;
    let archive_name = format!("geoengine-{}.{}", platform, kind.archive_ext());
    let archive_url = format!(
        "https://github.com/NikaGeospatial/geoengine/releases/download/{}/{}",
        tag, archive_name
    );

    println!(
        "{}",
        format!("==> Downloading {} @ {}", archive_name, tag).blue()
    );

    let expected_hash = fetch_expected_checksum(&archive_name, &tag, &meta_client).await?;
    let (archive_path, actual_hash) = download_to_temp_file(
        &archive_url,
        &archive_client,
        "geoengine_archive",
        kind.archive_ext(),
    )
    .await
    .with_context(|| format!("Failed to download {}", archive_url))?;

    if let Err(e) = verify_checksum(&archive_name, &expected_hash, &actual_hash) {
        let _ = std::fs::remove_file(&archive_path);
        return Err(e);
    };

    // Extract the binary and install script from the verified archive.
    let archive_for_spawn = archive_path.clone();
    let extract = kind.extractor();
    let extract_result = tokio::task::spawn_blocking(move || extract(&archive_for_spawn))
        .await
        .context("Extraction task panicked")?;
    let _ = std::fs::remove_file(&archive_path);
    let (tmp_binary, tmp_script) = extract_result?;

    let status = run_installer(kind, &tmp_script, &tmp_binary)
        .await
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp_binary);
            let _ = std::fs::remove_file(&tmp_script);
        })
        .with_context(|| format!("Failed to run {}", kind.script_name()))?;

    let _ = std::fs::remove_file(&tmp_binary);
    let _ = std::fs::remove_file(&tmp_script);

    if !status.success() {
        bail!(
            "{} exited with non-zero status — update aborted",
            kind.script_name()
        );
    }

    println!("{}", kind.success_message().green());
    Ok(())
}

async fn run_installer(
    kind: ScriptUpdateKind,
    script_path: &std::path::Path,
    binary_path: &std::path::Path,
) -> Result<std::process::ExitStatus> {
    match kind {
        ScriptUpdateKind::Shell => {
            // `install/install.sh`'s `--local <path>` mode is the shell self-update
            // contract. The release archive bundles that script with the binary so
            // the updater and installer stay in lockstep across releases.
            println!(
                "{}",
                format!(
                    "==> bash {} --local {}",
                    script_path.display(),
                    binary_path.display()
                )
                .blue()
            );
            let status = Command::new("bash")
                .arg(script_path.as_os_str())
                .arg("--local")
                .arg(binary_path)
                .status()
                .await?;
            Ok(status)
        }
        ScriptUpdateKind::PowerShell => {
            println!(
                "{}",
                format!(
                    "==> powershell {} -LocalBinary {}",
                    script_path.display(),
                    binary_path.display()
                )
                .blue()
            );
            let status = Command::new("powershell")
                .args([
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    script_path
                        .to_str()
                        .context("Temp path contains non-UTF-8 characters")?,
                    "-LocalBinary",
                    binary_path
                        .to_str()
                        .context("Temp binary path contains non-UTF-8 characters")?,
                ])
                .status()
                .await?;
            Ok(status)
        }
    }
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

    expected_checksum_from_text(checksums_text, archive_name)
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

fn expected_checksum_from_text(checksums_text: &str, archive_name: &str) -> Result<String> {
    let expected_hash = checksums_text
        .lines()
        .find_map(|line| {
            let (hash, name) = parse_checksum_line(line)?;
            (name == archive_name).then(|| hash.to_owned())
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

fn parse_checksum_line(line: &str) -> Option<(&str, &str)> {
    // sha256sum emits either "<hash>  <name>" (text mode) or "<hash> *<name>"
    // (binary mode). Split once on the first run of whitespace and normalize
    // away the optional binary marker.
    let line = line.trim();
    let separator_idx = line.find(|c: char| c.is_whitespace())?;
    let (hash, remainder) = line.split_at(separator_idx);
    let name = remainder.trim_start();
    let stripped_name = name.strip_prefix('*').unwrap_or(name);

    if hash.is_empty() || stripped_name.is_empty() {
        return None;
    }

    Some((hash, stripped_name))
}

fn temp_file_name_stem(prefix: &str) -> String {
    let pid = std::process::id();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{}_{}_{}", prefix, pid, now)
}

fn temp_file_candidate_path(
    dir: &std::path::Path,
    stem: &str,
    ext: Option<&str>,
    attempt: u32,
) -> std::path::PathBuf {
    let name = match ext {
        Some(ext) => format!("{}_{}.{}", stem, attempt, ext),
        None => format!("{}_{}", stem, attempt),
    };
    dir.join(name)
}

fn create_temp_file_blocking(
    prefix: &str,
    ext: Option<&str>,
) -> Result<(std::path::PathBuf, std::fs::File)> {
    let dir = std::env::temp_dir();
    let stem = temp_file_name_stem(prefix);

    for attempt in 0..10u32 {
        let path = temp_file_candidate_path(&dir, &stem, ext, attempt);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
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

fn cleanup_temp_pair(first: &std::path::Path, second: &std::path::Path) {
    let _ = std::fs::remove_file(first);
    let _ = std::fs::remove_file(second);
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

    let (binary_out, mut binary_file) = create_temp_file_blocking("geoengine_bin", None)?;
    let (script_out, mut script_file) = create_temp_file_blocking("geoengine_install", Some("sh"))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&binary_out);
        })?;

    let mut binary_done = false;
    let mut script_done = false;

    let entries = archive.entries().inspect_err(|_| {
        cleanup_temp_pair(&binary_out, &script_out);
    })?;

    for entry in entries {
        let mut entry = entry.inspect_err(|_| {
            cleanup_temp_pair(&binary_out, &script_out);
        })?;
        let name = {
            let p = entry.path().inspect_err(|_| {
                cleanup_temp_pair(&binary_out, &script_out);
            })?;
            p.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_owned()
        };

        match name.as_str() {
            "geoengine" if !binary_done => {
                std::io::copy(&mut entry, &mut binary_file).inspect_err(|_| {
                    cleanup_temp_pair(&binary_out, &script_out);
                })?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&binary_out, std::fs::Permissions::from_mode(0o755))
                        .inspect_err(|_| {
                        cleanup_temp_pair(&binary_out, &script_out);
                    })?;
                }
                binary_done = true;
            }
            "install.sh" if !script_done => {
                std::io::copy(&mut entry, &mut script_file).inspect_err(|_| {
                    cleanup_temp_pair(&binary_out, &script_out);
                })?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&script_out, std::fs::Permissions::from_mode(0o700))
                        .inspect_err(|_| {
                        cleanup_temp_pair(&binary_out, &script_out);
                    })?;
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
        cleanup_temp_pair(&binary_out, &script_out);
        bail!("Binary 'geoengine' not found inside tar.gz archive");
    }
    if !script_done {
        cleanup_temp_pair(&binary_out, &script_out);
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

    let (binary_out, mut binary_file) = create_temp_file_blocking("geoengine_bin", Some("exe"))?;
    let (script_out, mut script_file) = create_temp_file_blocking("geoengine_install", Some("ps1"))
        .inspect_err(|_| {
            let _ = std::fs::remove_file(&binary_out);
        })?;

    let mut binary_done = false;
    let mut script_done = false;

    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).inspect_err(|_| {
            cleanup_temp_pair(&binary_out, &script_out);
        })?;
        // Use file_name() so entries stored as `./geoengine.exe` or inside a
        // subdirectory are matched the same way as bare `geoengine.exe`.
        let name = std::path::Path::new(entry.name())
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();

        match name.as_str() {
            "geoengine.exe" if !binary_done => {
                std::io::copy(&mut entry, &mut binary_file)
                    .inspect_err(|_| {
                        cleanup_temp_pair(&binary_out, &script_out);
                    })
                    .context("Failed to extract binary from zip")?;
                binary_done = true;
            }
            "install.ps1" if !script_done => {
                std::io::copy(&mut entry, &mut script_file)
                    .inspect_err(|_| {
                        cleanup_temp_pair(&binary_out, &script_out);
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
        cleanup_temp_pair(&binary_out, &script_out);
        bail!("Binary 'geoengine.exe' not found inside zip archive");
    }
    if !script_done {
        cleanup_temp_pair(&binary_out, &script_out);
        bail!("Script 'install.ps1' not found inside zip archive");
    }

    Ok((binary_out, script_out))
}

const GITHUB_API_LATEST: &str =
    "https://api.github.com/repos/NikaGeospatial/geoengine/releases/latest";
const APP_USER_AGENT: &str = concat!(env!("CARGO_PKG_NAME"), "/", env!("CARGO_PKG_VERSION"));
const METADATA_REQUEST_TIMEOUT_SECS: u64 = 30;
const ARCHIVE_CONNECT_TIMEOUT_SECS: u64 = 30;
const ARCHIVE_IDLE_TIMEOUT_SECS: u64 = 60;

/// Return the tag name of the latest GitHub release (e.g. `"v0.4.3"`).
async fn latest_release_tag(client: &reqwest::Client) -> Result<String> {
    #[derive(serde::Deserialize)]
    struct Release {
        tag_name: String,
    }

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
    let progress = create_archive_download_progress(response.content_length())?;

    let (path, mut file) = create_temp_file(prefix, ext).await?;
    let mut hasher = Sha256::new();

    let download_result: Result<String> = async {
        stream_response_to_file(response, &path, &mut file, &mut hasher, &progress, url).await?;
        file.flush()
            .await
            .with_context(|| format!("Failed to flush {}", path.display()))?;
        progress.finish_and_clear();
        Ok(format!("{:x}", hasher.finalize()))
    }
    .await;

    match download_result {
        Ok(hash) => Ok((path, hash)),
        Err(err) => {
            progress.finish_and_clear();
            let _ = tokio::fs::remove_file(&path).await;
            Err(err)
        }
    }
}

fn create_archive_download_progress(total_bytes: Option<u64>) -> Result<ProgressBar> {
    match total_bytes {
        Some(total) => {
            let pb = ProgressBar::new(total);
            pb.set_style(
                ProgressStyle::with_template(
                    "{spinner:.green} [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({bytes_per_sec}, eta {eta})",
                )
                .context("Failed to configure archive download progress bar style")?
                .progress_chars("#>-"),
            );
            Ok(pb)
        }
        None => {
            let pb = ProgressBar::new_spinner();
            pb.set_style(
                ProgressStyle::with_template(
                    "{spinner:.green} Downloading archive {bytes} ({bytes_per_sec})",
                )
                .context("Failed to configure archive download spinner style")?,
            );
            pb.enable_steady_tick(std::time::Duration::from_millis(100));
            Ok(pb)
        }
    }
}

/// Streams an HTTP response body to a file, while computing its SHA-256 hash.
async fn stream_response_to_file(
    mut response: Response,
    path: &std::path::Path,
    file: &mut tokio::fs::File,
    hasher: &mut Sha256,
    progress: &ProgressBar,
    url: &str,
) -> Result<()> {
    while let Some(chunk) = tokio::time::timeout(
        std::time::Duration::from_secs(ARCHIVE_IDLE_TIMEOUT_SECS),
        response.chunk(),
    )
    .await
    .with_context(|| {
        format!(
            "Timed out waiting for archive data from {} after {}s",
            url, ARCHIVE_IDLE_TIMEOUT_SECS
        )
    })?
    .with_context(|| format!("Failed to read body from {}", url))?
    {
        hasher.update(&chunk);
        progress.inc(chunk.len() as u64);
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
    let stem = temp_file_name_stem(prefix);

    for attempt in 0..10u32 {
        let path = temp_file_candidate_path(&dir, &stem, Some(ext), attempt);
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

#[cfg(test)]
mod tests {
    use super::{expected_checksum_from_text, parse_checksum_line, temp_file_candidate_path};

    const HASH_A: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
    const HASH_B: &str = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";

    #[test]
    fn parse_checksum_line_supports_text_mode() {
        assert_eq!(
            parse_checksum_line(&format!("{HASH_A}  geoengine-linux-x86_64.tar.gz")),
            Some((HASH_A, "geoengine-linux-x86_64.tar.gz"))
        );
    }

    #[test]
    fn parse_checksum_line_supports_binary_mode() {
        assert_eq!(
            parse_checksum_line(&format!("{HASH_A} *geoengine-windows-x86_64.zip")),
            Some((HASH_A, "geoengine-windows-x86_64.zip"))
        );
    }

    #[test]
    fn parse_checksum_line_returns_none_for_empty_input() {
        assert_eq!(parse_checksum_line(""), None);
    }

    #[test]
    fn parse_checksum_line_returns_none_without_separator() {
        assert_eq!(parse_checksum_line(HASH_A), None);
    }

    #[test]
    fn parse_checksum_line_returns_none_for_empty_name_after_binary_marker() {
        assert_eq!(parse_checksum_line(&format!("{HASH_A} *")), None);
    }

    #[test]
    fn expected_checksum_from_text_returns_matching_hash() {
        let checksums = format!(
            "{HASH_A}  geoengine-linux-x86_64.tar.gz\n{HASH_B} *geoengine-windows-x86_64.zip\n"
        );

        let hash = expected_checksum_from_text(&checksums, "geoengine-windows-x86_64.zip")
            .expect("expected archive entry should return its checksum");

        assert_eq!(hash, HASH_B);
    }

    #[test]
    fn expected_checksum_from_text_errors_when_archive_is_missing() {
        let checksums = format!(
            "{HASH_A}  geoengine-linux-x86_64.tar.gz\n{HASH_B} *geoengine-windows-x86_64.zip\n"
        );

        let err = expected_checksum_from_text(&checksums, "geoengine-darwin-aarch64.tar.gz")
            .expect_err("missing archive should return an error");

        assert!(err
            .to_string()
            .contains("No checksum entry found for 'geoengine-darwin-aarch64.tar.gz'"));
    }

    #[test]
    fn temp_file_candidate_path_uses_provided_dir() {
        let dir = std::path::Path::new("/tmp/custom-dir");

        assert_eq!(
            temp_file_candidate_path(dir, "geoengine_test", Some("sh"), 3),
            dir.join("geoengine_test_3.sh")
        );
    }
}
