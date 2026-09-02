use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use webrms_next::config::AppConfig;
use webrms_next::state::AppState;

/// Boot a scratch install in a temp dir and return the router.
async fn test_app() -> (axum::Router, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pool = webrms_next::db::init_pool(tmp.path()).await.expect("pool");
    let mut cfg = AppConfig::default();
    cfg.role.mode = "hos".into();
    let state = AppState::new(pool, cfg, tmp.path().to_path_buf());
    (webrms_next::build_app(state), tmp)
}

async fn get(router: &axum::Router, uri: &str) -> (StatusCode, String) {
    let resp = router
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, String::from_utf8_lossy(&body).to_string())
}

#[tokio::test]
async fn mode_endpoint_returns_hos() {
    let (app, _tmp) = test_app().await;
    let (status, body) = get(&app, "/api/mode").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"mode\":\"hos\""), "body: {body}");
    assert!(body.contains("\"db_ok\":true"), "body: {body}");
}

#[tokio::test]
async fn branches_endpoint_returns_json() {
    let (app, _tmp) = test_app().await;
    let (status, body) = get(&app, "/api/branches").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"branches\":[]"), "body: {body}");
}

#[tokio::test]
async fn health_endpoint_ok() {
    let (app, _tmp) = test_app().await;
    let (status, body) = get(&app, "/api/health").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"status\":\"ok\""), "body: {body}");
    assert!(body.contains("\"db\":true"), "body: {body}");
}

#[tokio::test]
async fn version_endpoint_reports_schema() {
    let (app, _tmp) = test_app().await;
    let (status, body) = get(&app, "/api/version").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("\"name\":\"webrms-next\""), "body: {body}");
    // schema count must match the embedded migrations (0001_init, 0002_promo_unique)
    let n = webrms_next::db::MIGRATOR.iter().count();
    assert!(body.contains(&format!("\"schema\":{n}")), "body: {body} (expected {n})");
}

#[tokio::test]
async fn unknown_route_404() {
    let (app, _tmp) = test_app().await;
    let (status, _) = get(&app, "/api/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

// ── P2: stocktake module over seeded local data ─────────────────────────────

async fn seed_stocktake_data(pool: &sqlx::SqlitePool) {
    sqlx::query("INSERT INTO branches (id, name, is_ho) VALUES (1, 'Test HoS', 1), (2, 'Test BoS', 0)")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO departments (id, ext_key, name, target_margin) VALUES (60, 60, 'Spirits', 25.0), (70, 70, 'Wine', 25.0)")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO suppliers (id, ext_key, code, name) VALUES (1, '010', '010', 'Tasman Liquor'), (2, '013', '013', 'Hancocks')")
        .execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO items (id, upc, sku, description, department_id, supplier_id, cost, price1, pack_units, is_active) VALUES \
         (1, '5010677014205', 'SKU1', 'Jameson 1L', 60, 1, 40.0, 59.99, 1, 1), \
         (2, '5010677025812', 'SKU2', 'Bacardi 1L', 60, 2, 36.0, 52.99, 1, 1), \
         (3, '9300675090216', 'SKU3', 'Esk Valley 750ml', 70, 2, 12.0, 19.99, 1, 1)"
    ).execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO stock_current (branch_id, upc, qty, as_of) VALUES \
         (2, '5010677014205', 12.0, '2026-09-02'), \
         (2, '5010677025812', 4.0, '2026-09-02'), \
         (2, '9300675090216', 30.0, '2026-09-02')"
    ).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO item_barcodes (upc, barcode) VALUES ('5010677014205', '999-OLD-UPCA')")
        .execute(pool).await.unwrap();
}

async fn seed_app() -> (axum::Router, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pool = webrms_next::db::init_pool(tmp.path()).await.expect("pool");
    seed_stocktake_data(&pool).await;
    let mut cfg = AppConfig::default();
    cfg.role.mode = "hos".into();
    let state = AppState::new(pool, cfg, tmp.path().to_path_buf());
    (webrms_next::build_app(state), tmp)
}

#[tokio::test]
async fn stocktake_departments_returns_seeded() {
    let (app, _tmp) = seed_app().await;
    let (status, body) = get(&app, "/api/stocktake/departments").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Spirits (60)"), "body: {body}");
    assert!(body.contains("Wine (70)"), "body: {body}");
}

#[tokio::test]
async fn stocktake_search_returns_sorted_items_with_stock() {
    let (app, _tmp) = seed_app().await;
    let (status, body) = get(&app, "/api/stocktake/search?branch=2").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Bacardi 1L"), "body: {body}");
    assert!(body.contains("Jameson 1L"), "body: {body}");
    // stock_on_hand for branch 2
    assert!(body.contains("\"stock_on_hand\":12.0"), "body: {body}");
    // dept filter
    let (status, body) = get(&app, "/api/stocktake/search?branch=2&department=70").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("Esk Valley 750ml"), "body: {body}");
    assert!(!body.contains("Bacardi 1L"), "dept filter leaked: {body}");
}

#[tokio::test]
async fn stocktake_barcode_lookup_alt_then_primary_fallback() {
    let (app, _tmp) = seed_app().await;
    // alt barcode resolves
    let (status, body) = get(&app, "/api/stocktake/barcode-lookup?barcode=999-OLD-UPCA").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("5010677014205"), "body: {body}");
    // primary UPC fallback
    let (status, body) = get(&app, "/api/stocktake/barcode-lookup?barcode=5010677025812").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("5010677025812"), "primary fallback failed: {body}");
}

