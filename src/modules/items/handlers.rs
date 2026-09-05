// Items maintenance HTTP (W6). Endpoints:
//   GET  /api/items/search?q=&include_inactive=&supplier=
//   GET  /api/items/:upc
//   POST /api/items/edit    { upc, operator, fields{} }
//   POST /api/items/clone   { from_upc, new_upc, operator, fields{} }
//   POST /api/items/etl     { kind: edit|clone, upc|(from_upc,new_upc) } → .xlsx patch
// Author-gated writes (HoS / Remote-HoS); BoS read-only.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use crate::modules::items::{db, etl};
use crate::state::SharedState;

pub fn routes() -> axum::Router<SharedState> {
    axum::Router::new()
        .route("/api/items/search", axum::routing::get(search))
        .route("/api/items/facets", axum::routing::get(facets))
        .route("/api/items/{upc}", axum::routing::get(get_one))
        .route("/api/items/edit", axum::routing::post(edit))
        .route("/api/items/clone", axum::routing::post(clone))
        .route("/api/items/etl", axum::routing::post(etl_patch))
        .route("/api/items/patch/{name}", axum::routing::get(patch_file))
}

#[derive(Deserialize)]
struct SearchQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    include_inactive: bool,
    #[serde(default)]
    supplier: Option<String>,
    #[serde(default)]
    department: Option<String>,
    #[serde(default)]
    sub: Option<String>,
    #[serde(default)]
    class: Option<String>,
    #[serde(default)]
    disc_group: Option<String>,
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    non_stock: Option<bool>,
}

async fn search(State(state): State<SharedState>, Query(query): Query<SearchQuery>) -> impl IntoResponse {
    let f = db::SearchFilters {
        q: &query.q,
        include_inactive: query.include_inactive,
        supplier: query.supplier.as_deref(),
        department: query.department.as_deref(),
        sub: query.sub.as_deref(),
        class: query.class.as_deref(),
        disc_group: query.disc_group.as_deref(),
        parent: query.parent.as_deref(),
        non_stock: query.non_stock,
    };
    match db::search(&*state.pool_arc(), &f).await {
        Ok(items) => (StatusCode::OK, Json(json!({ "items": items }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("{e}") }))),
    }
}

async fn facets(State(state): State<SharedState>) -> impl IntoResponse {
    match db::facets(&*state.pool_arc()).await {
        Ok(f) => (StatusCode::OK, Json(json!(f))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("{e}") }))),
    }
}

async fn get_one(State(state): State<SharedState>, Path(upc): Path<String>) -> impl IntoResponse {
    match db::get(&*state.pool_arc(), &upc).await {
        Ok(Some(item)) => (StatusCode::OK, Json(json!(item))),
        Ok(None) => (StatusCode::NOT_FOUND, Json(json!({ "error": "item not found" }))),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("{e}") }))),
    }
}

#[derive(Deserialize)]
struct EditBody {
    upc: String,
    #[serde(default)]
    operator: String,
    fields: serde_json::Map<String, serde_json::Value>,
}

async fn edit(State(state): State<SharedState>, Json(body): Json<EditBody>) -> impl IntoResponse {
    if !state.config_author() {
        return forbidden();
    }
    let install = state.cfg.sync.install_name.clone();
    let operator = if body.operator.is_empty() { install.clone() } else { body.operator.clone() };
    match db::edit_item(&*state.pool_arc(), &install, &operator, &body.upc, &body.fields).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "status": "ok", "upc": body.upc }))),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("{e}") }))),
    }
}

#[derive(Deserialize)]
struct CloneBody {
    from_upc: String,
    new_upc: String,
    #[serde(default)]
    operator: String,
    #[serde(default)]
    fields: serde_json::Map<String, serde_json::Value>,
}

async fn clone(State(state): State<SharedState>, Json(body): Json<CloneBody>) -> impl IntoResponse {
    if !state.config_author() {
        return forbidden();
    }
    let install = state.cfg.sync.install_name.clone();
    let operator = if body.operator.is_empty() { install.clone() } else { body.operator.clone() };
    match db::clone_item(&*state.pool_arc(), &install, &operator, &body.from_upc, &body.new_upc, &body.fields).await {
        Ok(()) => (StatusCode::OK, Json(json!({ "status": "ok", "from_upc": body.from_upc, "new_upc": body.new_upc }))),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": format!("{e}") }))),
    }
}

#[derive(Deserialize)]
struct EtlBody {
    kind: String, // edit | clone
    upc: Option<String>,        // edit
    from_upc: Option<String>,   // clone
    new_upc: Option<String>,    // clone
}

