use std::{path::PathBuf, sync::Arc};

use axum::{routing::get, Json, Router};
use serde::Serialize;

use crate::{db::Database, workers::WorkerManager};

#[derive(Clone)]
pub struct AppState {
    pub db: Database,
    pub data_dir: PathBuf,
    pub workers: WorkerManager,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/health", get(health))
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