#[tokio::test]
async fn stocktake_export_server_writes_count_and_ticket() {
    let (app, _tmp) = seed_app().await;
    let req = serde_json::json!({
        "rows": [
            {"upc": "5010677014205", "description": "Jameson 1L", "stock_on_hand": 12.0, "count": 11.0, "variance": -1.0, "has_ticket": true, "ticket_qty": 2},
            {"upc": "5010677025812", "description": "Bacardi 1L", "stock_on_hand": 4.0, "count": 4.0, "variance": 0.0, "has_ticket": false}
        ],
        "destination": "server"
    });
    let resp = app.clone().oneshot(
        Request::builder().method("POST").uri("/api/stocktake/export")
            .header("content-type", "application/json")
            .body(Body::from(req.to_string())).unwrap()
    ).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let body = String::from_utf8_lossy(&body).to_string();
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body.contains("\"status\":\"ok\""), "body: {body}");
    assert!(body.contains("\"count_rows\":2"), "body: {body}");
    assert!(body.contains("\"ticket_rows\":1"), "body: {body}");
    // files written under the temp data dir
    let files: serde_json::Value = serde_json::from_str(&body).unwrap();
    let count_path = files["count_file"].as_str().unwrap();
    let count_txt = std::fs::read_to_string(count_path).unwrap();
    assert!(count_txt.contains("0,5010677014205,11.0000,"), "count file: {count_txt}");
    let ticket_path = files["ticket_file"].as_str().unwrap();
    let qry = std::fs::read_to_string(ticket_path).unwrap();
    assert!(qry.contains("CriteriaCount=1"), "qry: {qry}");
    assert!(qry.contains("CopiesException=2"), "qry: {qry}");
}

// ── P2-2: ordering module — sheet + post + ETL + incoming-PO (W5) ─────────────

async fn seed_ordering_data(pool: &sqlx::SqlitePool) {
    sqlx::query("INSERT INTO branches (id, name, is_ho) VALUES (1, 'HoS', 1), (2, 'BoS', 0)")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO departments (id, ext_key, name) VALUES (60, 60, 'Spirits')")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO suppliers (id, ext_key, code, name) VALUES (1, '010', '010', 'Tasman Liquor')")
        .execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO items (id, upc, sku, description, department_id, supplier_id, cost, price1, pack_units, min_qty, max_qty, no_order, is_active) VALUES \
         (1, '5010677014205', 'S1', 'Jameson 1L', 60, 1, 40.0, 59.99, 6, 0, 0, 0, 1), \
         (2, '5010677025812', 'S2', 'Bacardi 1L', 60, 1, 36.0, 52.99, 12, 0, 0, 0, 1)"
    ).execute(pool).await.unwrap();
    // 30 days of sales @ 4/day for item 1, none for item 2 (no_history)
    let today = chrono::Local::now().date_naive();
    let mut tx = pool.begin().await.unwrap();
    for d in 0..30 {
        let date = (today - chrono::Duration::days(d)).format("%Y-%m-%d").to_string();
        sqlx::query(
            "INSERT INTO sales_daily (branch_id, upc, sale_date, units, revenue, promo_units, normal_units, cost_amount, line_margin) \
             VALUES (2, '5010677014205', ?1, 4.0, 239.96, 0, 4, 160.0, 79.96)",
        ).bind(&date).execute(&mut *tx).await.unwrap();
    }
    tx.commit().await.unwrap();
    sqlx::query("INSERT INTO stock_current (branch_id, upc, qty, as_of) VALUES (2, '5010677014205', 12.0, '2026-09-02'), (2, '5010677025812', 4.0, '2026-09-02')")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO supplier_modes (supplier_code, mode, lead_days, cycle_days, source) VALUES ('010', 'weekly', 3, 7, 'connector')")
        .execute(pool).await.unwrap();
}

async fn seed_order_app() -> (axum::Router, tempfile::TempDir) {
    seed_app_with(|pool| Box::pin(async move { seed_ordering_data(&pool).await })).await
}

/// Generic app seeder: fresh temp DB + HoS-mode config, then run `seed`.
async fn seed_app_with(
    seed: impl FnOnce(sqlx::SqlitePool) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>,
) -> (axum::Router, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pool = webrms_next::db::init_pool(tmp.path()).await.expect("pool");
    seed(pool.clone()).await;
    let mut cfg = AppConfig::default();
    cfg.role.mode = "hos".into();
    let state = AppState::new(pool, cfg, tmp.path().to_path_buf());
    (webrms_next::build_app(state), tmp)
}

#[tokio::test]
async fn ordering_sheet_forecasts_suggested_qty() {
    let (app, _tmp) = seed_order_app().await;
    let (status, body) = get(&app, "/api/ordering/sheet?supplier=010&branch=2").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let lines = v["lines"].as_array().unwrap();
    assert_eq!(lines.len(), 2, "body: {body}");
    // Jameson: 30d @4/day → has history → suggested > 0
    let jameson = lines.iter().find(|l| l["upc"] == "5010677014205").unwrap();
    assert_eq!(jameson["result"]["no_history"], false);
    assert!(jameson["result"]["suggested"].as_f64().unwrap() > 0.0, "suggested should be >0");
    // Bacardi: no history → never auto-suggest
    let bacardi = lines.iter().find(|l| l["upc"] == "5010677025812").unwrap();
    assert_eq!(bacardi["result"]["no_history"], true, "body: {body}");
    assert_eq!(bacardi["result"]["suggested"].as_f64().unwrap(), 0.0);
}

