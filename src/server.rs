use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;

use crate::db;
use crate::state::{ServerInfo, ServerMode, SharedState};

/// Build the axum router with all registered routes.
pub fn build_router(state: SharedState) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/mode", get(mode))
        .route("/api/branches", get(branches))
        .route("/api/version", get(version))
        .route("/api/sync/now", post(sync_now))
        .route("/api/sync/status", get(sync_status))
        .merge(crate::modules::stocktake::handlers::routes())
        .with_state(state)
}

/// Read server info defensively (a poisoned lock falls back to a safe default).
fn info(state: &SharedState) -> ServerInfo {
    state
        .server_info
        .read()
        .map(|i| i.clone())
        .unwrap_or_else(|_| ServerInfo {
            mode: ServerMode::Standalone,
            branch_id: None,
            db_ok: false,
            version: env!("CARGO_PKG_VERSION").to_string(),
        })
}

async fn health(State(state): State<SharedState>) -> impl IntoResponse {
    let db_ok = db::ping(&state.pool).await;
    let info = info(&state);
    let poll = state.poller.read().map(|p| p.as_ref().map(|h| h.status())).unwrap_or(None);
    let (connector, last_poll) = match &poll {
        Some(st) => (
            if st.last_error.is_some() { "error" } else { "ok" }.to_string(),
            st.last_success.clone(),
        ),
        None => ("disabled".to_string(), None),
    };
    (
        StatusCode::OK,
        Json(json!({
            "status": if db_ok { "ok" } else { "degraded" },
            "db": db_ok,
            "mode": info.mode.as_str(),
            "connector": connector,
            "last_poll": last_poll,
            "snapshot": "n/a",      // P3: snapshot staleness
            "replication": "n/a",    // P3: replication lag
            "version": env!("CARGO_PKG_VERSION"),
        })),
    )
}

async fn mode(State(state): State<SharedState>) -> impl IntoResponse {
    let info = info(&state);
    (StatusCode::OK, Json(json!({
        "mode": info.mode.as_str(),
        "branch_id": info.branch_id,
        "db_ok": info.db_ok,
        "version": info.version,
        "author": state.config_author(),
        "remote_hos": state.is_remote_hos(),
    })))
}

async fn version() -> impl IntoResponse {
    Json(json!({
        "name": "webrms-next",
        "version": env!("CARGO_PKG_VERSION"),
        "schema": crate::db::MIGRATOR.iter().count(),
    }))
}

/// Manual "Sync now" — trigger a connector tick immediately (O-2).
async fn sync_now(State(state): State<SharedState>) -> impl IntoResponse {
    let handle = state.poller.read().map(|p| p.clone()).unwrap_or(None);
    match handle {
        Some(h) => match h.tick_now().await {
            Ok(()) => (
                StatusCode::OK,
                Json(json!({ "ok": true, "message": "sync tick completed" })),
            ),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "ok": false, "error": format!("{e}") })),
            ),
        },
        None => (
            StatusCode::CONFLICT,
            Json(json!({ "ok": false, "error": "connector not configured (standalone mode)" })),
        ),
    }
}

/// Connector poll status (for UI badges + ops).
async fn sync_status(State(state): State<SharedState>) -> impl IntoResponse {
    let handle = state.poller.read().map(|p| p.clone()).unwrap_or(None);
    match handle {
        Some(h) => {
            let st = h.status();
            (
                StatusCode::OK,
                Json(json!({
                    "enabled": st.connector_enabled,
                    "tick_count": st.tick_count,
                    "last_success": st.last_success,
                    "last_error": st.last_error,
                    "last_items": st.last_items,
                    "last_sales": st.last_sales,
                })),
            )
        }
        None => (
            StatusCode::OK,
            Json(json!({ "enabled": false, "tick_count": 0 })),
        ),
    }
}

async fn branches(State(state): State<SharedState>) -> impl IntoResponse {
    match sqlx::query_as::<_, (i64, String, Option<i64>, i64)>(
        "SELECT id, name, ext_key, is_ho FROM branches WHERE is_active = 1 ORDER BY id",
    )
    .fetch_all(&state.pool)
    .await
    {
        Ok(rows) => {
            let list: Vec<_> = rows
                .into_iter()
                .map(|(id, name, ext, ho)| json!({ "id": id, "name": name, "ext_key": ext, "is_ho": ho == 1 }))
                .collect();
            (StatusCode::OK, Json(json!({ "branches": list })))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "ok": false, "error": format!("{e}") })),
        ),
    }
}
