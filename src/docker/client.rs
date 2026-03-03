use anyhow::{Context, Result};
use bollard::container::{Config, CreateContainerOptions, LogsOptions, StartContainerOptions, WaitContainerOptions};
use bollard::image::{CreateImageOptions, ImportImageOptions, TagImageOptions};
use bollard::Docker;
use futures::StreamExt;
#[cfg(unix)]
use libc;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedSender;

use super::container::ContainerConfig;

/// Docker client wrapper for GeoEngine operations
pub struct DockerClient {
    docker: Docker,
}

/// Information about a Docker image
#[derive(Clone)]
pub struct ImageInfo {
    pub id: String,
    pub repo_tags: Vec<String>,
    pub size: i64,
    pub created: i64,
}

impl DockerClient {
    fn extract_build_step(line: &str) -> Option<String> {
        let line = line.trim();
        if line.is_empty() {
            return None;
        }

        if let Some(rest) = line.strip_prefix('#') {
            let rest = rest.trim_start();
            let split_idx = rest.find(' ')?;
            let after_id = rest[split_idx + 1..].trim_start();

            if after_id.starts_with('[') && !after_id.contains(" DONE ") {
                return Some(after_id.to_string());
            }
            if after_id.starts_with("building with ") {
                return Some(after_id.to_string());
            }
            return None;
        }

        if line.starts_with("Step ") {
            return Some(line.to_string());
        }

        None
    }

    /// Create a new Docker client
    pub async fn new() -> Result<Self> {
        match Self::connect_and_ping().await {
            Ok(docker) => return Ok(Self { docker }),
            Err(initial_error) => {
                // Socket permission issues are unlikely to be fixed by auto-starting Docker.
                if Self::is_permission_denied(&initial_error) {
                    return Err(initial_error).context(
                        "Failed to connect to Docker daemon due to permission denied on the Docker socket",
                    );
                }

                tracing::debug!(
                    "Docker daemon unavailable ({}). Attempting to start Docker...",
                    initial_error
                );

                Self::try_start_docker_daemon().context(
                    "Failed to start Docker automatically. Please start Docker and try again",
                )?;

                const MAX_RETRIES: usize = 30;
                const RETRY_DELAY: Duration = Duration::from_secs(1);

                let mut last_error = initial_error;
                for _ in 0..MAX_RETRIES {
                    tokio::time::sleep(RETRY_DELAY).await;
                    match Self::connect_and_ping().await {
                        Ok(docker) => return Ok(Self { docker }),
                        Err(err) => last_error = err,
                    }
                }

                Err(last_error).context(
                    "Failed to connect to Docker daemon after attempting automatic startup",
                )
            }
        }
    }

    async fn connect_and_ping() -> Result<Docker> {
        let docker = Docker::connect_with_local_defaults()
            .context("Failed to connect to Docker daemon. Is Docker running?")?;

        docker
            .ping()
            .await
            .context("Failed to ping Docker daemon")?;

        Ok(docker)
    }

    fn is_permission_denied(err: &anyhow::Error) -> bool {
        format!("{:#}", err).to_lowercase().contains("permission denied")
    }

    fn try_start_docker_daemon() -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            let status = std::process::Command::new("open")
                .args(["-a", "Docker"])
                .status()
                .context("Failed to run `open -a Docker`")?;

            if status.success() {
                return Ok(());
            }

