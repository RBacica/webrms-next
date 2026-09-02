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
    // 0001_init.sql = exactly 1 migration
    assert!(body.contains("\"schema\":1"), "body: {body}");
}

#[tokio::test]
async fn unknown_route_404() {
    let (app, _tmp) = test_app().await;
    let (status, _) = get(&app, "/api/nope").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
