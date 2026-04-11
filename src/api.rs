use std::sync::{Arc, Mutex};

use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::{Json, Router, routing};

use crate::db::Db;
use crate::models::PeerReport;

const SECRET_HEADER: &str = "X-Netwatch-Token";

#[derive(Clone)]
pub struct AppState {
    pub node_id: String,
    pub db: Arc<Mutex<Db>>,
    pub api_secret: Option<String>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/sync", routing::post(sync_handler))
        .route("/api/status", routing::get(status_handler))
        .route("/api/history/{host}", routing::get(history_handler))
        .layer(middleware::from_fn_with_state(state.clone(), secret_guard))
        .with_state(state)
}

async fn secret_guard(State(state): State<AppState>, req: Request, next: Next) -> Response {
    if let Some(expected) = &state.api_secret {
        let provided = req
            .headers()
            .get(SECRET_HEADER)
            .and_then(|v| v.to_str().ok());

        if provided != Some(expected.as_str()) {
            return StatusCode::NOT_FOUND.into_response();
        }
    }

    next.run(req).await
}

async fn sync_handler(
    State(state): State<AppState>,
    Json(peer_report): Json<PeerReport>,
) -> Response {
    let db = Arc::clone(&state.db);
    let node_id = state.node_id.clone();

    match tokio::task::spawn_blocking(move || {
        let db = db.lock().map_err(|e| format!("db lock poisoned: {e}"))?;

        for result in &peer_report.results {
            if let Err(e) = db.insert(result) {
                log::error!("db insert from peer sync failed: {e}");
            }
        }

        let results = db.latest_status(1).map_err(|e| format!("db query failed: {e}"))?;
        Ok::<_, String>(PeerReport { node_id, results })
    })
    .await
    {
        Ok(Ok(report)) => Json(report).into_response(),
        Ok(Err(e)) => {
            log::error!("sync handler failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            log::error!("sync handler worker task failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn status_handler(State(state): State<AppState>) -> impl IntoResponse {
    let db = Arc::clone(&state.db);

    match tokio::task::spawn_blocking(move || {
        let db = db.lock().map_err(|e| format!("db lock poisoned: {e}"))?;
        db.latest_status(24)
            .map_err(|e| format!("db query failed: {e}"))
    })
    .await
    {
        Ok(Ok(results)) => Json(results).into_response(),
        Ok(Err(e)) => {
            log::error!("status handler failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            log::error!("status handler worker task failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn history_handler(
    State(state): State<AppState>,
    Path(host): Path<String>,
) -> Response {
    let db = Arc::clone(&state.db);

    match tokio::task::spawn_blocking(move || {
        let db = db.lock().map_err(|e| format!("db lock poisoned: {e}"))?;
        db.history(&host, 100)
            .map_err(|e| format!("db query failed: {e}"))
    })
    .await
    {
        Ok(Ok(results)) if results.is_empty() => {
            (StatusCode::NOT_FOUND, Json(results)).into_response()
        }
        Ok(Ok(results)) => Json(results).into_response(),
        Ok(Err(e)) => {
            log::error!("history handler failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
        Err(e) => {
            log::error!("history handler worker task failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}