#[tokio::test]
async fn ordering_post_writes_etl_and_incoming_po() {
    let (app, _tmp) = seed_order_app().await;
    let body = serde_json::json!({
        "supplier": "010",
        "branch": 2,
        "by": "tester",
        "lines": [
            {"upc": "5010677014205", "qty": 12.0, "unit_cost": 40.0, "suggested_qty": 18.0}
        ]
    });
    let resp = app.clone().oneshot(
        Request::builder().method("POST").uri("/api/ordering/orders")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    let status = resp.status();
    let resp_body = resp.into_body().collect().await.unwrap().to_bytes();
    let resp_body = String::from_utf8_lossy(&resp_body).to_string();
    assert_eq!(status, StatusCode::OK, "body: {resp_body}");
    let v: serde_json::Value = serde_json::from_str(&resp_body).unwrap();
    assert_eq!(v["status"], "waiting_import");
    assert!(v["po_file"].as_str().unwrap().ends_with(".xlsx"));
    let bol = v["bill_of_lading"].as_str().unwrap();
    assert_eq!(bol.len(), 10, "bol: {bol}");
    // order recorded
    let (status, list) = get(&app, "/api/ordering/orders?branch=2").await;
    assert_eq!(status, StatusCode::OK);
    assert!(list.contains("\"status\":\"open\""), "orders: {list}");
    // incoming-PO tracked
    let (status, inc) = get(&app, "/api/sync/incoming-po").await;
    assert_eq!(status, StatusCode::OK);
    assert!(inc.contains("PurchaseOrder-010-2-"), "incoming: {inc}");
    // ETL file exists on disk under output/incoming-po/2/
    let files = std::fs::read_dir(_tmp.path().join("output/incoming-po/2")).unwrap();
    let names: Vec<String> = files.map(|f| f.unwrap().file_name().to_string_lossy().to_string()).collect();
    assert_eq!(names.len(), 1, "files: {names:?}");
    assert!(names[0].ends_with(".xlsx"));
}

#[tokio::test]
async fn ordering_confirmation_csv_shape() {
    let (app, _tmp) = seed_order_app().await;
    // post first to get an order id
    let body = serde_json::json!({
        "supplier": "010", "branch": 2,
        "lines": [{"upc": "5010677014205", "qty": 6.0, "unit_cost": 40.0}]
    });
    let resp = app.clone().oneshot(
        Request::builder().method("POST").uri("/api/ordering/orders")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    let resp_body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    let order_id = v["order_id"].as_str().unwrap();
    // confirmation CSV
    let (status, csv_resp) = get(&app, &format!("/api/ordering/confirmation-csv?order_id={order_id}")).await;
    assert_eq!(status, StatusCode::OK);
    let v: serde_json::Value = serde_json::from_str(&csv_resp).unwrap();
    let content = v["content"].as_str().unwrap();
    assert!(content.starts_with("Supplier,UPC,Description,Pack,OrderQty\n"), "csv: {content}");
    assert!(content.contains("010,5010677014205,Jameson 1L,6,6"), "csv: {content}");
}

// ── P2-3: payables — bills net of paid_ledger, returns, config, mark-paid ─────

async fn seed_payables_data(pool: &sqlx::SqlitePool) {
    sqlx::query("INSERT INTO branches (id, name, is_ho) VALUES (1, 'HoS', 1), (2, 'BoS', 0)")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO suppliers (id, ext_key, code, name) VALUES (1, '010', '010', 'Tasman Liquor'), (2, '013', '013', 'Hancocks')")
        .execute(pool).await.unwrap();
    // two unpaid invoices + one fully-paid (excluded)
    sqlx::query(
        "INSERT INTO ap_invoices (branch_id, supplier_id, invoice_number, description, invoice_date, invoice_amount, paid_amount, tax_amount1, logged) VALUES \
         (2, 1, 'INV-100', 'goods-in', '2026-07-15', 1234.50, 0, 154.31, '2026-07-15T10:00:00'), \
         (2, 2, 'INV-101', 'goods-in', '2026-07-16', 500.00, 0, 60.00, '2026-07-16T10:00:00'), \
         (2, 1, 'INV-102', 'goods-in', '2026-07-17', 900.00, 900.00, 108.00, '2026-07-17T10:00:00')"
    ).execute(pool).await.unwrap();
    // one return 'Z' credit
    sqlx::query(
        "INSERT INTO receipts (branch_id, trans_no, station, trans_type, supplier_id, invoice_no, total_cost, logged) \
         VALUES (2, 99, 1, 'Z', 1, 'CR-5', 200.00, '2026-07-18T09:00:00')"
    ).execute(pool).await.unwrap();
    // terms for 013: EOM+10 configured
    sqlx::query("INSERT INTO supplier_terms (supplier_code, term_type, term_days, configured, source) VALUES ('013', 'EOM', 10, 1, 'app')")
        .execute(pool).await.unwrap();
}

#[tokio::test]
async fn payables_bills_net_of_paid_and_returns() {
    let (app, _tmp) = seed_app_with(|pool| Box::pin(async move { seed_payables_data(&pool).await })).await;
    let (status, body) = get(&app, "/api/payables/invoices?from=2026-07-01&to=2026-08-01&branch=2").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let arr = v.as_array().unwrap();
    // INV-102 fully paid → excluded; INV-100/101 present
    assert_eq!(arr.len(), 2, "body: {body}");
    let inv100 = arr.iter().find(|i| i["invoice_number"] == "INV-100").unwrap();
    assert_eq!(inv100["invoice_amount"].as_f64().unwrap(), 1234.50);
    // 013 has EOM+10 configured → 2026-07-31 + 10 = 2026-08-10
    let inv101 = arr.iter().find(|i| i["invoice_number"] == "INV-101").unwrap();
    assert_eq!(inv101["due_date"], "2026-08-10");
    assert_eq!(inv101["terms_unset"], false);
    // 010 unconfigured → EOM+20 = 2026-08-20
    assert_eq!(inv100["due_date"], "2026-08-20");
    assert_eq!(inv100["terms_unset"], true);
}

#[tokio::test]
async fn payables_returns_lists_credits() {
    let (app, _tmp) = seed_app_with(|pool| Box::pin(async move { seed_payables_data(&pool).await })).await;
    let (status, body) = get(&app, "/api/payables/returns?from=2026-07-01&to=2026-08-01").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["invoice_number"], "CR-5");
    assert_eq!(arr[0]["invoice_amount"].as_f64().unwrap(), 200.00);
}

#[tokio::test]
async fn payables_mark_paid_and_config() {
    let (app, _tmp) = seed_app_with(|pool| Box::pin(async move { seed_payables_data(&pool).await })).await;
    // mark INV-100 paid
    let body = serde_json::json!({
        "rows": [{"branch_id": 2, "supplier_code": "010", "invoice_number": "INV-100", "amount": 1234.50}]
    });
    let resp = app.clone().oneshot(
        Request::builder().method("POST").uri("/api/payables/pay")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // bills now exclude INV-100 → only INV-101
    let (_, body) = get(&app, "/api/payables/invoices?from=2026-07-01&to=2026-08-01&branch=2").await;
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1, "body: {body}");
    assert_eq!(v.as_array().unwrap()[0]["invoice_number"], "INV-101");
    // paid ledger lists it
    let (_, body) = get(&app, "/api/payables/paid").await;
    assert!(body.contains("INV-100"), "paid: {body}");
    // config save (HoS mode) then read-back
    let cfg_body = serde_json::json!({"supplier_code": "010", "term_type": "EOM", "term_days": 30, "order_type": "", "payment_type": ""});
    let resp = app.clone().oneshot(
        Request::builder().method("POST").uri("/api/payables/config")
            .header("content-type", "application/json")
            .body(Body::from(cfg_body.to_string())).unwrap()
    ).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let (_, body) = get(&app, "/api/payables/config").await;
    assert!(body.contains("\"term_days\":30"), "config: {body}");
}

// ── P2-4: promotions — list / items / effectiveness over local promo_rules ───

async fn seed_promo_data(pool: &sqlx::SqlitePool) {
    sqlx::query("INSERT INTO branches (id, name, is_ho) VALUES (1, 'HoS', 1), (2, 'BoS', 0)")
        .execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO items (id, upc, sku, description, cost, price1, pack_units, is_active) VALUES \
         (1, '5010677014205', 'S1', 'Jameson 1L', 40.0, 59.99, 6, 1), \
         (2, '5010677025812', 'S2', 'Bacardi 1L', 36.0, 52.99, 12, 1)"
    ).execute(pool).await.unwrap();
    // RBP condition: bare UPC promo $49.99 — window RELATIVE to today so the
    // seeded sales (last 28 days) overlap it: promo = last 8 days, base = 8 days before
    let today = chrono::Local::now().date_naive();
    let promo_start = (today - chrono::Duration::days(7)).format("%Y-%m-%d").to_string();
    let promo_end = today.format("%Y-%m-%d").to_string();
    sqlx::query(
        "INSERT INTO promo_rules (id, kind, source, source_key, description, payload, sequence_match, condition_type, \
                adjustment_type, adjustment_value, effective_start, effective_end, branch_scope, is_active) VALUES \
         (1, 'rbp_condition', 'connector', '2808', 'ABS 2x', '{}', '5010677014205', 'RETAIL', 'ABS', 49.99, \
          ?1, ?2, NULL, 1)"
    ).bind(&promo_start).bind(&promo_end)
    .execute(pool).await.unwrap();
    // sales: promo window 10 units @ 49.99, base window 5 units @ 59.99
    let mut tx = pool.begin().await.unwrap();
    for d in 0..28 {
        let date = (today - chrono::Duration::days(d)).format("%Y-%m-%d").to_string();
        let (units, promo_units, revenue, price) = if d < 8 {
            (4.0, 4.0, 4.0 * 49.99, 49.99) // promo window: 32 units total
        } else if d < 16 {
            (2.0, 0.0, 2.0 * 59.99, 59.99) // base window: 16 units total
        } else {
            (0.0, 0.0, 0.0, 0.0)
        };
        if units > 0.0 {
            sqlx::query(
                "INSERT INTO sales_daily (branch_id, upc, sale_date, units, revenue, promo_units, normal_units, cost_amount, line_margin) \
                 VALUES (2, '5010677014205', ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            ).bind(&date).bind(units).bind(revenue).bind(promo_units).bind(units - promo_units)
             .bind(units * 40.0).bind(revenue - units * 40.0).execute(&mut *tx).await.unwrap();
        }
        let _ = price;
    }
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn promotions_list_and_effectiveness() {
    let (app, _tmp) = seed_app_with(|pool| Box::pin(async move { seed_promo_data(&pool).await })).await;
    // engine + list
    let (status, body) = get(&app, "/api/promotions/list").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["total"], 1);
    assert_eq!(v["active"], 1);
    assert_eq!(v["promotions"][0]["scope"], "UPC");
    assert_eq!(v["promotions"][0]["price"].as_f64().unwrap(), 49.99);
    // items resolve
    let (status, body) = get(&app, "/api/promotions/items?id=pc-1").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["resolvable"], true);
    let item = &v["items"][0];
    assert_eq!(item["upc"], "5010677014205");
    assert!(item["discount_pct"].as_f64().unwrap() < 0.0, "promo below Price1 → negative");
    // effectiveness: promo 32 units vs base 16 → uplift 2.0
    let today = chrono::Local::now().date_naive();
    let from = (today - chrono::Duration::days(40)).format("%Y-%m-%d").to_string();
    let to = today.format("%Y-%m-%d").to_string();
    let (status, body) = get(&app, &format!("/api/promotions/effectiveness?from={from}&to={to}&branch=2")).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    let s = &v["specials"][0];
    assert_eq!(s["promo_units"].as_f64().unwrap(), 32.0, "body: {body}");
    assert_eq!(s["base_units"].as_f64().unwrap(), 16.0, "body: {body}");
    assert!((s["uplift_units"].as_f64().unwrap() - 2.0).abs() < 1e-9);
}