            anyhow::bail!("`open -a Docker` exited with status {:?}", status.code());
        }

        #[cfg(target_os = "windows")]
        {
            let status = std::process::Command::new("cmd")
                .args(["/C", "start", "", "Docker Desktop"])
                .status()
                .context("Failed to run `cmd /C start \"\" \"Docker Desktop\"`")?;

            if status.success() {
                return Ok(());
            }

            anyhow::bail!(
                "`cmd /C start \"\" \"Docker Desktop\"` exited with status {:?}",
                status.code()
            );
        }

        #[cfg(target_os = "linux")]
        {
            if which::which("systemctl").is_ok() {
                let desktop = std::process::Command::new("systemctl")
                    .args(["--user", "start", "docker-desktop"])
                    .status();
                if matches!(desktop, Ok(status) if status.success()) {
                    return Ok(());
                }

                let daemon = std::process::Command::new("systemctl")
                    .args(["start", "docker"])
                    .status();
                if matches!(daemon, Ok(status) if status.success()) {
                    return Ok(());
                }
            }

            if which::which("service").is_ok() {
                let service = std::process::Command::new("service")
                    .args(["docker", "start"])
                    .status();
                if matches!(service, Ok(status) if status.success()) {
                    return Ok(());
                }
            }

            anyhow::bail!(
                "Could not auto-start Docker on Linux via systemctl/service. Start Docker manually."
            );
        }

        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            anyhow::bail!("Auto-start Docker is not supported on this operating system.");
        }
    }

    /// Import a Docker image from a tar file
    pub async fn import_image(&self, tarfile: &PathBuf, tag: Option<&str>) -> Result<String> {
        // Read the entire tar file into memory
        let file_contents = tokio::fs::read(tarfile)
            .await
            .with_context(|| format!("Failed to read tar file: {}", tarfile.display()))?;

        let options = ImportImageOptions {
            ..Default::default()
        };

        let mut stream = self.docker.import_image(options, file_contents.into(), None);

        let mut image_id = String::new();
        while let Some(result) = stream.next().await {
            match result {
                Ok(info) => {
                    if let Some(status) = info.status {
                        tracing::debug!("Import status: {}", status);
                    }
                    if let Some(id) = info.id {
                        image_id = id;
                    }
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Import failed: {}", e));
                }
            }
        }

        // Tag the image if requested
        if let Some(tag) = tag {
            self.tag_image(&image_id, tag).await?;
        }

        Ok(image_id)
    }

    /// List Docker images under geoengine
    pub async fn list_images(&self, filter: Option<&str>, all: bool) -> Result<Vec<ImageInfo>> {
        let options = bollard::image::ListImagesOptions::<String> {
            all,
            ..Default::default()
        };

        let images = self.docker.list_images(Some(options)).await?;

        let mut result: Vec<ImageInfo> = images
            .into_iter()
            .filter(|img| {
               img.repo_tags.iter().any(|t| t.starts_with("geoengine-local"))
            })
            .filter(|img| {
                if let Some(f) = filter {
                    img.repo_tags
                        .iter()
                        .any(|t| t.contains(f))
                } else {
                    true
                }
            })
            .map(|img| ImageInfo {
                id: img.id,
                repo_tags: img.repo_tags,
                size: img.size,
                created: img.created,
            })
            .collect();

        result.sort_by(|a, b| b.created.cmp(&a.created));
        Ok(result)
    }

    /// Pull a Docker image from a registry
    pub async fn pull_image(&self, image: &str) -> Result<()> {
        let options = Some(CreateImageOptions {
            from_image: image,
            ..Default::default()
        });

        let mut stream = self.docker.create_image(options, None, None);

        while let Some(result) = stream.next().await {
            match result {
                Ok(info) => {
                    if let Some(status) = info.status {
                        tracing::debug!("Pull status: {}", status);
                    }
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Pull failed: {}", e));
                }
            }
        }

        Ok(())
    }

    /// Remove a Docker image
    pub async fn remove_image(&self, image: &str, force: bool) -> Result<()> {
        let options = bollard::image::RemoveImageOptions {
            force,
            ..Default::default()
        };

        self.docker.remove_image(image, Some(options), None).await?;
        Ok(())
    }

    /// Export a Docker image to a tar file
    pub async fn export_image(&self, image: &str, output: &PathBuf) -> Result<()> {
        let mut stream = self.docker.export_image(image);

        let mut file = tokio::fs::File::create(output)
            .await
            .with_context(|| format!("Failed to create output file: {}", output.display()))?;

        while let Some(result) = stream.next().await {
            match result {
                Ok(data) => {
                    file.write_all(&data).await?;
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Export failed: {}", e));
                }
            }
        }

        file.flush().await?;
        Ok(())
    }

    /// Tag a Docker image
    pub async fn tag_image(&self, source: &str, target: &str) -> Result<()> {
        let (repo, tag) = if target.contains(':') {
            let parts: Vec<&str> = target.rsplitn(2, ':').collect();
            (parts[1], parts[0])
        } else {
            (target, "latest")
        };

        let options = TagImageOptions { repo, tag };
        self.docker.tag_image(source, Some(options)).await?;
        Ok(())
    }

    /// Push a Docker image to a registry
    pub async fn push_image(&self, image: &str) -> Result<()> {
        let options = bollard::image::PushImageOptions::<String> {
            tag: image.split(':').last().unwrap_or("latest").to_string(),
        };

        let mut stream = self.docker.push_image(
            image.split(':').next().unwrap_or(image),
            Some(options),
            None,
        );

        while let Some(result) = stream.next().await {
            match result {
                Ok(info) => {
                    if let Some(status) = info.status {
                        tracing::debug!("Push status: {}", status);
                    }
                    if let Some(error) = info.error {
                        return Err(anyhow::anyhow!("Push failed: {}", error));
                    }
                }
                Err(e) => {
                    return Err(anyhow::anyhow!("Push failed: {}", e));
                }
            }
        }

        Ok(())
    }

    /// Build a Docker image
    pub async fn build_image(
        &self,
        dockerfile: &PathBuf,
        context: &PathBuf,
        tag: &str,
        build_args: &HashMap<String, String>,
        no_cache: bool,
        verbose: bool,
        progress_step_tx: Option<UnboundedSender<String>>,
    ) -> Result<()> {
        let mut cmd = std::process::Command::new("docker");
        cmd.args(["build", "-t", tag, "-f"]);
        cmd.arg(dockerfile.as_os_str());

        if no_cache {
            cmd.arg("--no-cache");
        }

        for (k, v) in build_args {
            cmd.args(["--build-arg", &format!("{}={}", k, v)]);
        }

        if verbose {
            cmd.args(["--progress", "plain"]);
            // Inherit stdout/stderr so output flows directly to the terminal
            cmd.stdout(std::process::Stdio::inherit());
            cmd.stderr(std::process::Stdio::inherit());
        } else {
            // Suppress output — caller shows a spinner
            cmd.stdout(std::process::Stdio::null());
            cmd.stderr(std::process::Stdio::piped());
        }

        cmd.arg(context.as_os_str());

        // Convert to tokio::process::Command so we don't block the async runtime
        // during what can be a multi-minute build.
        let mut cmd: tokio::process::Command = cmd.into();

        let mut child = cmd.spawn().context("Failed to spawn `docker build`")?;

        // Capture stderr when not verbose so we can surface it on failure.
        // Reading stderr to completion before wait() drains the pipe and prevents
        // the child from blocking on a full buffer. Capped at 64 KiB to avoid
        // unbounded memory growth from pathological docker build output.
        const MAX_STDERR_BYTES: usize = 64 * 1024;
        let stderr_output = if !verbose {
            let stderr = child.stderr.take();
            if let Some(stderr) = stderr {
                let step_sender = progress_step_tx;
                let mut reader = tokio::io::BufReader::new(stderr).lines();
                let mut raw = Vec::with_capacity(MAX_STDERR_BYTES + 1);

                while let Some(line) = reader
                    .next_line()
                    .await
                    .context("Failed to read docker build output")?
                {
                    let trimmed = line.trim_end_matches('\r');

                    if let Some(step) = Self::extract_build_step(trimmed) {
                        if let Some(tx) = step_sender.as_ref() {
                            let _ = tx.send(step);
                        }
                    }

                    if raw.len() <= MAX_STDERR_BYTES {
                        if !raw.is_empty() {
                            raw.push(b'\n');
                        }
                        let bytes = trimmed.as_bytes();
                        let remaining = (MAX_STDERR_BYTES + 1).saturating_sub(raw.len());
                        let take = remaining.min(bytes.len());
                        raw.extend_from_slice(&bytes[..take]);
                    }
                }

                drop(step_sender);

                let truncated = raw.len() > MAX_STDERR_BYTES;
                let mut s = String::from_utf8_lossy(&raw[..raw.len().min(MAX_STDERR_BYTES)]).into_owned();
                if truncated {
                    s.push_str("\n...(truncated)");
                }
                s
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let status = child.wait().await.context("`docker build` process failed")?;

        if !status.success() {
            if verbose {
                anyhow::bail!("Build failed (exit code {:?}).", status.code());
            } else if stderr_output.is_empty() {
                anyhow::bail!("Build failed (exit code {:?}). Re-run with --verbose for details.", status.code());
            } else {
                anyhow::bail!("Build failed:\n{}", stderr_output.trim());
            }
        }

        Ok(())
    }

    /// Run a container and wait for it to complete (attached mode)
    pub async fn run_container_attached(&self, config: &ContainerConfig) -> Result<i64> {
        self.run_container_attached_internal(config, false).await
    }

    /// Run a container attached, routing all container output to host stderr.
    /// This keeps host stdout free for structured output (e.g. JSON results).
    pub async fn run_container_attached_to_stderr(&self, config: &ContainerConfig) -> Result<i64> {
        self.run_container_attached_internal(config, true).await
    }

    async fn run_container_attached_internal(
        &self,
        config: &ContainerConfig,
        logs_to_stderr: bool,
    ) -> Result<i64> {
        let container_id = self.create_container(config).await?;

        // Start the container
        self.docker
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await?;

        if !logs_to_stderr {
            let command_display = config
                .command
                .as_ref()
                .map(|cmd| cmd.join(" "))
                .unwrap_or_else(|| "<none>".to_string());
            println!("Container command: {}", command_display);
        }

        // Stream logs until completion or cancellation signal.
        let log_options = LogsOptions::<String> {
            follow: true,
            stdout: true,
            stderr: true,
            ..Default::default()
        };

        let mut log_stream = self.docker.logs(&container_id, Some(log_options));
        let mut shutdown_signal = Box::pin(Self::wait_for_shutdown_signal());
        let mut cancel_reason: Option<&'static str> = None;

        loop {
            tokio::select! {
                signal = &mut shutdown_signal => {
                    cancel_reason = Some(signal?);
                    break;
                }
                result = log_stream.next() => {
                    let Some(result) = result else {
                        break;
                    };
                    match result {
                        Ok(output) => {
                            if logs_to_stderr {
                                eprint!("{}", output);
                            } else {
                                print!("{}", output);
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Log stream error: {}", e);
                            break;
                        }
                    }
                }
            }
        }

        let exit_code = if cancel_reason.is_none() {
            let wait_options = WaitContainerOptions {
                condition: "not-running",
            };

            let mut wait_stream = self.docker.wait_container(&container_id, Some(wait_options));
            let wait_result = tokio::select! {
                signal = &mut shutdown_signal => {
                    cancel_reason = Some(signal?);
                    None
                }
                result = wait_stream.next() => result
            };

            if let Some(result) = wait_result {
                match result {
                    Ok(response) => response.status_code,
                    Err(e) => {
                        tracing::warn!("Wait error: {}", e);
                        -1
                    }
                }
            } else {
                0
            }
        } else {
            -1
        };

        if let Some(reason) = cancel_reason {
            let cleanup_result = self
                .cleanup_container_after_cancellation(&container_id, config.remove_on_exit, reason)
                .await;
            if let Err(err) = cleanup_result {
                anyhow::bail!("Run cancelled ({reason}). Container cleanup error: {err}");
            }
            anyhow::bail!("Run cancelled ({reason})");
        }

        self.cleanup_container_on_exit(&container_id, config.remove_on_exit)
            .await;

        Ok(exit_code)
    }

    #[cfg(unix)]
    async fn wait_for_shutdown_signal() -> Result<&'static str> {
        use tokio::signal::unix::{signal, SignalKind};
        let mut sigterm = signal(SignalKind::terminate())
            .context("Failed to subscribe to SIGTERM")?;
        tokio::select! {
            res = tokio::signal::ctrl_c() => {
                res.context("Failed while waiting for Ctrl-C")?;
                Ok("SIGINT")
            }
            _ = sigterm.recv() => Ok("SIGTERM"),
        }
    }

    #[cfg(not(unix))]
    async fn wait_for_shutdown_signal() -> Result<&'static str> {
        tokio::signal::ctrl_c()
            .await
            .context("Failed while waiting for Ctrl-C")?;
        Ok("Ctrl-C")
    }

    async fn cleanup_container_after_cancellation(
        &self,
        container_id: &str,
        remove_on_exit: bool,
        reason: &str,
    ) -> Result<()> {
        eprintln!(
            "Cancellation signal ({}) received. Stopping container {}...",
            reason,
            container_id
        );

        let mut errors: Vec<String> = Vec::new();

        if let Err(err) = self.stop_container(container_id).await {
            let msg = format!("Failed to stop container {}: {}", container_id, err);
            eprintln!("{}", msg);
            if !Self::is_benign_stop_or_remove_error(&err) {
                errors.push(msg);
            }
        }

        if remove_on_exit {
            if let Err(err) = self.remove_container(container_id, true).await {
                let msg = format!("Failed to remove container {}: {}", container_id, err);
                eprintln!("{}", msg);
                if !Self::is_benign_stop_or_remove_error(&err) {
                    errors.push(msg);
                }
            }
        }

        if errors.is_empty() {
            eprintln!("Container {} cleaned up after cancellation.", container_id);
            Ok(())
        } else {
            anyhow::bail!("{}", errors.join(" | "));
        }
    }

    async fn cleanup_container_on_exit(&self, container_id: &str, remove_on_exit: bool) {
        if !remove_on_exit {
            return;
        }

        if let Err(err) = self.remove_container(container_id, true).await {
            tracing::warn!(
                "Failed to remove container {} after exit: {}",
                container_id,
                err
            );
        }
    }

    fn is_benign_stop_or_remove_error(err: &anyhow::Error) -> bool {
        let msg = format!("{:#}", err).to_ascii_lowercase();
        msg.contains("is not running")
            || msg.contains("already stopped")
            || msg.contains("no such container")
            || msg.contains("not found")
            || msg.contains("404")
    }

    /// Run a container in detached mode
    pub async fn run_container_detached(&self, config: &ContainerConfig) -> Result<String> {
        let container_id = self.create_container(config).await?;

        self.docker
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await?;

        Ok(container_id)
    }

    /// Create a container (helper method)
    async fn create_container(&self, config: &ContainerConfig) -> Result<String> {
        let mut env: Vec<String> = config
            .env_vars
            .iter()
            .map(|(k, v)| format!("{}={}", k, v))
            .collect();

        // Build bind mounts
        let binds: Vec<String> = config
            .mounts
            .iter()
            .map(|(host, container, ro)| {
                if *ro {
                    format!("{}:{}:ro", host, container)
                } else {
                    format!("{}:{}", host, container)
                }
            })
            .collect();

        // Build host config
        let mut host_config = bollard::models::HostConfig {
            binds: Some(binds),
            auto_remove: Some(config.remove_on_exit && config.detach),
            ..Default::default()
        };

        // GPU configuration — only set up Docker device requests for NVIDIA GPUs.
        // Metal (macOS) does not require Docker-level GPU passthrough.
        if let Some(gpu_config) = &config.gpu_config {
            if gpu_config.is_nvidia() {
                host_config.device_requests = Some(vec![bollard::models::DeviceRequest {
                    driver: Some("nvidia".to_string()),
                    count: Some(-1), // All available GPUs
                    capabilities: Some(vec![vec!["gpu".to_string()]]),
                    ..Default::default()
                }]);

                // Add NVIDIA env vars
                env.push("NVIDIA_VISIBLE_DEVICES=all".to_string());
                env.push("NVIDIA_DRIVER_CAPABILITIES=compute,utility".to_string());
            }
        }

        // Optionally inject the host UID:GID so the container process owns its
        // bind-mounted directories.  Skipped for images that expect root or
        // that manage their own user.
        let user: Option<String> = if config.inject_host_user {
            #[cfg(unix)]
            {
                let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
                Some(format!("{}:{}", uid, gid))
            }
            #[cfg(not(unix))]
            {
                None
            }
        } else {
            None
        };

        let container_config = Config {
            image: Some(config.image.clone()),
            cmd: config.command.clone(),
            env: Some(env),
            working_dir: config.workdir.clone(),
            tty: Some(config.tty),
            attach_stdin: Some(!config.detach),
            attach_stdout: Some(!config.detach),
            attach_stderr: Some(!config.detach),
            user,
            host_config: Some(host_config),
            ..Default::default()
        };

        let options = config.name.as_ref().map(|name| CreateContainerOptions {
            name: name.clone(),
            platform: None,
        });

        let response = self
            .docker
            .create_container(options, container_config)
            .await?;

        Ok(response.id)
    }

    /// Stop a running container
    pub async fn stop_container(&self, container_id: &str) -> Result<()> {
        self.docker
            .stop_container(container_id, None)
            .await?;
        Ok(())
    }

    /// Remove a container
    pub async fn remove_container(&self, container_id: &str, force: bool) -> Result<()> {
        let options = bollard::container::RemoveContainerOptions {
            force,
            ..Default::default()
        };
        self.docker.remove_container(container_id, Some(options)).await?;
        Ok(())
    }
}
