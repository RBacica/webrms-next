use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::json;
use tower_http::services::{ServeDir, ServeFile};

use crate::db;
use crate::replication;
use crate::state::{ServerInfo, ServerMode, SharedState};

/// Build the axum router with all registered routes + the static web UI.
pub fn build_router(state: SharedState) -> Router {
    // Static UI: web/ served from the working directory (same layout as the
    // old WebRMS — run-* dirs bundle web/ next to the binary). SPA fallback
    // for / and any non-API path.
    let assets = ServeDir::new("web").not_found_service(ServeFile::new("web/index.html"));

    Router::new()
        .route("/api/health", get(health))
        .route("/api/mode", get(mode))
        .route("/api/branches", get(branches))
        .route("/api/version", get(version))
        .route("/api/sync/now", post(sync_now))
        .route("/api/sync/status", get(sync_status))
        .route("/api/sync/outbox", get(replication::outbox_route))
        .route("/api/sync/up", post(replication::sync_up_route))
        .route("/api/sync/snapshot", get(sync_snapshot))
        .merge(crate::modules::stocktake::handlers::routes())
        .merge(crate::modules::ordering::handlers::routes())
        .merge(crate::modules::payables::handlers::routes())
        .merge(crate::modules::promotions::handlers::routes())
        .merge(crate::modules::reports::handlers::routes())
        .fallback_service(assets)
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
    let db_ok = db::ping(&state.pool_arc()).await;
    let info = info(&state);
    let poll = state.poller.read().map(|p| p.as_ref().map(|h| h.status())).unwrap_or(None);
    let (connector, last_poll, connector_age_secs) = match &poll {
        Some(st) => {
            let age = st.last_success.as_deref().and_then(|ts| {
                chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S")
                    .ok()
                    .map(|t| chrono::Local::now().naive_local().signed_duration_since(t).num_seconds().max(0))
            });
            (
                if st.last_error.is_some() { "error".to_string() } else { "ok".to_string() },
                st.last_success.clone(),
                age,
            )
        }
        None => ("disabled".to_string(), None, None),
    };
    let fb = crate::fallback::read_state(&state.data_dir);
    // D3: replication lag for sync clients — age of the last successful pull
    // from the source (sync_watermarks). The HoS is the source → disabled.
    let replication = if state.cfg.sync.enabled && !state.cfg.sync.source.is_empty() {
        let pool = state.pool_arc();
        match sqlx::query_scalar::<_, String>(
            "SELECT MAX(updated_at) FROM sync_watermarks WHERE source = ?1",
        )
        .bind(&state.cfg.sync.source)
        .fetch_optional(&*pool)
        .await
        {
            Ok(Some(ts)) => {
                let mins = chrono::NaiveDateTime::parse_from_str(&ts, "%Y-%m-%d %H:%M:%S")
                    .map(|t| chrono::Local::now().naive_local().signed_duration_since(t).num_minutes().max(0))
                    .unwrap_or(-1);
                json!({ "role": "client", "last_pull": ts, "lag_minutes": mins })
            }
            _ => json!({ "role": "client", "last_pull": null, "lag_minutes": null }),
        }
    } else {
        json!({ "role": "source", "last_pull": null, "lag_minutes": null })
    };
    (
        StatusCode::OK,
        Json(json!({
            "status": if db_ok { "ok" } else { "degraded" },
            "db": db_ok,
            "mode": info.mode.as_str(),
            "connector": connector,
            "last_poll": last_poll,
            "connector_age_secs": connector_age_secs,
            "fallback": {
                "enabled": state.cfg.sync.fallback_enabled,
                "engaged": fb.engaged,
                "restored_at": fb.restored_at,
                "recovered_at": fb.recovered_at,
                "via": fb.via,
                "size_bytes": fb.size_bytes,
                "last_attempt": fb.last_attempt,
                "last_error": fb.last_error,
                "attempts": fb.attempts,
            },
            "snapshot": if fb.engaged { "fallback-active" } else { "available" },
            "replication": replication,
            "version": env!("CARGO_PKG_VERSION"),
        })),
    )
}

async fn mode(State(state): State<SharedState>) -> impl IntoResponse {
    let info = info(&state);
    let fb = crate::fallback::read_state(&state.data_dir);
    (StatusCode::OK, Json(json!({
        "mode": info.mode.as_str(),
        "branch_id": info.branch_id,
        "db_ok": info.db_ok,
        "version": info.version,
        "author": state.config_author(),
        "remote_hos": state.is_remote_hos(),
        "fallback_engaged": fb.engaged,
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

/// GET /api/sync/snapshot — stream the HoS's gzip'd DB snapshot (O-5).
async fn sync_snapshot(State(state): State<SharedState>) -> impl IntoResponse {
    let key = state.cfg.sync.snapshot_key.clone();
    let data_dir = state.data_dir.clone();
    let pool = state.pool_arc();
    match crate::snapshot::build_snapshot(&pool, &data_dir, &key).await {
        Ok((path, sig)) => match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let mut headers = axum::http::HeaderMap::new();
                if !sig.is_empty() {
                    if let Ok(v) = axum::http::HeaderValue::from_str(&sig) {
                        headers.insert(crate::snapshot::SIG_HEADER, v);
                    }
                }
                if let Ok(v) = axum::http::HeaderValue::from_str("application/gzip") {
                    headers.insert(axum::http::header::CONTENT_TYPE, v);
                }
                (
                    StatusCode::OK,
                    headers,
                    axum::body::Body::from(bytes),
                )
            }
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::http::HeaderMap::new(),
                axum::body::Body::from(format!("{{ \"error\": \"read snapshot: {e}\" }}")),
            ),
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::http::HeaderMap::new(),
            axum::body::Body::from(format!("{{ \"error\": \"build snapshot: {e}\" }}")),
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
    .fetch_all(&*state.pool_arc())
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
