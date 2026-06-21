mod api;
mod config;
mod db;
mod media;
mod models;
mod workers;

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Context;
use axum::Router;
use tower_http::{cors::CorsLayer, services::ServeDir, trace::TraceLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use crate::{api::AppState, db::Database, workers::WorkerManager};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "flowlapse=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let data_dir = std::env::var("FLOWLAPSE_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("data"));
    std::fs::create_dir_all(&data_dir).context("creating data dir")?;

    let db = Database::open(data_dir.join("flowlapse.db")).context("opening database")?;
    db.migrate().context("running database migrations")?;
    db.reconcile_storage(&data_dir)
        .context("reconciling persisted segment state")?;

    let state = Arc::new(AppState {
        db,
        data_dir,
        workers: WorkerManager::default(),
    });

    let static_dir = std::env::var("FLOWLAPSE_STATIC_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("web/dist"));

    let app = api::router(state)
        .nest_service(
            "/",
            ServeDir::new(static_dir).append_index_html_on_directories(true),
        )
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http());

    let bind = std::env::var("FLOWLAPSE_BIND").unwrap_or_else(|_| "127.0.0.1:4822".to_string());
    let addr: SocketAddr = bind.parse().context("parsing FLOWLAPSE_BIND")?;
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    tracing::info!(%addr, "flowlapse daemon listening");
    axum::serve(listener, Router::new().merge(app))
        .await
        .context("serving http")?;

    Ok(())
}
