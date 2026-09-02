// Payables HTTP handlers (axum) — local SQLite port of WebRMS payables.
// Endpoints:
//   GET  /api/payables/suppliers        supplier list
//   GET  /api/payables/branches         non-HO branch list
//   GET  /api/payables/invoices         bills due (ap_invoices − paid_ledger)
//   GET  /api/payables/returns          supplier credits ('Z' receipts)
//   GET  /api/payables/config           supplier terms (any mode)
//   POST /api/payables/config           save one supplier's terms (HoS only)
//   POST /api/payables/config-bulk      bulk save (HoS only)
//   GET  /api/payables/paid             paid ledger
//   POST /api/payables/pay              mark rows paid (HoS only) → paid_ledger
//   POST /api/payables/export           build TSV export rows

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::modules::payables::db::{self, PaidRow};
use crate::state::{SharedState, ServerMode};
use crate::util::effective_branch;

pub fn routes() -> axum::Router<SharedState> {
    axum::Router::new()
        .route("/api/payables/suppliers", axum::routing::get(get_suppliers))
        .route("/api/payables/branches", axum::routing::get(get_branches))
        .route("/api/payables/invoices", axum::routing::get(get_invoices))
        .route("/api/payables/returns", axum::routing::get(get_returns))
        .route("/api/payables/config", axum::routing::get(get_config).post(save_config))
        .route("/api/payables/config-bulk", axum::routing::post(save_config_bulk))
        .route("/api/payables/paid", axum::routing::get(get_paid))
        .route("/api/payables/pay", axum::routing::post(mark_paid))
        .route("/api/payables/export", axum::routing::post(export_rows))
}

fn is_bos(state: &SharedState) -> bool {
    state
        .server_info
        .read()
        .map(|i| i.mode == ServerMode::Bos)
        .unwrap_or(false)
}

async fn get_suppliers(State(state): State<SharedState>) -> impl IntoResponse {
    match db::get_suppliers(&state.pool).await {
        Ok(list) => (StatusCode::OK, Json(json!(list))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn get_branches(State(state): State<SharedState>) -> impl IntoResponse {
    match db::get_branches(&state.pool).await {
        Ok(list) => (StatusCode::OK, Json(json!(list))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn get_invoices(
    State(state): State<SharedState>,
    Query(query): Query<db::InvoiceQuery>,
) -> impl IntoResponse {
    let from = query.from.clone().unwrap_or_default();
    let to = query.to.clone().unwrap_or_default();
    if from.is_empty() || to.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Missing 'from' and 'to' query parameters (YYYY-MM-DD format)" })),
        );
    }
    let branch = effective_branch(&state, query.branch.as_deref().and_then(|b| b.parse::<i32>().ok()))
        .map(|b| b as i64);
    match db::get_bills(&state.pool, &from, &to, branch, query.supplier.as_deref()).await {
        Ok(list) => (StatusCode::OK, Json(json!(list))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn get_returns(
    State(state): State<SharedState>,
    Query(query): Query<db::InvoiceQuery>,
) -> impl IntoResponse {
    let from = query.from.clone().unwrap_or_default();
    let to = query.to.clone().unwrap_or_default();
    if from.is_empty() || to.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Missing 'from' and 'to' query parameters (YYYY-MM-DD format)" })),
        );
    }
    let branch = effective_branch(&state, query.branch.as_deref().and_then(|b| b.parse::<i32>().ok()))
        .map(|b| b as i64);
    match db::get_returns(&state.pool, &from, &to, branch, query.supplier.as_deref()).await {
        Ok(list) => (StatusCode::OK, Json(json!(list))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

async fn get_config(State(state): State<SharedState>) -> impl IntoResponse {
    match db::get_config(&state.pool).await {
        Ok(list) => (StatusCode::OK, Json(json!({ "suppliers": list }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

#[derive(Deserialize)]
struct SaveOneConfig {
    supplier_code: String,
    term_type: String,
    term_days: Option<i64>,
    order_type: String,
    payment_type: String,
}

async fn save_config(
    State(state): State<SharedState>,
    Json(body): Json<SaveOneConfig>,
) -> impl IntoResponse {
    if is_bos(&state) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Payables is read-only on a BoS. Supplier terms are managed from the Head Office." })),
        );
    }
    match db::save_one(
        &state.pool,
        &body.supplier_code,
        &body.term_type,
        body.term_days,
        &body.order_type,
        &body.payment_type,
        &state.cfg.sync.install_name,
    )
    .await
    {
        Ok(()) => {
            crate::replication::notify_write(&state);
            (StatusCode::OK, Json(json!({ "status": "ok" })))
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to save config: {e}") })),
        ),
    }
}

#[derive(Deserialize)]
struct BulkSaveConfig {
    suppliers: Vec<serde_json::Value>,
}

async fn save_config_bulk(
    State(state): State<SharedState>,
    Json(body): Json<BulkSaveConfig>,
) -> impl IntoResponse {
    if is_bos(&state) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Payables is read-only on a BoS. Supplier terms are managed from the Head Office." })),
        );
    }
    let mut ok = 0usize;
    for s in &body.suppliers {
        let code = s["code"].as_str().unwrap_or("").to_string();
        if code.is_empty() {
            continue;
        }
        let term_type = s["term_type"].as_str().unwrap_or("").to_string();
        let term_days = s["term_days"].as_i64();
        let order_type = s["order_type"].as_str().unwrap_or("").to_string();
        let payment_type = s["payment_type"].as_str().unwrap_or("").to_string();
        if db::save_one(&state.pool, &code, &term_type, term_days, &order_type, &payment_type, &state.cfg.sync.install_name)
            .await
            .is_ok()
        {
            ok += 1;
        }
    }
    crate::replication::notify_write(&state);
    (StatusCode::OK, Json(json!({ "status": "ok", "saved": ok })))
}

async fn get_paid(State(state): State<SharedState>) -> impl IntoResponse {
    match db::get_paid(&state.pool).await {
        Ok(keys) => (StatusCode::OK, Json(json!({ "paid": keys }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Database error: {e}") })),
        ),
    }
}

#[derive(Deserialize)]
struct PayRequest {
    rows: Vec<PaidRow>,
}

async fn mark_paid(
    State(state): State<SharedState>,
    Json(body): Json<PayRequest>,
) -> impl IntoResponse {
    if is_bos(&state) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "Mark-as-paid is read-only on a BoS. Payment processing is managed from the Head Office." })),
        );
    }
    let rows: Vec<PaidRow> = body
        .rows
        .into_iter()
        .filter(|r| !r.invoice_number.is_empty())
        .collect();
    if rows.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "status": "error", "message": "No invoices selected to mark as paid" })),
        );
    }
    match db::mark_paid(&state.pool, &rows, &state.cfg.sync.install_name).await {
        Ok(added) => {
            crate::replication::notify_write(&state);
            (
                StatusCode::OK,
                Json(json!({ "status": "ok", "message": format!("Marked {added} invoice(s) as paid"), "rows": added })),
            )
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "status": "error", "message": format!("Failed to mark paid: {e}") })),
        ),
    }
}

