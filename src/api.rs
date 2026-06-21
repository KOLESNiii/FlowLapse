use std::{path::PathBuf, sync::Arc};

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderValue, Response, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use uuid::Uuid;

use crate::{
    config::{solve_config, ConfigError, ResolvedTimelapseConfig, TimelapseConfigInput},
    db::Database,
    models::{
        CreateExportRequest, CreateSourceRequest, CreateTimelapseRequest, Export, Source, Timelapse,
    },
    workers::{self, WorkerManager},
};

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub data_dir: PathBuf,
    pub workers: WorkerManager,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/config/solve", post(solve_config_handler))
        .route("/api/sources", get(list_sources).post(create_source))
        .route(
            "/api/timelapses",
            get(list_timelapses).post(create_timelapse),
        )
        .route("/api/timelapses/:id", get(get_timelapse))
        .route("/api/timelapses/:id/start", post(start_timelapse))
        .route("/api/timelapses/:id/stop", post(stop_timelapse))
        .route("/api/timelapses/:id/latest-frame", get(latest_frame))
        .route("/api/timelapses/:id/preview", post(create_preview))
        .route("/api/timelapses/:id/preview.mp4", get(download_preview))
        .route("/api/timelapses/:id/exports", post(create_export))
        .route("/api/exports", get(list_exports))
        .route("/api/exports/:id", get(get_export))
        .route("/api/exports/:id/download", get(download_export))
        .with_state(state)
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: true,
        service: "flowlapse",
    })
}

#[derive(Serialize)]
struct HealthResponse {
    ok: bool,
    service: &'static str,
}

async fn solve_config_handler(
    Json(input): Json<TimelapseConfigInput>,
) -> Result<Json<ResolvedTimelapseConfig>, ApiError> {
    Ok(Json(solve_config(input)?))
}

async fn list_sources(State(state): State<Arc<AppState>>) -> Result<Json<Vec<Source>>, ApiError> {
    Ok(Json(state.db.list_sources()?))
}

async fn create_source(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateSourceRequest>,
) -> Result<Json<Source>, ApiError> {
    if request.name.trim().is_empty() {
        return Err(ApiError::bad_request("source name is required"));
    }
    if request.url.trim().is_empty() {
        return Err(ApiError::bad_request("source URL is required"));
    }

    let source = state.db.create_source(
        request.name.trim().to_string(),
        request.kind,
        request.url.trim().to_string(),
        request.rtsp_transport,
    )?;
    Ok(Json(source))
}

async fn list_timelapses(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<Timelapse>>, ApiError> {
    Ok(Json(state.db.list_timelapses()?))
}

async fn create_timelapse(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateTimelapseRequest>,
) -> Result<Json<Timelapse>, ApiError> {
    if request.name.trim().is_empty() {
        return Err(ApiError::bad_request("timelapse name is required"));
    }
    let resolved = solve_config(request.config)?;
    let timelapse =
        state
            .db
            .create_timelapse(request.name.trim().to_string(), request.source_id, resolved)?;
    Ok(Json(timelapse))
}

async fn get_timelapse(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<TimelapseDetail>, ApiError> {
    let timelapse = state
        .db
        .get_timelapse(id)?
        .ok_or_else(|| ApiError::not_found("timelapse not found"))?;
    let source = state
        .db
        .get_source(timelapse.source_id)?
        .ok_or_else(|| ApiError::not_found("source not found"))?;
    let segments = state.db.list_segments(id)?;
    Ok(Json(TimelapseDetail {
        timelapse,
        source,
        segments,
        running: state.workers.is_running(id),
    }))
}

async fn start_timelapse(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<Timelapse>, ApiError> {
    Ok(Json(state.workers.start(state.clone(), id).await?))
}

async fn stop_timelapse(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<Timelapse>, ApiError> {
    Ok(Json(state.workers.stop(&state, id)?))
}

async fn latest_frame(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Response<Body>, ApiError> {
    let timelapse = state
        .db
        .get_timelapse(id)?
        .ok_or_else(|| ApiError::not_found("timelapse not found"))?;
    let source = state
        .db
        .get_source(timelapse.source_id)?
        .ok_or_else(|| ApiError::not_found("source not found"))?;
    let path = source
        .latest_frame_path
        .ok_or_else(|| ApiError::not_found("latest frame is not available yet"))?;
    send_file(PathBuf::from(path), "image/jpeg").await
}

async fn create_preview(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<FileJobResponse>, ApiError> {
    let path = workers::create_preview(state.clone(), id).await?;
    Ok(Json(FileJobResponse {
        path: path.to_string_lossy().to_string(),
        url: format!("/api/timelapses/{id}/preview.mp4"),
    }))
}

async fn download_preview(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Response<Body>, ApiError> {
    let path = state.data_dir.join("previews").join(format!("{id}.mp4"));
    send_file(path, "video/mp4").await
}

async fn create_export(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(request): Json<CreateExportRequest>,
) -> Result<Json<Export>, ApiError> {
    let format = request.format.unwrap_or_else(|| "mp4".to_string());
    if format != "mp4" {
        return Err(ApiError::bad_request(
            "only mp4 exports are currently supported",
        ));
    }
    Ok(Json(workers::queue_export(state, id, format).await?))
}

async fn list_exports(State(state): State<Arc<AppState>>) -> Result<Json<Vec<Export>>, ApiError> {
    Ok(Json(state.db.list_exports()?))
}

async fn get_export(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<Export>, ApiError> {
    let export = state
        .db
        .get_export(id)?
        .ok_or_else(|| ApiError::not_found("export not found"))?;
    Ok(Json(export))
}

async fn download_export(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Response<Body>, ApiError> {
    let export = state
        .db
        .get_export(id)?
        .ok_or_else(|| ApiError::not_found("export not found"))?;
    send_file(PathBuf::from(export.path), "video/mp4").await
}

async fn send_file(path: PathBuf, content_type: &'static str) -> Result<Response<Body>, ApiError> {
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| ApiError::not_found("file not found"))?;
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    Ok(response)
}

#[derive(Serialize)]
struct TimelapseDetail {
    timelapse: Timelapse,
    source: Source,
    segments: Vec<crate::models::Segment>,
    running: bool,
}

#[derive(Serialize)]
struct FileJobResponse {
    path: String,
    url: String,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(error: anyhow::Error) -> Self {
        tracing::error!(error = %error, "api error");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: error.to_string(),
        }
    }
}

impl From<ConfigError> for ApiError {
    fn from(error: ConfigError) -> Self {
        Self::bad_request(error.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}
