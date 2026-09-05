// Reports HTTP handlers (axum) — local SQLite port of WebRMS reports.
// Endpoints:
//   GET /api/reports/daily       ?from&to&branch
//   GET /api/reports/depts       ?from&to&branch&dept
//   GET /api/reports/overview    ?branch
//   GET /api/reports/overview/movers     ?from&to&branch&limit
//   GET /api/reports/overview/dept-movers ?from&to&branch
//   GET /api/reports/overview/dept-weekly ?branch

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::modules::reports::db;
use crate::state::SharedState;
use crate::util::effective_branch;

pub fn routes() -> axum::Router<SharedState> {
    axum::Router::new()
        .route("/api/reports/daily", axum::routing::get(get_daily))
        .route("/api/reports/depts", axum::routing::get(get_depts))
        .route("/api/reports/overview", axum::routing::get(get_overview))
        .route("/api/reports/overview/movers", axum::routing::get(get_movers))
        .route("/api/reports/overview/dept-movers", axum::routing::get(get_dept_movers))
        .route("/api/reports/overview/dept-weekly", axum::routing::get(get_dept_weekly))
        .route("/api/reports/stock", axum::routing::get(get_stock))
        .route("/api/reports/receipts", axum::routing::get(get_receipts))
        .route("/api/reports/stocktakes", axum::routing::get(get_stocktakes))
        .route("/api/reports/payments", axum::routing::get(get_payments))
        .route("/api/reports/hourly", axum::routing::get(get_hourly))
        .route("/api/reports/promo-summary", axum::routing::get(get_promo_summary))
}

#[derive(Deserialize)]
struct ReportQuery {
    from: Option<String>,
    to: Option<String>,
    branch: Option<i32>,
    dept: Option<i32>,
    limit: Option<i64>,
}

fn bad(msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg })))
}

