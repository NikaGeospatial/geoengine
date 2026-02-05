use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

use crate::cli::run::ContainerConfig;
use crate::config::project::ProjectConfig;
use crate::config::settings::Settings;
use crate::docker::client::DockerClient;
use crate::docker::gpu::GpuConfig;

/// Job request from GIS application
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobRequest {
    /// Project name
    pub project: String,

    /// Tool name to execute
    pub tool: String,

    /// Input parameters
    #[serde(default)]
    pub inputs: HashMap<String, serde_json::Value>,

    /// Output directory for results
    pub output_dir: Option<String>,
}

/// Job status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}

/// A processing job
#[derive(Debug, Clone)]
pub struct Job {
    pub id: Uuid,
    pub request: JobRequest,
    pub status: JobStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub container_id: Option<String>,
    pub error: Option<String>,
    pub logs: Vec<String>,
}

impl Job {
    pub fn new(request: JobRequest) -> Self {
        Self {
            id: Uuid::new_v4(),
            request,
            status: JobStatus::Queued,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            container_id: None,
            error: None,
            logs: Vec::new(),
        }
    }
}

/// Manages job queue and execution
pub struct JobManager {
    jobs: HashMap<Uuid, Job>,
    max_workers: usize,
    running_count: usize,
}

impl JobManager {
    pub fn new(max_workers: usize) -> Self {
        Self {
            jobs: HashMap::new(),
            max_workers,
            running_count: 0,
        }
    }

    /// Submit a new job
    pub async fn submit(&mut self, request: JobRequest) -> Result<Uuid> {
        // Validate project exists
        let settings = Settings::load()?;
        let project_path = settings.get_project_path(&request.project)?;

        // Validate tool exists
        let config = ProjectConfig::load(&project_path.join("geoengine.yaml"))?;
        let tools = config
            .gis
            .as_ref()
            .and_then(|g| g.tools.as_ref())
            .ok_or_else(|| anyhow::anyhow!("Project has no GIS tools defined"))?;

        let tool = tools
            .iter()
            .find(|t| t.name == request.tool)
            .ok_or_else(|| anyhow::anyhow!("Tool '{}' not found in project", request.tool))?;

        // Validate required inputs
        if let Some(inputs) = &tool.inputs {
            for input in inputs {
                if input.required.unwrap_or(true) && !request.inputs.contains_key(&input.name) {
                    anyhow::bail!("Missing required input: {}", input.name);
                }
            }
        }

        let job = Job::new(request);
        let job_id = job.id;

        self.jobs.insert(job_id, job);
        tracing::info!("Job {} queued", job_id);

        Ok(job_id)
    }

    /// Get a job by ID
    pub fn get_job(&self, id: &Uuid) -> Option<&Job> {
        self.jobs.get(id)
    }

    /// List jobs
    pub fn list_jobs(&self, include_completed: bool) -> Vec<&Job> {
        self.jobs
            .values()
            .filter(|j| {
                include_completed
                    || matches!(j.status, JobStatus::Queued | JobStatus::Running)
            })
            .collect()
    }

    /// Cancel a job
    pub async fn cancel(&mut self, id: &Uuid) -> Result<()> {
        let job = self
            .jobs
            .get_mut(id)
            .ok_or_else(|| anyhow::anyhow!("Job not found"))?;

        match job.status {
            JobStatus::Queued => {
                job.status = JobStatus::Cancelled;
                job.completed_at = Some(Utc::now());
            }
            JobStatus::Running => {
                // Stop the container
                if let Some(container_id) = &job.container_id {
                    let client = DockerClient::new().await?;
                    client.stop_container(container_id).await?;
                    client.remove_container(container_id, true).await?;
                }
                job.status = JobStatus::Cancelled;
                job.completed_at = Some(Utc::now());
                self.running_count = self.running_count.saturating_sub(1);
            }
            _ => {
                anyhow::bail!("Job cannot be cancelled (status: {:?})", job.status);
            }
        }

        tracing::info!("Job {} cancelled", id);
        Ok(())
    }