// ── P2-5: reports — daily/depts/overview/movers/dept-weekly over sales_daily ─

async fn seed_report_data(pool: &sqlx::SqlitePool) {
    sqlx::query("INSERT INTO branches (id, name, is_ho) VALUES (1, 'HoS', 1), (2, 'BoS', 0)")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO departments (id, ext_key, name, target_margin) VALUES (60, 60, 'Spirits', 25), (70, 70, 'Wine', 30)")
        .execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO items (id, upc, sku, description, department_id, cost, price1, is_active) VALUES \
         (1, '5010677014205', 'S1', 'Jameson 1L', 60, 40.0, 59.99, 1), \
         (2, '5010677025812', 'S2', 'Bacardi 1L', 60, 36.0, 52.99, 1), \
         (3, '9311043093661', 'W1', 'Banrock Moscato', 70, 8.0, 15.99, 1)"
    ).execute(pool).await.unwrap();
    // 10 days: Jameson 10/day, Bacardi 5/day, Banrock 3/day
    let today = chrono::Local::now().date_naive();
    let mut tx = pool.begin().await.unwrap();
    for d in 0..10 {
        let date = (today - chrono::Duration::days(d)).format("%Y-%m-%d").to_string();
        for (upc, units, cost) in [("5010677014205", 10.0, 40.0), ("5010677025812", 5.0, 36.0), ("9311043093661", 3.0, 8.0)] {
            let price = if upc == "5010677014205" { 59.99 } else if upc == "5010677025812" { 52.99 } else { 15.99 };
            let revenue = units * price;
            sqlx::query(
                "INSERT INTO sales_daily (branch_id, upc, sale_date, units, revenue, promo_units, normal_units, cost_amount, line_margin) \
                 VALUES (2, ?1, ?2, ?3, ?4, 0, ?3, ?5, ?6)",
            ).bind(upc).bind(&date).bind(units).bind(revenue).bind(units * cost).bind(revenue - units * cost)
             .execute(&mut *tx).await.unwrap();
        }
    }
    tx.commit().await.unwrap();
    sqlx::query("INSERT INTO stock_current (branch_id, upc, qty, as_of) VALUES (2, '5010677014205', 50.0, '2026-09-02'), (2, '5010677025812', 20.0, '2026-09-02'), (2, '9311043093661', 0.0, '2026-09-02')")
        .execute(pool).await.unwrap();
}

