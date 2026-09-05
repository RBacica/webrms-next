// Stocktake HTTP handlers (axum) — ported from WebRMS actix handlers.
// Endpoints:
//   GET  /api/stocktake/departments
//   GET  /api/stocktake/suppliers
//   GET  /api/stocktake/suppliers-for-dept?department=
//   GET  /api/stocktake/sub-departments?department=
//   GET  /api/stocktake/search?department=&supplier=&sub_department=&branch=
//   GET  /api/stocktake/refresh-upc?upc=&branch=
//   GET  /api/stocktake/barcode-lookup?barcode=&branch=
//   POST /api/stocktake/export  { rows, destination: "server"|"client" }

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

use crate::modules::stocktake::{db, exports};
use crate::state::SharedState;
use crate::util::effective_branch;

pub fn routes() -> axum::Router<SharedState> {
    axum::Router::new()
        .route("/api/stocktake/departments", axum::routing::get(get_departments))
        .route("/api/stocktake/suppliers", axum::routing::get(get_suppliers))
        .route("/api/stocktake/suppliers-for-dept", axum::routing::get(get_suppliers_for_dept))
        .route("/api/stocktake/sub-departments", axum::routing::get(get_sub_departments))
        .route("/api/stocktake/search", axum::routing::get(search_items))
        .route("/api/stocktake/refresh-upc", axum::routing::get(refresh_upc))
        .route("/api/stocktake/barcode-lookup", axum::routing::get(barcode_lookup))
        .route("/api/stocktake/export", axum::routing::post(export_rows))
}

async fn get_departments(State(state): State<SharedState>) -> impl IntoResponse {
    match db::get_departments(&*state.pool_arc()).await {
        Ok(list) => (StatusCode::OK, Json(json!(list))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn get_suppliers(State(state): State<SharedState>) -> impl IntoResponse {
    match db::get_suppliers(&*state.pool_arc()).await {
        Ok(list) => (StatusCode::OK, Json(json!(list))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn get_suppliers_for_dept(
    State(state): State<SharedState>,
    Query(query): Query<db::DeptQuery>,
) -> impl IntoResponse {
    let dept = query.department.clone().unwrap_or_else(|| "ALL".to_string());
    match db::get_suppliers_for_department(&*state.pool_arc(), &dept).await {
        Ok(list) => (StatusCode::OK, Json(json!(list))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn get_sub_departments(
    State(state): State<SharedState>,
    Query(query): Query<db::SubDeptQuery>,
) -> impl IntoResponse {
    let dept = query.department.clone().unwrap_or_default();
    match db::get_sub_departments(&*state.pool_arc(), &dept).await {
        Ok(subs) => (StatusCode::OK, Json(json!(subs))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn search_items(
    State(state): State<SharedState>,
    Query(query): Query<db::SearchQuery>,
) -> impl IntoResponse {
    let dept = query.department.clone().unwrap_or_else(|| "ALL".to_string());
    let sup = query.supplier.clone().unwrap_or_else(|| "ALL".to_string());
    let sub_dept = query.sub_department.clone().unwrap_or_else(|| "ALL".to_string());
    let branch = effective_branch(&state, query.branch);

    match db::search_items(&*state.pool_arc(), &dept, &sup, &sub_dept, branch).await {
        Ok(items) => (
            StatusCode::OK,
            Json(json!({ "items": items, "branch": branch })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn refresh_upc(
    State(state): State<SharedState>,
    Query(query): Query<db::UpcQuery>,
) -> impl IntoResponse {
    let upc = query.upc.clone().unwrap_or_default();
    if upc.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Missing upc query parameter" })),
        );
    }
    let branch = effective_branch(&state, query.branch);
    match db::refresh_upc(&*state.pool_arc(), &upc, branch).await {
        Ok(stock_on_hand) => (
            StatusCode::OK,
            Json(json!({ "upc": upc, "stock_on_hand": stock_on_hand })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn barcode_lookup(
    State(state): State<SharedState>,
    Query(query): Query<db::BarcodeQuery>,
) -> impl IntoResponse {
    let barcode = query.barcode.trim().to_string();
    if barcode.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Missing barcode query parameter" })),
        );
    }
    let branch = effective_branch(&state, query.branch);
    match db::barcode_lookup_upcs(&*state.pool_arc(), &barcode).await {
        Ok(upcs) => (
            StatusCode::OK,
            Json(json!({ "upcs": upcs, "branch": branch })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn export_rows(
    State(state): State<SharedState>,
    Json(req): Json<db::SaveRequest>,
) -> impl IntoResponse {
    if req.rows.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "status": "error", "message": "No rows to export" })),
        );
    }
    let destination = req.destination.trim().to_lowercase();
    if !matches!(destination.as_str(), "server" | "client") {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "status": "error", "message": "destination must be 'server' or 'client'" })),
        );
    }

    let result = match exports::export(&state.data_dir, &req.rows) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("export failed: {e}");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "status": "error", "message": format!("Failed to export: {e}") })),
            );
        }
    };
    let count_rows = result.count_rows;
    let ticket_rows = result.ticket_rows;

    // Record the run (stocktake_runs) so the Reports → Stocktake & Shrink page
    // has export history. Costed variance (G-3) is a documented open item.
    let _ = record_run(&state, &result, &req).await;

    if destination == "client" {
        let mut files_out: Vec<serde_json::Value> = Vec::new();
        if let Some(p) = &result.count_path {
            let bytes = std::fs::read(p).unwrap_or_default();
            files_out.push(json!({
                "filename": format!("stocktake-{}.txt", result.timestamp),
                "content": String::from_utf8_lossy(&bytes),
            }));
        }
        if let Some(p) = &result.ticket_path {
            let bytes = std::fs::read(p).unwrap_or_default();
            files_out.push(json!({
                "filename": format!("tickets-{}.qry", result.timestamp.replace('-', "")),
                "content": String::from_utf8_lossy(&bytes),
            }));
        }
        return (
            StatusCode::OK,
            Json(json!({
                "status": "ok",
                "destination": "client",
                "files": files_out,
                "count_rows": count_rows,
                "ticket_rows": ticket_rows,
            })),
        );
    }

    (
        StatusCode::OK,
        Json(json!({
            "status": "ok",
            "destination": "server",
            "count_file": result.count_path,
            "ticket_file": result.ticket_path,
            "count_rows": count_rows,
            "ticket_rows": ticket_rows,
        })),
    )
}

/// Insert a stocktake_runs row after a successful export (branch from the
/// request's branch field when present; best-effort).
async fn record_run(
    state: &crate::state::SharedState,
    result: &exports::ExportResult,
    req: &db::SaveRequest,
) -> anyhow::Result<()> {
    use uuid::Uuid;
    // result.timestamp = "YYYY-MM-DD-HH-MM-SS" → store "YYYY-MM-DD HH:MM:SS"
    let started_db = chrono::NaiveDateTime::parse_from_str(&result.timestamp, "%Y-%m-%d-%H-%M-%S")
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|_| result.timestamp.clone());
    let branch_id = req.branch.map(|b| b as i64);
    sqlx::query(
        "INSERT INTO stocktake_runs (id, branch_id, started_at, status, count_file, ticket_file) \
         VALUES (?1, ?2, ?3, 'exported', ?4, ?5)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(branch_id)
    .bind(&started_db)
    .bind(result.count_path.clone())
    .bind(result.ticket_path.clone())
    .execute(&*state.pool_arc())
    .await?;
    Ok(())
}
