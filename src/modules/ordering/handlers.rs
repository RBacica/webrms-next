// Ordering HTTP handlers (axum) — ported from WebRMS ordering module.
// Endpoints:
//   GET  /api/ordering/suppliers            supplier list (CustType='R')
//   GET  /api/ordering/sheet?supplier=&branch=&active_only=   order sheet
//   POST /api/ordering/orders               { supplier, branch, lines[], by } → PO ETL + incoming PO
//   GET  /api/ordering/orders?branch=       order log (lifecycle G-10)
//   GET  /api/ordering/confirmation-csv?order_id=   supplier confirmation CSV (G-5)
//   GET  /api/sync/incoming-po              incoming PO list with status

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::modules::ordering::{db, etl, orders};
use crate::state::SharedState;
use crate::util::effective_branch;

pub fn routes() -> axum::Router<SharedState> {
    axum::Router::new()
        .route("/api/ordering/suppliers", axum::routing::get(suppliers))
        .route("/api/ordering/sheet", axum::routing::get(sheet))
        .route("/api/ordering/orders", axum::routing::get(list_orders).post(post_order))
        .route("/api/ordering/confirmation-csv", axum::routing::get(confirmation_csv))
        .route("/api/sync/incoming-po", axum::routing::get(incoming_po).delete(delete_incoming_po))
}

#[derive(Deserialize)]
struct SheetQuery {
    supplier: Option<String>,
    branch: Option<i32>,
    #[serde(default)]
    active_only: bool,
}

#[derive(Deserialize)]
struct PostOrderBody {
    supplier: String,
    branch: Option<i64>,
    #[serde(default)]
    lines: Vec<PostLine>,
    #[serde(default)]
    by: Option<String>,
    #[serde(default)]
    ext_ref: Option<String>,
}

#[derive(Deserialize, Clone)]
struct PostLine {
    upc: String,
    qty: f64,
    #[serde(default)]
    unit_cost: f64,
    #[serde(default)]
    suggested_qty: f64,
}

async fn suppliers(State(state): State<SharedState>) -> impl IntoResponse {
    match sqlx::query_as::<_, (String, String)>(
        "SELECT code, name FROM suppliers WHERE is_active = 1 ORDER BY code",
    )
    .fetch_all(&*state.pool_arc())
    .await
    {
        Ok(rows) => (
            StatusCode::OK,
            Json(json!({ "suppliers": rows.into_iter().map(|(code, name)| json!({"code": code, "name": name})).collect::<Vec<_>>() })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn sheet(
    State(state): State<SharedState>,
    Query(query): Query<SheetQuery>,
) -> impl IntoResponse {
    let supplier = query.supplier.clone().unwrap_or_default();
    if supplier.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Missing supplier query parameter" })),
        );
    }
    let branch = effective_branch(&state, query.branch);
    match db::order_sheet(&*state.pool_arc(), &supplier, branch.map(|b| b as i64), query.active_only).await {
        Ok(lines) => (
            StatusCode::OK,
            Json(json!({ "supplier": supplier, "branch": branch, "lines": lines })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

/// Post an order: persist order + lines, generate the ETL PO .xlsx, write it to
/// the incoming-PO tree, record the BillOfLading + status (W5).
async fn post_order(
    State(state): State<SharedState>,
    Json(body): Json<PostOrderBody>,
) -> impl IntoResponse {
    if body.supplier.is_empty() || body.lines.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "supplier and at least one line are required" })),
        );
    }
    let branch = body.branch.unwrap_or(0);

    // 1. persist order (G-10 lifecycle: status 'open'); resolve supplier id
    let sup_id: Option<i64> = sqlx::query_scalar("SELECT id FROM suppliers WHERE code = ?1")
        .bind(&body.supplier)
        .fetch_optional(&*state.pool_arc())
        .await
        .unwrap_or(None);
    let lines: Vec<(String, f64, f64, f64)> = body
        .lines
        .iter()
        .map(|l| (l.upc.clone(), l.qty, l.unit_cost, l.suggested_qty))
        .collect();
    let order_id = match orders::create_order(
        &*state.pool_arc(),
        &state.cfg.sync.install_name,
        branch,
        sup_id,
        body.by.as_deref(),
        &lines,
    )
    .await
    {
        Ok(id) => id,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Database error: {e}") })),
            );
        }
    };

    // 2. generate the ETL PO file (compact H/D blocks, fresh BillOfLading)
    let bol = etl::generate_bill_of_lading();
    let poid = chrono::Local::now().timestamp();
    let etl_lines: Vec<etl::EtlPoLine> = body
        .lines
        .iter()
        .map(|l| etl::EtlPoLine { upc: l.upc.clone(), qty: l.qty, cost: l.unit_cost })
        .collect();
    let bytes = match etl::build_purchase_order_xlsx(
        poid,
        &body.supplier,
        branch,
        body.ext_ref.as_deref(),
        body.by.as_deref(),
        &bol,
        &etl_lines,
    ) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("ETL build failed: {e}") })),
            );
        }
    };

    // 3. write into incoming-po/<branch>/ + record tracking row
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let filename = format!("PurchaseOrder-{}-{}-{}-{}.xlsx", body.supplier, branch, ts, bol);
    let dir = state.data_dir.join("output/incoming-po").join(branch.to_string());
    std::fs::create_dir_all(&dir).ok();
    let file_path = dir.join(&filename);
    if let Err(e) = std::fs::write(&file_path, &bytes) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to write PO file: {e}") })),
        );
    }
    let placed_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let install = state.cfg.sync.install_name.clone();
    let incoming_id = uuid::Uuid::new_v4().to_string();
    let mut tx = match state.pool_arc().begin().await {
        Ok(tx) => tx,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("Failed to begin tx: {e}") }))),
    };
    if let Err(e) = sqlx::query(
        "INSERT INTO incoming_pos (id, origin_install, branch_id, supplier_id, filename, \
                bill_of_lading, poid, status, imported, placed_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'waiting_import', 0, ?8)",
    )
    .bind(&incoming_id)
    .bind(&install)
    .bind(branch)
    .bind(sup_id)
    .bind(&filename)
    .bind(&bol)
    .bind(poid)
    .bind(&placed_at)
    .execute(&mut *tx)
    .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to record incoming PO: {e}") })),
        );
    }
    // outbox: replicate the incoming PO up to the HoS
    let payload = serde_json::json!({
        "id": incoming_id, "origin_install": install, "branch_id": branch,
        "supplier_id": sup_id, "filename": filename, "bill_of_lading": bol,
        "poid": poid, "status": "waiting_import", "imported": 0, "placed_at": placed_at,
    });
    if let Err(e) = crate::replication::emit(&mut tx, &install, "incoming_pos", &incoming_id, "insert", &payload).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to emit outbox: {e}") })),
        );
    }
    if let Err(e) = tx.commit().await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to commit: {e}") })),
        );
    }
    crate::replication::notify_write(&state);

    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "order_id": order_id,
            "po_file": filename,
            "bill_of_lading": bol,
            "poid": poid,
            "status": "waiting_import",
            "tracked_incoming": true,
        })),
    )
}