#[tokio::test]
async fn reports_overview_and_depts() {
    let (app, _tmp) = seed_app_with(|pool| Box::pin(async move { seed_report_data(&pool).await })).await;
    let today = chrono::Local::now().date_naive();
    let from = (today - chrono::Duration::days(15)).format("%Y-%m-%d").to_string();
    let to = (today + chrono::Duration::days(1)).format("%Y-%m-%d").to_string();

    // overview
    let (status, body) = get(&app, "/api/reports/overview?branch=2").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v["sales"]["today"].as_f64().unwrap() > 0.0, "body: {body}");
    assert!(v["stock"]["items"].as_i64().unwrap() >= 3, "body: {body}");
    // stockout: Banrock qty 0
    assert!(v["stock"]["stockout"].as_i64().unwrap() >= 1, "body: {body}");

    // depts: 2 depts, Spirits has 2 products
    let (status, body) = get(&app, &format!("/api/reports/depts?from={from}&to={to}&branch=2")).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 2, "body: {body}");
    let spirits = v.as_array().unwrap().iter().find(|d| d["dept_name"] == "Spirits").unwrap();
    assert_eq!(spirits["products"].as_array().unwrap().len(), 2, "body: {body}");

    // movers: Jameson top by net
    let (status, body) = get(&app, &format!("/api/reports/overview/movers?from={from}&to={to}&branch=2&limit=5")).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v.as_array().unwrap()[0]["upc"], "5010677014205", "body: {body}");

    // dept-weekly: includes Total row
    let (status, body) = get(&app, "/api/reports/overview/dept-weekly?branch=2").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let v: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert!(v.as_array().unwrap().iter().any(|d| d["dept_name"] == "Total"), "body: {body}");
    assert!(v.as_array().unwrap()[0]["this_week_gross"].as_f64().unwrap() > 0.0, "body: {body}");
}

// ── P2-6: incoming-PO lifecycle — waiting_import → pending_receipt → receipted ─

