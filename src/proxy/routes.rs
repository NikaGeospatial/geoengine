use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use super::jobs::{JobRequest, JobStatus};
use super::server::AppState;
use crate::config::project::ProjectConfig;
use crate::config::settings::Settings;

/// Health check response
#[derive(Serialize)]
pub struct HealthResponse {
    status: String,
    version: String,
    uptime_seconds: u64,
}

/// Query parameters for listing jobs
#[derive(Deserialize)]
pub struct ListJobsQuery {
    all: Option<bool>,
}

/// Health check endpoint
pub async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: 0, // TODO: Track actual uptime
    })
}

/// List all jobs
pub async fn list_jobs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListJobsQuery>,
) -> Json<Vec<JobSummary>> {
    let manager = state.job_manager.read().await;
    let jobs = manager.list_jobs(params.all.unwrap_or(false));

    Json(
        jobs.iter()
            .map(|j| JobSummary {
                id: j.id.to_string(),
                project: j.request.project.clone(),
                tool: j.request.tool.clone(),
                status: format!("{:?}", j.status).to_lowercase(),
                created_at: j.created_at.to_rfc3339(),
                started_at: j.started_at.map(|t| t.to_rfc3339()),
                completed_at: j.completed_at.map(|t| t.to_rfc3339()),
            })
            .collect(),
    )
}

#[derive(Serialize)]
pub struct JobSummary {
    id: String,
    project: String,
    tool: String,
    status: String,
    created_at: String,
    started_at: Option<String>,
    completed_at: Option<String>,
}

/// Submit a new job
pub async fn submit_job(
    State(state): State<Arc<AppState>>,
    Json(request): Json<JobRequest>,
) -> Result<Json<JobResponse>, AppError> {
    let mut manager = state.job_manager.write().await;

    let job_id = manager.submit(request).await?;

    Ok(Json(JobResponse {
        id: job_id.to_string(),
        status: "queued".to_string(),
    }))
}

#[derive(Serialize)]
pub struct JobResponse {
    id: String,
    status: String,
}

/// Get job details
pub async fn get_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<JobDetails>, AppError> {
    let manager = state.job_manager.read().await;

    let job_id = uuid::Uuid::parse_str(&id)
        .map_err(|_| AppError::NotFound("Invalid job ID".to_string()))?;

    let job = manager
        .get_job(&job_id)
        .ok_or_else(|| AppError::NotFound("Job not found".to_string()))?;

    Ok(Json(JobDetails {
        id: job.id.to_string(),
        project: job.request.project.clone(),
        tool: job.request.tool.clone(),
        inputs: job.request.inputs.clone(),
        status: format!("{:?}", job.status).to_lowercase(),
        created_at: job.created_at.to_rfc3339(),
        started_at: job.started_at.map(|t| t.to_rfc3339()),
        completed_at: job.completed_at.map(|t| t.to_rfc3339()),
        output_dir: job.request.output_dir.clone(),
        error: job.error.clone(),
        logs: job.logs.clone(),
    }))
}

#[derive(Serialize)]
pub struct JobDetails {
    id: String,
    project: String,
    tool: String,
    inputs: HashMap<String, serde_json::Value>,
    status: String,
    created_at: String,
    started_at: Option<String>,
    completed_at: Option<String>,
    output_dir: Option<String>,
    error: Option<String>,
    logs: Vec<String>,
}

