use anyhow::{Context, Result};
use bollard::container::{Config, CreateContainerOptions, LogsOptions, StartContainerOptions, WaitContainerOptions};
use bollard::image::{CreateImageOptions, ImportImageOptions, TagImageOptions};
use bollard::Docker;
use futures::StreamExt;
#[cfg(unix)]
use libc;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;

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
    /// Create a new Docker client
    pub async fn new() -> Result<Self> {
        let docker = Docker::connect_with_local_defaults()
            .context("Failed to connect to Docker daemon. Is Docker running?")?;

        // Verify connection
        docker
            .ping()
            .await
            .context("Failed to ping Docker daemon")?;

        Ok(Self { docker })
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
    ) -> Result<()> {
        let mut cmd = std::process::Command::new("docker");
        cmd.args(["build", "-t", tag, "-f", dockerfile.to_str().unwrap()]);

        if no_cache {
            cmd.arg("--no-cache");
        }

        for (k, v) in build_args {
            cmd.args(["--build-arg", &format!("{}={}", k, v)]);
        }

        cmd.arg(context.to_str().unwrap());

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

        // Convert to tokio::process::Command so we don't block the async runtime
        // during what can be a multi-minute build.
        let mut cmd: tokio::process::Command = cmd.into();

        let mut child = cmd.spawn().context("Failed to spawn `docker build`")?;

        // Capture stderr when not verbose so we can surface it on failure.
        // Reading stderr to completion before wait() drains the pipe and prevents
        // the child from blocking on a full buffer.
        let stderr_output = if !verbose {
            let stderr = child.stderr.take();
            if let Some(stderr) = stderr {
                use tokio::io::AsyncReadExt;
                let mut buf = String::new();
                tokio::io::BufReader::new(stderr).read_to_string(&mut buf).await.ok();
                buf
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        let status = child.wait().await.context("`docker build` process failed")?;

        if !status.success() {
            if stderr_output.is_empty() {
                anyhow::bail!("Build failed (exit code {:?}). Re-run with --verbose for details.", status.code());
            } else {
                anyhow::bail!("Build failed:\n{}", stderr_output.trim());
            }
        }

        Ok(())
    }

    /// Run a container and wait for it to complete (attached mode)
    pub async fn run_container_attached(&self, config: &ContainerConfig) -> Result<i64> {
        let container_id = self.create_container(config).await?;

        // Start the container
        self.docker
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await?;
        
        let command_display = config
            .command
            .as_ref()
            .map(|cmd| cmd.join(" "))
            .unwrap_or_else(|| "<none>".to_string());
        println!("Container command: {}", command_display);

        // Stream logs
        let log_options = LogsOptions::<String> {
            follow: true,
            stdout: true,
            stderr: true,
            ..Default::default()
        };

        let mut log_stream = self.docker.logs(&container_id, Some(log_options));

        while let Some(result) = log_stream.next().await {
            match result {
                Ok(output) => {
                    print!("{}", output);
                }
                Err(e) => {
                    tracing::warn!("Log stream error: {}", e);
                    break;
                }
            }
        }

        // Wait for container to finish
        let wait_options = WaitContainerOptions {
            condition: "not-running",
        };

        let mut wait_stream = self.docker.wait_container(&container_id, Some(wait_options));
        let exit_code = if let Some(result) = wait_stream.next().await {
            match result {
                Ok(response) => response.status_code,
                Err(e) => {
                    tracing::warn!("Wait error: {}", e);
                    -1
                }
            }
        } else {
            0
        };

        // Remove container if requested
        if config.remove_on_exit {
            self.docker
                .remove_container(
                    &container_id,
                    Some(bollard::container::RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                )
                .await
                .ok();
        }

        Ok(exit_code)
    }

    /// Run a container attached, routing all container output to host stderr.
    /// This keeps host stdout free for structured output (e.g. JSON results).
    pub async fn run_container_attached_to_stderr(&self, config: &ContainerConfig) -> Result<i64> {
        let container_id = self.create_container(config).await?;

        // Start the container
        self.docker
            .start_container(&container_id, None::<StartContainerOptions<String>>)
            .await?;

        // Stream logs to stderr
        let log_options = LogsOptions::<String> {
            follow: true,
            stdout: true,
            stderr: true,
            ..Default::default()
        };

        let mut log_stream = self.docker.logs(&container_id, Some(log_options));

        while let Some(result) = log_stream.next().await {
            match result {
                Ok(output) => {
                    eprint!("{}", output);
                }
                Err(e) => {
                    tracing::warn!("Log stream error: {}", e);
                    break;
                }
            }
        }

        // Wait for container to finish
        let wait_options = WaitContainerOptions {
            condition: "not-running",
        };

        let mut wait_stream = self.docker.wait_container(&container_id, Some(wait_options));
        let exit_code = if let Some(result) = wait_stream.next().await {
            match result {
                Ok(response) => response.status_code,
                Err(e) => {
                    tracing::warn!("Wait error: {}", e);
                    -1
                }
            }
        } else {
            0
        };

        // Remove container if requested
        if config.remove_on_exit {
            self.docker
                .remove_container(
                    &container_id,
                    Some(bollard::container::RemoveContainerOptions {
                        force: true,
                        ..Default::default()
                    }),
                )
                .await
                .ok();
        }

        Ok(exit_code)
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