async fn seed_incoming_po_data(pool: &sqlx::SqlitePool) {
    sqlx::query("INSERT INTO branches (id, name, is_ho) VALUES (1, 'HoS', 1), (2, 'BoS', 0)")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO suppliers (id, ext_key, code, name) VALUES (1, '010', '010', 'Tasman Liquor')")
        .execute(pool).await.unwrap();
    // a PO we generated, still waiting for import
    sqlx::query(
        "INSERT INTO incoming_pos (id, origin_install, branch_id, supplier_id, filename, bill_of_lading, poid, status, imported, placed_at) \
         VALUES ('po-1', 'local', 2, 1, 'PurchaseOrder-010-2-20260902-120000-ABC123.xlsx', 'ABC123', 123456, 'waiting_import', 0, '2026-09-02 12:00:00')"
    ).execute(pool).await.unwrap();
    // a PO NOT ours (no poid match) — must stay waiting_import
    sqlx::query(
        "INSERT INTO incoming_pos (id, origin_install, branch_id, supplier_id, filename, bill_of_lading, poid, status, imported, placed_at) \
         VALUES ('po-2', 'local', 2, 1, 'PurchaseOrder-010-2-20260902-130000-DEF456.xlsx', 'DEF456', 999999, 'waiting_import', 0, '2026-09-02 13:00:00')"
    ).execute(pool).await.unwrap();
}

#[tokio::test]
async fn incoming_po_auto_flip_lifecycle() {
    let (_app, tmp) = seed_app_with(|pool| Box::pin(async move { seed_incoming_po_data(&pool).await })).await;
    // seed_app_with calls init_pool(tmp.path()) → the DB is <tmp>/data.db
    let pool = webrms_next::db::init_pool(tmp.path()).await.unwrap();

    // stage 1: Infinity imports the PO → a 'P' receipt with our BillOfLading appears
    sqlx::query(
        "INSERT INTO receipts (branch_id, trans_no, station, trans_type, supplier_id, invoice_no, total_cost, logged, bill_of_lading) \
         VALUES (2, 5000, 1, 'P', 1, 'P-5000', 1000.0, '2026-09-02 14:00:00', 'ABC123')"
    ).execute(&pool).await.unwrap();
    let flipped = webrms_next::modules::incoming_po::auto_flip(&pool, "local").await.unwrap();
    assert_eq!(flipped, 1, "only our PO flips");
    let status: String = sqlx::query_scalar("SELECT status FROM incoming_pos WHERE id = 'po-1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(status, "pending_receipt");
    let status2: String = sqlx::query_scalar("SELECT status FROM incoming_pos WHERE id = 'po-2'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(status2, "waiting_import", "foreign PO untouched");

    // stage 2: goods-in arrives, linked to the P via OriginatingTransNo
    sqlx::query(
        "INSERT INTO receipts (branch_id, trans_no, station, trans_type, supplier_id, invoice_no, total_cost, logged, bill_of_lading, originating_trans_no) \
         VALUES (2, 6000, 1, 'G', 1, 'G-6000', 1000.0, '2026-09-02 16:00:00', 'ABC123', 5000)"
    ).execute(&pool).await.unwrap();
    let flipped = webrms_next::modules::incoming_po::auto_flip(&pool, "local").await.unwrap();
    assert_eq!(flipped, 1);
    let status: String = sqlx::query_scalar("SELECT status FROM incoming_pos WHERE id = 'po-1'")
        .fetch_one(&pool).await.unwrap();
    assert_eq!(status, "receipted");

    // list shows both with supplier codes (ordered placed_at DESC → po-2 first)
    let list = webrms_next::modules::incoming_po::list(&pool).await.unwrap();
    assert_eq!(list.len(), 2);
    let ours = list.iter().find(|p| p["poid"] == 123456).unwrap();
    assert_eq!(ours["status"], "receipted");
    assert_eq!(ours["supplier_code"], "010");
    let foreign = list.iter().find(|p| p["poid"] == 999999).unwrap();
    assert_eq!(foreign["status"], "waiting_import");
}

// ── P2-8: full round-trip (W5) — order → ETL PO → waiting_import → 'P' import
//         → pending_receipt → 'G' goods-in → receipted, all on the local DB ──

async fn seed_roundtrip_data(pool: &sqlx::SqlitePool) {
    sqlx::query("INSERT INTO branches (id, name, is_ho) VALUES (1, 'HoS', 1), (2, 'BoS', 0)")
        .execute(pool).await.unwrap();
    sqlx::query("INSERT INTO suppliers (id, ext_key, code, name) VALUES (1, '010', '010', 'Tasman Liquor')")
        .execute(pool).await.unwrap();
    sqlx::query(
        "INSERT INTO items (id, upc, sku, description, department_id, supplier_id, cost, price1, pack_units, is_active) VALUES \
         (1, '5010677014205', 'S1', 'Jameson 1L', NULL, 1, 40.0, 59.99, 6, 1)"
    ).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO stock_current (branch_id, upc, qty, as_of) VALUES (2, '5010677014205', 12.0, '2026-09-02')")
        .execute(pool).await.unwrap();
}

#[tokio::test]
async fn w5_full_round_trip() {
    let (app, tmp) = seed_app_with(|pool| Box::pin(async move { seed_roundtrip_data(&pool).await })).await;
    // 1. post order via the API (creates order + ETL xlsx + incoming_pos row)
    let body = serde_json::json!({
        "supplier": "010", "branch": 2,
        "by": "e2e",
        "lines": [{"upc": "5010677014205", "qty": 12.0, "unit_cost": 40.0, "suggested_qty": 18.0}]
    });
    let resp = app.clone().oneshot(
        Request::builder().method("POST").uri("/api/ordering/orders")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())).unwrap()
    ).await.unwrap();
    let rb = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&rb).unwrap();
    assert_eq!(v["status"], "waiting_import");
    let bol = v["bill_of_lading"].as_str().unwrap().to_string();
    let order_id = v["order_id"].as_str().unwrap().to_string();

    // order exists with status open
    let (_, obody) = get(&app, "/api/ordering/orders?branch=2").await;
    assert!(obody.contains("\"status\":\"open\""), "orders: {obody}");

    // 2. ETL file exists in output/incoming-po/2/
    let files: Vec<String> = std::fs::read_dir(tmp.path().join("output/incoming-po/2"))
        .unwrap().map(|f| f.unwrap().file_name().to_string_lossy().to_string()).collect();
    assert_eq!(files.len(), 1, "files: {files:?}");
    assert!(files[0].ends_with(".xlsx"));

    // 3. Infinity imports the PO → 'P' receipt with our BillOfLading appears
    let pool = webrms_next::db::init_pool(tmp.path()).await.unwrap();
    sqlx::query(
        "INSERT INTO receipts (branch_id, trans_no, station, trans_type, supplier_id, invoice_no, total_cost, logged, bill_of_lading) \
         VALUES (2, 7000, 1, 'P', 1, 'P-7000', 480.0, '2026-09-02 15:00:00', ?1)"
    ).bind(&bol).execute(&pool).await.unwrap();
    let flipped = webrms_next::modules::incoming_po::auto_flip(&pool, "local").await.unwrap();
    assert_eq!(flipped, 1);

    // 4. goods-in → 'G' linked via OriginatingTransNo → receipted
    sqlx::query(
        "INSERT INTO receipts (branch_id, trans_no, station, trans_type, supplier_id, invoice_no, total_cost, logged, bill_of_lading, originating_trans_no) \
         VALUES (2, 7001, 1, 'G', 1, 'G-7001', 480.0, '2026-09-02 17:00:00', ?1, 7000)"
    ).bind(&bol).execute(&pool).await.unwrap();
    let flipped = webrms_next::modules::incoming_po::auto_flip(&pool, "local").await.unwrap();
    assert_eq!(flipped, 1);
    let status: String = sqlx::query_scalar("SELECT status FROM incoming_pos WHERE bill_of_lading = ?1")
        .bind(&bol).fetch_one(&pool).await.unwrap();
    assert_eq!(status, "receipted");

    // 5. the linked order line qty now counts as on-order history but the
    //    order itself stays open (cleared only via explicit clearing) — the
    //    G-10 lifecycle: active_on_order includes the open order
    let on_order = webrms_next::modules::ordering::orders::active_on_order(&pool, 2, "5010677014205").await.unwrap();
    assert_eq!(on_order, 12.0, "open order line still on order");

    // confirmation CSV works for the order
    let (status, csv_body) = get(&app, &format!("/api/ordering/confirmation-csv?order_id={order_id}")).await;
    assert_eq!(status, StatusCode::OK);
    let cv: serde_json::Value = serde_json::from_str(&csv_body).unwrap();
    assert!(cv["content"].as_str().unwrap().contains("010,5010677014205,Jameson 1L,6,12"), "csv: {}", cv);
}