#[derive(Deserialize)]
struct ExportRequest {
    rows: Vec<db::PayablesExportRow>,
}

/// TSV shape (server-save + client-download share this) — ported from
/// WebRMS build_tsv (tab-separated, sanitized).
pub fn build_tsv(rows: &[db::PayablesExportRow]) -> String {
    let mut out = String::from("Branch\tSupplier\tInvoice#\tDate\tDescription\tAmount\tTax\tPO#\tDue\n");
    for r in rows {
        let clean = |s: &str| s.replace(['\n', '\t'], " ");
        out.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\t{:.2}\t{:.2}\t{}\t{}\n",
            clean(&r.branch),
            clean(&r.supplier_code),
            clean(&r.invoice_number),
            clean(&r.invoice_date),
            clean(&r.description),
            r.invoice_amount,
            r.tax_amount,
            clean(&r.po_number),
            clean(&r.due_date),
        ));
    }
    out
}

async fn export_rows(
    State(state): State<SharedState>,
    Json(body): Json<ExportRequest>,
) -> impl IntoResponse {
    let tsv = build_tsv(&body.rows);
    match std::fs::write(state.data_dir.join("output/payables-export.tsv"), &tsv) {
        Ok(()) => (StatusCode::OK, Json(json!({ "status": "ok", "tsv": tsv }))),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("Failed to write export: {e}") })),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tsv_format_exact() {
        let rows = vec![db::PayablesExportRow {
            branch: "342".into(),
            supplier_code: "001".into(),
            invoice_number: "INV-1".into(),
            description: "Beer supply".into(),
            invoice_date: "2026-07-15".into(),
            invoice_amount: 1234.5,
            po_number: "PO-9".into(),
            tax_amount: 154.31,
            due_date: "2026-08-04".into(),
        }];
        let tsv = build_tsv(&rows);
        let expected = "Branch\tSupplier\tInvoice#\tDate\tDescription\tAmount\tTax\tPO#\tDue\n\
                        342\t001\tINV-1\t2026-07-15\tBeer supply\t1234.50\t154.31\tPO-9\t2026-08-04\n";
        assert_eq!(tsv, expected);
    }

    #[test]
    fn tsv_sanitizes_newlines_and_tabs() {
        let rows = vec![db::PayablesExportRow {
            branch: "1".into(),
            supplier_code: "2".into(),
            invoice_number: "3".into(),
            description: "line1\nline2\twith tab".into(),
            invoice_date: "2026-01-01".into(),
            invoice_amount: 1.0,
            po_number: "".into(),
            tax_amount: 0.0,
            due_date: "2026-02-01".into(),
        }];
        let tsv = build_tsv(&rows);
        assert!(tsv.contains("line1 line2 with tab"));
        assert!(tsv.contains("\n1\t2\t3\t2026-01-01\tline1 line2 with tab\t1.00\t0.00\t\t2026-02-01"));
    }

    #[test]
    fn fallback_due_eom_plus_20() {
        // invoice 2026-07-15 → EOM 2026-07-31 + 20 = 2026-08-20
        assert_eq!(db::fallback_due_date("2026-07-15"), "2026-08-20");
        // December rolls to next year
        assert_eq!(db::fallback_due_date("2026-12-10"), "2027-01-20");
    }
}
