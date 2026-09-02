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