// ── P3: replication — outbox push up (order/PO) + pull down (config) ─────────

/// Spin up a real HTTP server on an ephemeral port, return (base_url, app).
async fn serve_app(state: webrms_next::state::SharedState) -> (String, axum::Router) {
    let app = webrms_next::build_app(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app2 = app.clone();
    tokio::spawn(async move {
        axum::serve(listener, app2).await.unwrap();
    });
    (format!("http://{addr}"), app)
}

#[tokio::test]
async fn replication_order_up_config_down() {
    // ── HoS (source) ──
    let hos_tmp = tempfile::tempdir().unwrap();
    let hos_pool = webrms_next::db::init_pool(hos_tmp.path()).await.unwrap();
    let mut hos_cfg = AppConfig::default();
    hos_cfg.role.mode = "hos".into();
    hos_cfg.sync.install_name = "hos-1".into();
    hos_cfg.sync.enabled = true;
    let hos_state = AppState::new(hos_pool.clone(), hos_cfg, hos_tmp.path().to_path_buf());
    let (hos_url, _hos_app) = serve_app(hos_state).await;
    // seed both with the same reference rows (branches/suppliers)
    sqlx::query("INSERT INTO branches (id, name, is_ho) VALUES (1, 'HoS', 1), (2, 'BoS', 0)")
        .execute(&hos_pool).await.unwrap();
    sqlx::query("INSERT INTO suppliers (id, ext_key, code, name) VALUES (1, '010', '010', 'Tasman Liquor')")
        .execute(&hos_pool).await.unwrap();

    // ── BoS (client) ──
    let bos_tmp = tempfile::tempdir().unwrap();
    let bos_pool = webrms_next::db::init_pool(bos_tmp.path()).await.unwrap();
    let mut bos_cfg = AppConfig::default();
    bos_cfg.role.mode = "bos".into();
    bos_cfg.sync.install_name = "bos-2".into();
    bos_cfg.sync.enabled = true;
    bos_cfg.sync.source = hos_url.clone();
    let bos_state = AppState::new(bos_pool.clone(), bos_cfg.clone(), bos_tmp.path().to_path_buf());
    let bos_sync = bos_cfg.sync.clone();
    let (_bos_url, _bos_app) = serve_app(bos_state).await;
    sqlx::query("INSERT INTO branches (id, name, is_ho) VALUES (1, 'HoS', 1), (2, 'BoS', 0)")
        .execute(&bos_pool).await.unwrap();
    sqlx::query("INSERT INTO suppliers (id, ext_key, code, name) VALUES (1, '010', '010', 'Tasman Liquor')")
        .execute(&bos_pool).await.unwrap();

    // ── 1. BoS creates an order (branch 2) → outbox row on the BoS ──
    let order_id = webrms_next::modules::ordering::orders::create_order(
        &bos_pool, "bos-2", 2, Some(1), Some("tester"),
        &[("5010677014205".into(), 12.0, 40.0, 18.0)],
    ).await.unwrap();
    let bo_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbox WHERE origin_install = 'bos-2'")
        .fetch_one(&bos_pool).await.unwrap();
    assert_eq!(bo_count, 1, "order emitted one outbox row");

    // ── 2. BoS pushes up → HoS applies it ──
    let pushed = webrms_next::replication::push_once(&bos_pool, &bos_sync).await.unwrap();
    assert_eq!(pushed, 1, "one row pushed up");
    let hos_orders: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM orders").fetch_one(&hos_pool).await.unwrap();
    assert_eq!(hos_orders, 1, "HoS has the BoS order");
    let status: String = sqlx::query_scalar("SELECT status FROM orders WHERE id = ?1")
        .bind(&order_id).fetch_one(&hos_pool).await.unwrap();
    assert_eq!(status, "open");
    // pushed row marked applied on the BoS (no retry loop)
    let unapplied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbox WHERE origin_install = 'bos-2' AND applied = 0")
        .fetch_one(&bos_pool).await.unwrap();
    assert_eq!(unapplied, 0);

    // ── 3. HoS sets supplier terms (config) → BoS pulls down ──
    sqlx::query(
        "INSERT INTO supplier_terms (supplier_code, term_type, term_days, configured, source, updated_at) \
         VALUES ('010', 'EOM', 30, 1, 'app', '2026-09-02')",
    ).execute(&hos_pool).await.unwrap();
    let payload = serde_json::json!({
        "supplier_code": "010", "term_type": "EOM", "term_days": 30,
        "order_type": "", "payment_type": "", "configured": 1, "source": "app",
        "updated_at": "2026-09-02",
    });
    let mut tx = hos_pool.begin().await.unwrap();
    webrms_next::replication::emit(&mut tx, "hos-1", "supplier_terms", "010", "insert", &payload).await.unwrap();
    tx.commit().await.unwrap();

    let pulled = webrms_next::replication::pull_once(&bos_pool, &hos_sync(&hos_url), Some(2)).await.unwrap();
    assert_eq!(pulled, 1, "BoS pulled the config row");
    let terms: Option<i64> = sqlx::query_scalar("SELECT term_days FROM supplier_terms WHERE supplier_code = '010'")
        .fetch_one(&bos_pool).await.unwrap();
    assert_eq!(terms, Some(30), "terms propagated down");

    // ── 4. echo prevention: BoS must NOT pull back its own order ──
    let pulled2 = webrms_next::replication::pull_once(&bos_pool, &hos_sync(&hos_url), Some(2)).await.unwrap();
    assert_eq!(pulled2, 0, "no echo of own rows; watermark advanced");
}

fn hos_sync(url: &str) -> webrms_next::config::SyncConfig {
    webrms_next::config::SyncConfig {
        enabled: true,
        source: url.to_string(),
        poll_interval_minutes: 1,
        install_name: "bos-2".into(),
        snapshot_key: String::new(),
    }
}

// ── P3: snapshot fallback (O-5) — signed gzip restore round-trip ────────────

#[tokio::test]
async fn snapshot_signed_restore_round_trip() {
    // HoS with a signing key
    let hos_tmp = tempfile::tempdir().unwrap();
    let hos_pool = webrms_next::db::init_pool(hos_tmp.path()).await.unwrap();
    let mut hos_cfg = AppConfig::default();
    hos_cfg.role.mode = "hos".into();
    hos_cfg.sync.snapshot_key = "test-key-123".into();
    let hos_state = AppState::new(hos_pool.clone(), hos_cfg, hos_tmp.path().to_path_buf());
    let (hos_url, _app) = serve_app(hos_state).await;
    sqlx::query("INSERT INTO branches (id, name, is_ho) VALUES (1, 'HoS', 1)")
        .execute(&hos_pool).await.unwrap();
    sqlx::query("INSERT INTO suppliers (id, ext_key, code, name) VALUES (1, '010', '010', 'Tasman Liquor')")
        .execute(&hos_pool).await.unwrap();
    sqlx::query("INSERT INTO items (id, upc, sku, description, cost, price1, is_active) VALUES (1, '5010677014205', 'S1', 'Jameson', 40.0, 59.99, 1)")
        .execute(&hos_pool).await.unwrap();

    // client restores the signed snapshot into a fresh data dir
    let client_dir = tempfile::tempdir().unwrap();
    let db_path = client_dir.path().join("data.db");
    let restored = webrms_next::snapshot::restore_snapshot(&hos_url, "test-key-123", &db_path).await.unwrap();
    assert!(restored > 0, "downloaded gz bytes");

    // wrong key must refuse
    let wrong = webrms_next::snapshot::restore_snapshot(&hos_url, "wrong-key", &db_path).await;
    assert!(wrong.is_err(), "HMAC mismatch must refuse restore");

    // open the restored DB and verify content
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new().connect_with(opts).await.unwrap();
    let items: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items").fetch_one(&pool).await.unwrap();
    assert_eq!(items, 1);
    let suppliers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM suppliers").fetch_one(&pool).await.unwrap();
    assert_eq!(suppliers, 1);
}