async fn etl_patch(State(state): State<SharedState>, Json(body): Json<EtlBody>) -> impl IntoResponse {
    let pool = state.pool_arc();
    let data_dir = state.data_dir.clone();
    let mut rows: Vec<etl::EtlRow> = Vec::new();
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let filename;
    match body.kind.as_str() {
        "edit" => {
            let upc = body.upc.clone().unwrap_or_default();
            let item = match db::get(&*pool, &upc).await {
                Ok(Some(v)) => v,
                Ok(None) => return err(StatusCode::NOT_FOUND, "item not found"),
                Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")),
            };
            filename = format!("Item-edit-{upc}-{ts}.xlsx");
            rows.push(item_row(&item, false));
        }
        "clone" => {
            let from = body.from_upc.clone().unwrap_or_default();
            let to = body.new_upc.clone().unwrap_or_default();
            let new_item = match db::get(&*pool, &to).await {
                Ok(Some(v)) => v,
                Ok(None) => return err(StatusCode::NOT_FOUND, "new item not found — clone it first"),
                Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")),
            };
            let old_item = match db::get(&*pool, &from).await {
                Ok(Some(v)) => v,
                Ok(None) => return err(StatusCode::NOT_FOUND, "source item not found"),
                Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("{e}")),
            };
            filename = format!("Item-clone-{from}-{to}-{ts}.xlsx");
            let mut new_row = item_row(&new_item, false);
            new_row.alternate_barcode = Some(from.clone());
            rows.push(new_row);
            let mut old_row = item_row(&old_item, true);
            old_row.sku = Some(format!("OLD_{to}"));
            old_row.product_code = from;
            rows.push(old_row);
        }
        _ => return err(StatusCode::BAD_REQUEST, "kind must be edit|clone"),
    }
    let bytes = etl::build_item_patch(&rows);
    let path = match crate::files::write_atomic(&data_dir, &format!("{}/items", crate::files::OUTPUT_DIR), &filename, &bytes) {
        Ok(p) => p,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, &format!("write failed: {e}")),
    };
    // attach the file to the pending export record
    let _ = sqlx::query("UPDATE item_etl_exports SET filename = ?1 WHERE kind = ?2 AND filename = '' ORDER BY created_at DESC LIMIT 1")
        .bind(&filename).bind(&body.kind)
        .execute(&*pool).await;
    (
        StatusCode::OK,
        Json(json!({ "filename": filename, "path": path.display().to_string(), "rows": rows.len() })),
    )
}

/// GET /api/items/patch/:name — stream a generated Item-ETL patch from
/// output/items (sanitized filename, local file only).
async fn patch_file(State(state): State<SharedState>, Path(name): Path<String>) -> impl IntoResponse {
    use axum::http::header;
    if !name.starts_with("Item-") || !name.ends_with(".xlsx") || name.contains('/') || name.contains('\\') || name.contains("..") {
        return (StatusCode::BAD_REQUEST, header::HeaderMap::new(), axum::body::Body::from("bad filename"));
    }
    let path = state.data_dir.join("output/items").join(&name);
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let mut headers = header::HeaderMap::new();
            if let Ok(v) = header::HeaderValue::from_str("application/vnd.openxmlformats-officedocument.spreadsheetml.sheet") {
                headers.insert(header::CONTENT_TYPE, v);
            }
            if let Ok(v) = header::HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
                headers.insert(header::CONTENT_DISPOSITION, v);
            }
            (StatusCode::OK, headers, axum::body::Body::from(bytes))
        }
        Err(_) => (StatusCode::NOT_FOUND, header::HeaderMap::new(), axum::body::Body::from("not found")),
    }
}

fn item_row(item: &db::ItemRow, inactive: bool) -> etl::EtlRow {
    etl::EtlRow {
        product_code: item.upc.clone(),
        sku: item.sku.clone(),
        description: item.description.clone(),
        supplier: item.supplier_code.clone(),
        supplier_prod_code: item.supplier_prod_code.clone(),
        alternate_barcode: None,
        cost: Some(item.cost),
        cost_ave: Some(item.cost_ave),
        pack_cost: None,
        pack_size: Some(item.pack_units),
        price1: Some(item.price1),
        inactive: Some(inactive || !item.is_active),
    }
}

fn forbidden() -> (StatusCode, Json<serde_json::Value>) {
    (StatusCode::FORBIDDEN, Json(json!({ "error": "Item maintenance is Head Office only." })))
}
fn err(code: StatusCode, msg: &str) -> (StatusCode, Json<serde_json::Value>) {
    (code, Json(json!({ "error": msg })))
}