    /// Process pending jobs
    pub async fn process_pending(&mut self) -> Result<()> {
        // Find queued jobs that can be started
        let queued_ids: Vec<Uuid> = self
            .jobs
            .iter()
            .filter(|(_, j)| j.status == JobStatus::Queued)
            .map(|(id, _)| *id)
            .collect();

        for job_id in queued_ids {
            if self.running_count >= self.max_workers {
                break;
            }

            // Start the job
            if let Err(e) = self.start_job(&job_id).await {
                tracing::error!("Failed to start job {}: {}", job_id, e);
                if let Some(job) = self.jobs.get_mut(&job_id) {
                    job.status = JobStatus::Failed;
                    job.error = Some(e.to_string());
                    job.completed_at = Some(Utc::now());
                }
            }
        }

        // Check running jobs for completion
        let running_ids: Vec<Uuid> = self
            .jobs
            .iter()
            .filter(|(_, j)| j.status == JobStatus::Running)
            .map(|(id, _)| *id)
            .collect();

        for job_id in running_ids {
            // TODO: Check container status and update job
        }

        Ok(())
    }

    /// Start a job
    async fn start_job(&mut self, job_id: &Uuid) -> Result<()> {
        let job = self
            .jobs
            .get_mut(job_id)
            .ok_or_else(|| anyhow::anyhow!("Job not found"))?;

        // Load project config
        let settings = Settings::load()?;
        let project_path = settings.get_project_path(&job.request.project)?;
        let config = ProjectConfig::load(&project_path.join("geoengine.yaml"))?;

        // Find tool
        let tool = config
            .gis
            .as_ref()
            .and_then(|g| g.tools.as_ref())
            .and_then(|tools| tools.iter().find(|t| t.name == job.request.tool))
            .ok_or_else(|| anyhow::anyhow!("Tool not found"))?;

        // Get script command
        let script_cmd = config
            .scripts
            .as_ref()
            .and_then(|s| s.get(&tool.script))
            .ok_or_else(|| anyhow::anyhow!("Script '{}' not found", tool.script))?;

        // Build container config
        let image_tag = format!("geoengine-{}:latest", config.name);

        let mut env_vars: HashMap<String, String> = config
            .runtime
            .as_ref()
            .and_then(|r| r.environment.clone())
            .unwrap_or_default();

        // Add input parameters as environment variables
        for (key, value) in &job.request.inputs {
            env_vars.insert(
                format!("GEOENGINE_INPUT_{}", key.to_uppercase()),
                value.to_string().trim_matches('"').to_string(),
            );
        }

        // Add output directory
        if let Some(output_dir) = &job.request.output_dir {
            env_vars.insert("GEOENGINE_OUTPUT_DIR".to_string(), "/output".to_string());
        }

        let mut mounts: Vec<(String, String, bool)> = Vec::new();

        // Add project mounts
        if let Some(runtime) = &config.runtime {
            if let Some(mount_configs) = &runtime.mounts {
                for m in mount_configs {
                    let host_path = if m.host.starts_with("./") {
                        project_path.join(&m.host[2..])
                    } else {
                        std::path::PathBuf::from(&m.host)
                    };
                    mounts.push((
                        host_path.to_string_lossy().to_string(),
                        m.container.clone(),
                        m.readonly.unwrap_or(false),
                    ));
                }
            }
        }

        // Add output directory mount
        if let Some(output_dir) = &job.request.output_dir {
            mounts.push((output_dir.clone(), "/output".to_string(), false));
        }

        // Add input file mounts
        for (key, value) in &job.request.inputs {
            if let Some(path_str) = value.as_str() {
                let path = std::path::Path::new(path_str);
                if path.exists() && path.is_file() {
                    let container_path = format!("/inputs/{}", path.file_name().unwrap().to_string_lossy());
                    mounts.push((path_str.to_string(), container_path, true));
                }
            }
        }

        let gpu_config = if config.runtime.as_ref().map(|r| r.gpu).unwrap_or(false) {
            GpuConfig::detect().await.ok()
        } else {
            None
        };

        let container_config = ContainerConfig {
            image: image_tag,
            command: Some(vec!["/bin/sh".to_string(), "-c".to_string(), script_cmd.clone()]),
            env_vars,
            mounts,
            gpu_config,
            memory: config.runtime.as_ref().and_then(|r| r.memory.clone()),
            cpus: config.runtime.as_ref().and_then(|r| r.cpus),
            shm_size: config.runtime.as_ref().and_then(|r| r.shm_size.clone()),
            workdir: config.runtime.as_ref().and_then(|r| r.workdir.clone()),
            name: Some(format!("geoengine-job-{}", job_id)),
            remove_on_exit: false,
            detach: true,
            tty: false,
        };

        // Start container
        let client = DockerClient::new().await?;
        let container_id = client.run_container_detached(&container_config).await?;

        job.status = JobStatus::Running;
        job.started_at = Some(Utc::now());
        job.container_id = Some(container_id);
        self.running_count += 1;

        tracing::info!("Job {} started", job_id);

        Ok(())
    }
}