async fn get_daily(
    State(state): State<SharedState>,
    Query(query): Query<ReportQuery>,
) -> impl IntoResponse {
    let (Some(from), Some(to)) = (query.from.clone(), query.to.clone()) else {
        return bad("Missing 'from' and 'to' query parameters (YYYY-MM-DD format)");
    };
    let branch = effective_branch(&state, query.branch).map(|b| b as i64);
    match db::daily_summary(&*state.pool_arc(), &from, &to, branch).await {
        Ok(report) => (StatusCode::OK, Json(json!(report))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn get_depts(
    State(state): State<SharedState>,
    Query(query): Query<ReportQuery>,
) -> impl IntoResponse {
    let (Some(from), Some(to)) = (query.from.clone(), query.to.clone()) else {
        return bad("Missing 'from' and 'to' query parameters (YYYY-MM-DD format)");
    };
    let branch = effective_branch(&state, query.branch).map(|b| b as i64);
    match db::dept_sales(&*state.pool_arc(), &from, &to, branch, query.dept.map(|d| d as i64)).await {
        Ok(depts) => (StatusCode::OK, Json(json!(depts))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn get_overview(
    State(state): State<SharedState>,
    Query(query): Query<ReportQuery>,
) -> impl IntoResponse {
    let branch = effective_branch(&state, query.branch).map(|b| b as i64);
    match db::overview(&*state.pool_arc(), branch).await {
        Ok(overview) => (StatusCode::OK, Json(json!(overview))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn get_movers(
    State(state): State<SharedState>,
    Query(query): Query<ReportQuery>,
) -> impl IntoResponse {
    let (Some(from), Some(to)) = (query.from.clone(), query.to.clone()) else {
        return bad("Missing 'from' and 'to' query parameters (YYYY-MM-DD format)");
    };
    let branch = effective_branch(&state, query.branch).map(|b| b as i64);
    match db::top_movers(&*state.pool_arc(), &from, &to, branch, query.dept.map(|d| d as i64), query.limit.unwrap_or(20)).await {
        Ok(movers) => (StatusCode::OK, Json(json!(movers))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn get_dept_movers(
    State(state): State<SharedState>,
    Query(query): Query<ReportQuery>,
) -> impl IntoResponse {
    let (Some(from), Some(to)) = (query.from.clone(), query.to.clone()) else {
        return bad("Missing 'from' and 'to' query parameters (YYYY-MM-DD format)");
    };
    let branch = effective_branch(&state, query.branch).map(|b| b as i64);
    match db::dept_movers(&*state.pool_arc(), &from, &to, branch).await {
        Ok(movers) => (StatusCode::OK, Json(json!(movers))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn get_dept_weekly(
    State(state): State<SharedState>,
    Query(query): Query<ReportQuery>,
) -> impl IntoResponse {
    let branch = effective_branch(&state, query.branch).map(|b| b as i64);
    match db::dept_weekly(&*state.pool_arc(), branch).await {
        Ok(rows) => (StatusCode::OK, Json(json!(rows))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

// ── R-parity handlers: stock valuation / GRN↔AP receipts / stocktakes ────────

async fn get_stock(
    State(state): State<SharedState>,
    Query(query): Query<ReportQuery>,
) -> impl IntoResponse {
    // BoS is locked to its local branch; HoS: ?branch= or omitted = ALL.
    let branch = effective_branch(&state, query.branch).map(|b| b as i64);
    match db::stock_valuation(&*state.pool_arc(), branch).await {
        Ok(depts) => (StatusCode::OK, Json(json!(depts))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn get_receipts(
    State(state): State<SharedState>,
    Query(query): Query<ReportQuery>,
) -> impl IntoResponse {
    let (Some(from), Some(to)) = (query.from.clone(), query.to.clone()) else {
        return bad("Missing 'from' and 'to' query parameters (YYYY-MM-DD format)");
    };
    let branch = effective_branch(&state, query.branch).map(|b| b as i64);
    match db::receipts_report(&*state.pool_arc(), &from, &to, branch).await {
        Ok(rows) => (StatusCode::OK, Json(json!(rows))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn get_stocktakes(State(state): State<SharedState>) -> impl IntoResponse {
    match db::stocktakes_report(&*state.pool_arc()).await {
        Ok(rows) => (StatusCode::OK, Json(json!(rows))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

// ── Payment Mix + Hourly Curve ───────────────────────────────────────────────

async fn get_payments(
    State(state): State<SharedState>,
    Query(query): Query<ReportQuery>,
) -> impl IntoResponse {
    let (Some(from), Some(to)) = (query.from.clone(), query.to.clone()) else {
        return bad("Missing 'from' and 'to' query parameters (YYYY-MM-DD format)");
    };
    let branch = effective_branch(&state, query.branch).map(|b| b as i64);
    match db::payment_mix(&*state.pool_arc(), &from, &to, branch).await {
        Ok(rows) => (StatusCode::OK, Json(json!(rows))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("Database error: {e}") }))),
    }
}

async fn get_hourly(
    State(state): State<SharedState>,
    Query(query): Query<ReportQuery>,
) -> impl IntoResponse {
    let (Some(from), Some(to)) = (query.from.clone(), query.to.clone()) else {
        return bad("Missing 'from' and 'to' query parameters (YYYY-MM-DD format)");
    };
    let branch = effective_branch(&state, query.branch).map(|b| b as i64);
    match db::hourly_curve(&*state.pool_arc(), &from, &to, branch).await {
        Ok(rows) => (StatusCode::OK, Json(json!(rows))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("Database error: {e}") }))),
    }
}

async fn get_promo_summary(
    State(state): State<SharedState>,
    Query(query): Query<ReportQuery>,
) -> impl IntoResponse {
    let (Some(from), Some(to)) = (query.from.clone(), query.to.clone()) else {
        return bad("Missing 'from' and 'to' query parameters (YYYY-MM-DD format)");
    };
    match db::promo_summary(&*state.pool_arc(), &from, &to).await {
        Ok(s) => (StatusCode::OK, Json(json!(s))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}