#[derive(Deserialize)]
struct ListOrdersQuery {
    branch: Option<i64>,
    status: Option<String>,
}

async fn list_orders(
    State(state): State<SharedState>,
    Query(query): Query<ListOrdersQuery>,
) -> impl IntoResponse {
    match orders::list_orders(&*state.pool_arc(), query.branch, query.status.as_deref()).await {
        Ok(list) => (StatusCode::OK, Json(json!({ "orders": list }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

/// Supplier confirmation CSV — minimal supplier-facing columns (G-5, cf87634):
/// Supplier,UPC,Description,Pack,OrderQty
#[derive(Deserialize)]
struct ConfirmQuery {
    order_id: String,
}

async fn confirmation_csv(
    State(state): State<SharedState>,
    Query(query): Query<ConfirmQuery>,
) -> impl IntoResponse {
    let order = match orders::list_orders(&*state.pool_arc(), None, None).await {
        Ok(list) => list.into_iter().find(|o| o.id == query.order_id),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": format!("Database error: {e}") })),
            );
        }
    };
    let Some(order) = order else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "order not found" })),
        );
    };
    let supplier = order.supplier_code.unwrap_or_default();
    let mut csv = String::from("Supplier,UPC,Description,Pack,OrderQty\n");
    for l in &order.lines {
        let (desc, pack) = sqlx::query_as::<_, (String, f64)>(
            "SELECT COALESCE(description, ''), COALESCE(pack_units, 1) FROM items WHERE upc = ?1",
        )
        .bind(&l.upc)
        .fetch_one(&*state.pool_arc())
        .await
        .map(|(d, p)| (d, p))
        .unwrap_or_default();
        let desc = desc.replace(',', " ").replace('\n', " ");
        csv.push_str(&format!("{},{},{},{},{}\n", supplier, l.upc, desc, pack, l.qty));
    }
    (
        StatusCode::OK,
        Json(json!({
            "filename": format!("order-confirmation-{}.csv", order.id),
            "content": csv,
        })),
    )
}

/// Incoming PO list with status (W5): files tracked in incoming_pos.
async fn incoming_po(State(state): State<SharedState>) -> impl IntoResponse {
    match crate::modules::incoming_po::list(&*state.pool_arc()).await {
        Ok(files) => (StatusCode::OK, Json(json!({ "incoming": files }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

/// Delete an incoming-PO record + file (HoS only — BoS/Remote-HoS 403).
#[derive(Deserialize)]
struct DeletePoQuery {
    filename: String,
}

async fn delete_incoming_po(
    State(state): State<SharedState>,
    Query(query): Query<DeletePoQuery>,
) -> impl IntoResponse {
    let is_hos = state
        .server_info
        .read()
        .map(|i| i.mode == crate::state::ServerMode::Hos)
        .unwrap_or(false);
    if !is_hos {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Incoming-PO deletion is Head Office only." })),
        );
    }
    // delete the physical file from output/incoming-po/<branch>/ if present
    if let Ok(dir) = std::fs::read_dir(state.data_dir.join("output/incoming-po")) {
        for branch_dir in dir.filter_map(|e| e.ok()) {
            let f = branch_dir.path().join(&query.filename);
            if f.exists() {
                std::fs::remove_file(&f).ok();
                break;
            }
        }
    }
    match crate::modules::incoming_po::remove(&*state.pool_arc(), &query.filename).await {
        Ok(n) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "removed": n, "filename": query.filename })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}
