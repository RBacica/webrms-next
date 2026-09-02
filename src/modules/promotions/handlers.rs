// Promotions HTTP handlers — READ-ONLY everywhere (port of WebRMS).
//
// Endpoints:
//   GET /api/promotions/engine   -> { engine: "Rules_Based"|"Standard" }
//   GET /api/promotions/list     ?branch=
//   GET /api/promotions/items    ?id=&branch=
//   GET /api/promotions/effectiveness ?from&to&branch

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::modules::promotions::db;
use crate::state::SharedState;
use crate::util::effective_branch;

pub fn routes() -> axum::Router<SharedState> {
    axum::Router::new()
        .route("/api/promotions/engine", axum::routing::get(get_engine))
        .route("/api/promotions/list", axum::routing::get(list_promotions))
        .route("/api/promotions/items", axum::routing::get(promotion_items))
        .route("/api/promotions/effectiveness", axum::routing::get(effectiveness))
}

#[derive(Deserialize)]
struct PromoQuery {
    branch: Option<i32>,
    from: Option<String>,
    to: Option<String>,
    id: Option<String>,
}

async fn get_engine(State(state): State<SharedState>) -> impl IntoResponse {
    let engine = db::pricing_engine(&state.pool).await;
    (
        StatusCode::OK,
        Json(json!({ "engine": engine, "read_only": true })),
    )
}

async fn list_promotions(
    State(state): State<SharedState>,
    Query(query): Query<PromoQuery>,
) -> impl IntoResponse {
    let branch = effective_branch(&state, query.branch);
    match db::list_promotions(&state.pool, branch).await {
        Ok(list) => {
            let engine = db::pricing_engine(&state.pool).await;
            let (active, inactive): (Vec<_>, Vec<_>) =
                list.iter().partition(|p| p.active);
            (
                StatusCode::OK,
                Json(json!({
                    "engine": engine,
                    "total": list.len(),
                    "active": active.len(),
                    "inactive": inactive.len(),
                    "promotions": list,
                })),
            )
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn promotion_items(
    State(state): State<SharedState>,
    Query(query): Query<PromoQuery>,
) -> impl IntoResponse {
    let Some(id) = query.id.clone() else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Missing 'id' query parameter (the promotion id from the list)" })),
        );
    };
    if id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Missing 'id' query parameter (the promotion id from the list)" })),
        );
    }
    let branch = effective_branch(&state, query.branch);
    match db::promotion_items(&state.pool, &id, branch).await {
        Ok(items) => (StatusCode::OK, Json(json!(items))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn effectiveness(
    State(state): State<SharedState>,
    Query(query): Query<PromoQuery>,
) -> impl IntoResponse {
    let from = query.from.clone().unwrap_or_default();
    let to = query.to.clone().unwrap_or_default();
    let branch = effective_branch(&state, query.branch);
    if from.is_empty() || to.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Missing 'from' and 'to' query parameters (YYYY-MM-DD format)" })),
        );
    }
    match db::promotion_effectiveness(&state.pool, &from, &to, branch).await {
        Ok(rows) => (
            StatusCode::OK,
            Json(json!({
                "engine": db::pricing_engine(&state.pool).await,
                "specials": rows,
            })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}