/// Cancel a job
pub async fn cancel_job(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<JobResponse>, AppError> {
    let mut manager = state.job_manager.write().await;

    let job_id = uuid::Uuid::parse_str(&id)
        .map_err(|_| AppError::NotFound("Invalid job ID".to_string()))?;

    manager.cancel(&job_id).await?;

    Ok(Json(JobResponse {
        id,
        status: "cancelled".to_string(),
    }))
}

/// Get job output files
pub async fn get_job_output(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<OutputResponse>, AppError> {
    let manager = state.job_manager.read().await;

    let job_id = uuid::Uuid::parse_str(&id)
        .map_err(|_| AppError::NotFound("Invalid job ID".to_string()))?;

    let job = manager
        .get_job(&job_id)
        .ok_or_else(|| AppError::NotFound("Job not found".to_string()))?;

    if job.status != JobStatus::Completed {
        return Err(AppError::BadRequest("Job not completed".to_string()));
    }

    // List output files
    let output_dir = job
        .request
        .output_dir
        .as_ref()
        .ok_or_else(|| AppError::NotFound("No output directory".to_string()))?;

    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(output_dir) {
        for entry in entries.flatten() {
            if let Ok(metadata) = entry.metadata() {
                if metadata.is_file() {
                    files.push(OutputFile {
                        name: entry.file_name().to_string_lossy().to_string(),
                        path: entry.path().to_string_lossy().to_string(),
                        size: metadata.len(),
                    });
                }
            }
        }
    }

    Ok(Json(OutputResponse { files }))
}

#[derive(Serialize)]
pub struct OutputResponse {
    files: Vec<OutputFile>,
}

#[derive(Serialize)]
pub struct OutputFile {
    name: String,
    path: String,
    size: u64,
}

/// List registered projects
pub async fn list_projects() -> Result<Json<Vec<ProjectSummary>>, AppError> {
    let settings = Settings::load().map_err(|e| AppError::Internal(e.to_string()))?;

    let mut projects = Vec::new();
    for (name, path) in settings.list_projects() {
        let config_path = path.join("geoengine.yaml");
        if config_path.exists() {
            if let Ok(config) = ProjectConfig::load(&config_path) {
                projects.push(ProjectSummary {
                    name: name.to_string(),
                    version: config.version.clone(),
                    path: path.display().to_string(),
                    tools_count: config
                        .gis
                        .as_ref()
                        .and_then(|g| g.tools.as_ref())
                        .map(|t| t.len())
                        .unwrap_or(0),
                });
            }
        }
    }

    Ok(Json(projects))
}

#[derive(Serialize)]
pub struct ProjectSummary {
    name: String,
    version: Option<String>,
    path: String,
    tools_count: usize,
}

/// Get project details
pub async fn get_project(Path(name): Path<String>) -> Result<Json<ProjectConfig>, AppError> {
    let settings = Settings::load().map_err(|e| AppError::Internal(e.to_string()))?;

    let path = settings
        .get_project_path(&name)
        .map_err(|_| AppError::NotFound("Project not found".to_string()))?;

    let config = ProjectConfig::load(&path.join("geoengine.yaml"))
        .map_err(|e| AppError::Internal(e.to_string()))?;

    Ok(Json(config))
}

/// Get tools for a project
pub async fn get_project_tools(
    Path(name): Path<String>,
) -> Result<Json<Vec<ToolInfo>>, AppError> {
    let settings = Settings::load().map_err(|e| AppError::Internal(e.to_string()))?;

    let path = settings
        .get_project_path(&name)
        .map_err(|_| AppError::NotFound("Project not found".to_string()))?;

    let config = ProjectConfig::load(&path.join("geoengine.yaml"))
        .map_err(|e| AppError::Internal(e.to_string()))?;

    let tools = config
        .gis
        .as_ref()
        .and_then(|g| g.tools.as_ref())
        .map(|tools| {
            tools
                .iter()
                .map(|t| ToolInfo {
                    name: t.name.clone(),
                    label: t.label.clone(),
                    description: t.description.clone(),
                    inputs: t.inputs.as_ref().map(|inputs| {
                        inputs
                            .iter()
                            .map(|i| ParameterInfo {
                                name: i.name.clone(),
                                label: i.label.clone(),
                                param_type: i.param_type.clone(),
                                required: i.required.unwrap_or(true),
                                default: i.default.clone(),
                            })
                            .collect()
                    }),
                    outputs: t.outputs.as_ref().map(|outputs| {
                        outputs
                            .iter()
                            .map(|o| ParameterInfo {
                                name: o.name.clone(),
                                label: o.label.clone(),
                                param_type: o.param_type.clone(),
                                required: o.required.unwrap_or(true),
                                default: o.default.clone(),
                            })
                            .collect()
                    }),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Json(tools))
}

#[derive(Serialize)]
pub struct ToolInfo {
    name: String,
    label: Option<String>,
    description: Option<String>,
    inputs: Option<Vec<ParameterInfo>>,
    outputs: Option<Vec<ParameterInfo>>,
}

#[derive(Serialize)]
pub struct ParameterInfo {
    name: String,
    label: Option<String>,
    param_type: String,
    required: bool,
    default: Option<serde_yaml::Value>,
}

/// Application error type
pub enum AppError {
    NotFound(String),
    BadRequest(String),
    Internal(String),
}

impl From<anyhow::Error> for AppError {
    fn from(err: anyhow::Error) -> Self {
        AppError::Internal(err.to_string())
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        let (status, message) = match self {
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = serde_json::json!({ "error": message });
        (status, Json(body)).into_response()
    }
}
