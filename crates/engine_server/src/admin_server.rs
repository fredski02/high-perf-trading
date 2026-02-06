use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::State,
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
    Router,
};

use common::Metrics;

#[derive(Clone)]
struct AppState {
    metrics: Arc<Metrics>,
}

pub async fn run(addr: String, metrics: Arc<Metrics>) -> anyhow::Result<()> {
    let state = AppState { metrics };

    let app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics_handler))
        .with_state(state);

    let addr: SocketAddr = addr.parse()?;
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;

    Ok(())
}

async fn health() -> impl IntoResponse {
    (StatusCode::OK, "ok\n")
}

async fn metrics_handler(State(state): State<AppState>) -> impl IntoResponse {
    let body = state.metrics.render_prom_text();
    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}
