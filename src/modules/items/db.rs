// Items maintenance (W6) over the local DB.
//
// The WebRMS-Next item model (user decision, 2026-09-02): a UPC change is a
// CLONE — copy the item to the new UPC, retire the old one (SKU = OLD_<new>),
// carry history (history_alias) and set the new item's alt barcode to the old
// UPC. App-edited fields are protected from the connector via app_overrides
// (O-12): the connector must never silently overwrite an app-authoritative
// value, and a direct AKPOS change to an overridden field is flagged
// external_edit instead of being lost.
//
// The ETL patch writer (etl.rs) emits the exact columns Infinity's Items ETL
// reads by header name (verified against a real Item-*.xlsx export).

use serde_json::json;
use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

#[derive(Debug, serde::Serialize, Clone)]
pub struct ItemRow {
    pub upc: String,
    pub sku: Option<String>,
    pub description: Option<String>,
    pub supplier_code: Option<String>,
    pub supplier_prod_code: Option<String>,
    pub cost: f64,
    pub cost_ave: f64,
    pub price1: f64,
    pub pack_units: f64,
    pub department: Option<String>,
    pub department_id: Option<i64>,
    pub sub_department: Option<String>,
    pub class: Option<String>,
    pub disc_group: Option<String>,
    pub non_stock: bool,
    pub is_active: bool,
    pub parent_upc: Option<String>,
    pub source: String,
    pub overridden: bool, // any app_overrides exist for this item
}

/// Fields an operator may edit/clone. Matches the app_overrides whitelist.
pub const EDITABLE: &[&str] = &[
    "description", "supplier_code", "supplier_prod_code", "cost", "cost_ave",
    "price1", "price2", "price3", "price4", "price5", "price6", "price7", "price8",
    "pack_units", "volume_ml", "tax_code", "parent_upc", "non_stock", "is_active",
];

#[derive(Debug, Default, Clone)]
pub struct SearchFilters<'a> {
    pub q: &'a str,
    pub include_inactive: bool,
    pub supplier: Option<&'a str>,
    pub department: Option<&'a str>,
    pub sub: Option<&'a str>,
    pub class: Option<&'a str>,
    pub disc_group: Option<&'a str>,
    pub parent: Option<&'a str>,   // "parent" | "child" | None
    pub non_stock: Option<bool>,
}

pub async fn search(pool: &SqlitePool, f: &SearchFilters<'_>) -> anyhow::Result<Vec<ItemRow>> {
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT i.upc, i.sku, i.description, s.code AS supplier_code, i.supplier_prod_code, \
                CAST(COALESCE(i.cost,0) AS REAL) AS cost, CAST(COALESCE(i.cost_ave,0) AS REAL) AS cost_ave, \
                CAST(COALESCE(i.price1,0) AS REAL) AS price1, CAST(COALESCE(i.pack_units,1) AS REAL) AS pack_units, \
                COALESCE(d.name, '') AS department, i.department_id, i.sub_department, i.class, i.disc_group, \
                i.non_stock, i.is_active, i.parent_upc, i.source, \
                EXISTS (SELECT 1 FROM app_overrides o WHERE o.entity_type='item' AND o.entity_key = i.upc) AS overridden \
         FROM items i \
         LEFT JOIN suppliers s ON s.id = i.supplier_id \
         LEFT JOIN departments d ON d.id = i.department_id \
         WHERE 1=1",
    );
    if !f.q.is_empty() {
        qb.push(" AND (i.upc LIKE ").push_bind(format!("%{}%", f.q))
            .push(" OR i.description LIKE ").push_bind(format!("%{}%", f.q))
            .push(" OR i.sku LIKE ").push_bind(format!("%{}%", f.q))
            .push(" OR EXISTS (SELECT 1 FROM item_barcodes ib WHERE ib.upc = i.upc AND ib.barcode LIKE ")
            .push_bind(format!("%{}%", f.q)).push("))");
    }
    if let Some(sc) = f.supplier.filter(|s| !s.is_empty()) {
        qb.push(" AND s.code = ").push_bind(sc);
    }
    if let Some(d) = f.department.filter(|s| !s.is_empty()) {
        qb.push(" AND CAST(i.department_id AS TEXT) = ").push_bind(d);
    }
    if let Some(s) = f.sub.filter(|s| !s.is_empty()) {
        qb.push(" AND i.sub_department = ").push_bind(s);
    }
    if let Some(c) = f.class.filter(|s| !s.is_empty()) {
        qb.push(" AND i.class = ").push_bind(c);
    }
    if let Some(dg) = f.disc_group.filter(|s| !s.is_empty()) {
        qb.push(" AND i.disc_group = ").push_bind(dg);
    }
    if let Some(p) = f.parent {
        match p {
            "parent" => { qb.push(" AND i.parent_upc IS NULL AND EXISTS (SELECT 1 FROM items c WHERE c.parent_upc = i.upc)"); }
            "child" => { qb.push(" AND i.parent_upc IS NOT NULL"); }
            _ => {}
        }
    }
    if let Some(ns) = f.non_stock {
        qb.push(" AND i.non_stock = ").push_bind(if ns { 1 } else { 0 });
    }
    if !f.include_inactive {
        qb.push(" AND i.is_active = 1");
    }
    qb.push(" ORDER BY i.description LIMIT 300");
    let rows: Vec<ItemSearchRow> = qb.build_query_as().fetch_all(pool).await?;
    Ok(rows.into_iter().map(|r| ItemRow {
        upc: r.upc, sku: r.sku, description: r.description, supplier_code: r.supplier_code,
        supplier_prod_code: r.supplier_prod_code,
        cost: r.cost, cost_ave: r.cost_ave, price1: r.price1, pack_units: r.pack_units,
        department: if r.department.is_empty() { None } else { Some(r.department) },
        department_id: r.department_id, sub_department: r.sub_department, class: r.class, disc_group: r.disc_group,
        non_stock: r.non_stock != 0, is_active: r.is_active != 0, parent_upc: r.parent_upc, source: r.source,
        overridden: r.overridden != 0,
    }).collect())
}

#[derive(sqlx::FromRow)]
struct ItemSearchRow {
    upc: String,
    sku: Option<String>,
    description: Option<String>,
    supplier_code: Option<String>,
    supplier_prod_code: Option<String>,
    cost: f64,
    cost_ave: f64,
    price1: f64,
    pack_units: f64,
    department: String,
    department_id: Option<i64>,
    sub_department: Option<String>,
    class: Option<String>,
    disc_group: Option<String>,
    non_stock: i64,
    is_active: i64,
    parent_upc: Option<String>,
    source: String,
    overridden: i64,
}

pub async fn get(pool: &SqlitePool, upc: &str) -> anyhow::Result<Option<ItemRow>> {
    Ok(search(pool, &SearchFilters { q: upc, include_inactive: true, ..Default::default() }).await?.into_iter().find(|r| r.upc == upc))
}

/// Apply an app edit to an existing item + record overrides + ETL export.
/// `edits` values are JSON (strings for text, numbers for numerics).
pub async fn edit_item(
    pool: &SqlitePool,
    install: &str,
    operator: &str,
    upc: &str,
    edits: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<()> {
    if edits.is_empty() {
        anyhow::bail!("no fields to edit");
    }
    for k in edits.keys() {
        if !EDITABLE.contains(&k.as_str()) {
            anyhow::bail!("field '{k}' is not editable");
        }
    }
    let mut tx = pool.begin().await?;
    apply_edits_sql(&mut tx, upc, edits).await?;
    write_overrides(&mut tx, install, upc, edits).await?;
    record_export(&mut tx, install, operator, "edit", upc).await?;
    tx.commit().await?;
    Ok(())
}

/// Clone `from_upc` → `new_upc` (user's UPC-change convention), applying any
/// edits to the NEW item. Retires the old item (SKU=OLD_<new>, inactive),
/// carries history via history_alias, sets the new item's alt barcode = old
/// UPC, and protects app-edited fields with overrides.
pub async fn clone_item(
    pool: &SqlitePool,
    install: &str,
    operator: &str,
    from_upc: &str,
    new_upc: &str,
    edits: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<()> {
    if from_upc == new_upc {
        anyhow::bail!("new UPC must differ from the source UPC");
    }
    if !new_upc.chars().all(|c| c.is_ascii_digit()) || new_upc.len() < 6 {
        anyhow::bail!("new UPC must be a ≥6-digit numeric barcode");
    }
    let src = get(pool, from_upc).await?.ok_or_else(|| anyhow::anyhow!("source item {from_upc} not found"))?;
    if get(pool, new_upc).await?.is_some() {
        anyhow::bail!("item {new_upc} already exists");
    }
    for k in edits.keys() {
        if !EDITABLE.contains(&k.as_str()) {
            anyhow::bail!("field '{k}' is not editable");
        }
    }
    let mut tx = pool.begin().await?;

    // copy the source's master row into the new UPC (with edits applied)
    let mut copy: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
    for k in ["sku", "description", "supplier_code", "supplier_prod_code", "cost", "cost_ave",
              "price1", "pack_units", "volume_ml", "tax_code", "parent_upc", "non_stock", "department"]
    {
        match k {
            "sku" => { copy.insert("sku".into(), json!(new_upc)); }
            "description" => { if let Some(d) = &src.description { copy.insert("description".into(), json!(d)); } }
            "supplier_code" => { if let Some(s) = &src.supplier_code { copy.insert("supplier_code".into(), json!(s)); } }
            "supplier_prod_code" => { if let Some(s) = &src.supplier_prod_code { copy.insert("supplier_prod_code".into(), json!(s)); } }
            "cost" => { copy.insert("cost".into(), json!(src.cost)); }
            "cost_ave" => { copy.insert("cost_ave".into(), json!(src.cost_ave)); }
            "price1" => { copy.insert("price1".into(), json!(src.price1)); }
            "pack_units" => { copy.insert("pack_units".into(), json!(src.pack_units)); }
            "parent_upc" => { copy.insert("parent_upc".into(), json!(src.parent_upc)); }
            "is_active" => { copy.insert("is_active".into(), json!(true)); }
            _ => {}
        }
    }
    for (k, v) in edits {
        copy.insert(k.clone(), v.clone());
    }
    insert_item_sql(&mut tx, new_upc, "app", &copy).await?;

    // alt barcode: new item carries the old UPC
    sqlx::query("INSERT OR IGNORE INTO item_barcodes (upc, barcode) VALUES (?1, ?2)")
        .bind(new_upc).bind(from_upc)
        .execute(&mut *tx).await
        .map_err(|e| anyhow::anyhow!("alt barcode insert: {e}"))?;

    // retire the old item (user rule): SKU = OLD_<new_upc>, inactive
    sqlx::query("UPDATE items SET sku = ?1, is_active = 0, source = 'app' WHERE upc = ?2")
        .bind(format!("OLD_{new_upc}")).bind(from_upc)
        .execute(&mut *tx).await
        .map_err(|e| anyhow::anyhow!("retire old ({from_upc}): {e}"))?;

    // history alias: forecasts/sellout for the new UPC inherit the old sales
    sqlx::query("INSERT INTO history_alias (old_upc, new_upc, created_by) VALUES (?1, ?2, ?3)")
        .bind(from_upc).bind(new_upc).bind(operator)
        .execute(&mut *tx).await
        .map_err(|e| anyhow::anyhow!("history alias: {e}"))?;

    // protect app-edited fields from the connector (O-12)
    if !edits.is_empty() {
        write_overrides(&mut tx, install, new_upc, edits).await?;
    }

    record_export(&mut tx, install, operator, "clone", new_upc).await?;
    tx.commit().await?;
    Ok(())
}

async fn insert_item_sql(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    upc: &str,
    source: &str,
    fields: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<()> {
    let strf = |k: &str| fields.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
    let numf = |k: &str| fields.get(k).and_then(|v| v.as_f64()).unwrap_or(0.0);
    let dept_id: Option<i64> = match strf("department") {
        Some(name) if !name.is_empty() => sqlx::query_scalar("SELECT id FROM departments WHERE name = ?1")
            .bind(&name).fetch_optional(&mut **tx).await?,
        _ => None,
    };
    let sup_id: Option<i64> = match strf("supplier_code") {
        Some(code) if !code.is_empty() => sqlx::query_scalar("SELECT id FROM suppliers WHERE code = ?1")
            .bind(&code).fetch_optional(&mut **tx).await?,
        _ => None,
    };
    sqlx::query(
        "INSERT INTO items (upc, source, source_key, sku, description, department_id, supplier_id, \
                parent_upc, supplier_prod_code, cost, cost_ave, purchase_cost, \
                price1, price2, price3, price4, price5, price6, price7, price8, \
                tax_code, pack_units, non_stock, is_active, last_synced_at) \
         VALUES (?1, ?2, ?1, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?9, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, 1, datetime('now'))",
    )
    .bind(upc).bind(source)
    .bind(strf("sku").unwrap_or_else(|| upc.to_string()))
    .bind(strf("description"))
    .bind(dept_id).bind(sup_id)
    .bind(strf("parent_upc"))
    .bind(strf("supplier_prod_code"))
    .bind(numf("cost")).bind(numf("cost_ave"))
    .bind(numf("price1")).bind(numf("price2")).bind(numf("price3")).bind(numf("price4"))
    .bind(numf("price5")).bind(numf("price6")).bind(numf("price7")).bind(numf("price8"))
    .bind(strf("tax_code"))
    .bind(numf("pack_units"))
    .bind(fields.get("non_stock").and_then(|v| v.as_bool()).unwrap_or(false))
    .execute(&mut **tx)
    .await
    .map_err(|e| anyhow::anyhow!("item insert ({upc}): {e}"))?;
    Ok(())
}

/// UPDATE an existing item's editable fields (all of which have local cols).
async fn apply_edits_sql(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    upc: &str,
    edits: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<()> {
    // Map edit keys → (local column, value). Supplier code needs id resolution.
    for (k, v) in edits {
        match k.as_str() {
            "supplier_code" => {
                let code = v.as_str().unwrap_or("");
                let id: Option<i64> = if code.is_empty() { None } else {
                    sqlx::query_scalar("SELECT id FROM suppliers WHERE code = ?1").bind(code)
                        .fetch_optional(&mut **tx).await?
                };
                sqlx::query("UPDATE items SET supplier_id = ?1 WHERE upc = ?2").bind(id).bind(upc)
                    .execute(&mut **tx).await?;
            }
            "department" => {
                let name = v.as_str().unwrap_or("");
                let id: Option<i64> = if name.is_empty() { None } else {
                    sqlx::query_scalar("SELECT id FROM departments WHERE name = ?1").bind(name)
                        .fetch_optional(&mut **tx).await?
                };
                sqlx::query("UPDATE items SET department_id = ?1 WHERE upc = ?2").bind(id).bind(upc)
                    .execute(&mut **tx).await?;
            }
            "is_active" => {
                let b = v.as_bool().unwrap_or(true);
                sqlx::query("UPDATE items SET is_active = ?1 WHERE upc = ?2").bind(if b { 1 } else { 0 }).bind(upc)
                    .execute(&mut **tx).await?;
            }
            "description" | "sku" | "supplier_prod_code" | "tax_code" | "parent_upc" => {
                let s = v.as_str().unwrap_or("");
                let q = match k.as_str() {
                    "description" => "UPDATE items SET description = ?1 WHERE upc = ?2",
                    "sku" => "UPDATE items SET sku = ?1 WHERE upc = ?2",
                    "supplier_prod_code" => "UPDATE items SET supplier_prod_code = ?1 WHERE upc = ?2",
                    "tax_code" => "UPDATE items SET tax_code = ?1 WHERE upc = ?2",
                    _ => "UPDATE items SET parent_upc = ?1 WHERE upc = ?2",
                };
                sqlx::query(q).bind(s).bind(upc).execute(&mut **tx).await?;
            }
            "cost" | "cost_ave" | "purchase_cost" | "price1" | "price2" | "price3" | "price4"
            | "price5" | "price6" | "price7" | "price8" | "pack_units" => {
                let n = v.as_f64().unwrap_or(0.0);
                let q = match k.as_str() {
                    "cost" => "UPDATE items SET cost = ?1 WHERE upc = ?2",
                    "cost_ave" => "UPDATE items SET cost_ave = ?1 WHERE upc = ?2",
                    "purchase_cost" => "UPDATE items SET purchase_cost = ?1 WHERE upc = ?2",
                    "price1" => "UPDATE items SET price1 = ?1 WHERE upc = ?2",
                    "price2" => "UPDATE items SET price2 = ?1 WHERE upc = ?2",
                    "price3" => "UPDATE items SET price3 = ?1 WHERE upc = ?2",
                    "price4" => "UPDATE items SET price4 = ?1 WHERE upc = ?2",
                    "price5" => "UPDATE items SET price5 = ?1 WHERE upc = ?2",
                    "price6" => "UPDATE items SET price6 = ?1 WHERE upc = ?2",
                    "price7" => "UPDATE items SET price7 = ?1 WHERE upc = ?2",
                    "price8" => "UPDATE items SET price8 = ?1 WHERE upc = ?2",
                    _ => "UPDATE items SET pack_units = ?1 WHERE upc = ?2",
                };
                sqlx::query(q).bind(n).bind(upc).execute(&mut **tx).await?;
            }
            _ => anyhow::bail!("field '{k}' is not editable"),
        }
    }
    // mark the whole row app-sourced so the connector treats it carefully
    sqlx::query("UPDATE items SET source = 'app' WHERE upc = ?1").bind(upc).execute(&mut **tx).await?;
    Ok(())
}

async fn write_overrides(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    install: &str,
    upc: &str,
    edits: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<()> {
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    for (k, v) in edits {
        if k == "supplier_code" || k == "department" {
            continue; // mapped onto *_id — no direct column override to protect
        }
        sqlx::query(
            "INSERT INTO app_overrides (entity_type, entity_key, field, value, conflict_state, updated_at, updated_by) \
             VALUES ('item', ?1, ?2, ?3, 'clean', ?4, ?5) \
             ON CONFLICT(entity_type, entity_key, field) DO UPDATE SET \
               value = excluded.value, conflict_state = 'clean', updated_at = excluded.updated_at, updated_by = excluded.updated_by",
        )
        .bind(upc).bind(k).bind(v.to_string()).bind(&now).bind(install)
        .execute(&mut **tx).await?;
    }
    Ok(())
}

/// Track the change for the ETL + audit trail.
async fn record_export(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    install: &str,
    operator: &str,
    kind: &str, // edit | clone
    upc: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO item_etl_exports (id, kind, filename, status, requested_by, verify_diff, created_at) \
         VALUES (?1, ?2, '', 'pending', ?3, ?4, datetime('now'))",
    )
    .bind(Uuid::new_v4().to_string()).bind(kind).bind(operator).bind(upc)
    .execute(&mut **tx).await?;
    let _ = install;
    Ok(())
}

// ── Facets for the items filter dropdowns ────────────────────────────────────

#[derive(Debug, serde::Serialize, Clone, Default)]
pub struct Facets {
    pub departments: Vec<FacetDept>,
    pub sub_departments: Vec<String>,
    pub classes: Vec<String>,
    pub suppliers: Vec<FacetSupplier>,
    pub disc_groups: Vec<String>,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct FacetDept {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, serde::Serialize, Clone)]
pub struct FacetSupplier {
    pub code: String,
    pub name: String,
}

pub async fn facets(pool: &SqlitePool) -> anyhow::Result<Facets> {
    let departments: Vec<(i64, String)> = sqlx::query_as(
        "SELECT d.id, COALESCE(d.name, '') FROM departments d JOIN items i ON i.department_id = d.id GROUP BY d.id ORDER BY d.name",
    ).fetch_all(pool).await?;
    let sub_departments: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT sub_department FROM items WHERE sub_department IS NOT NULL AND sub_department != '' ORDER BY 1",
    ).fetch_all(pool).await?;
    let classes: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT class FROM items WHERE class IS NOT NULL AND class != '' ORDER BY 1",
    ).fetch_all(pool).await?;
    let suppliers: Vec<(String, String)> = sqlx::query_as(
        "SELECT s.code, COALESCE(s.name, '') FROM suppliers s JOIN items i ON i.supplier_id = s.id GROUP BY s.code, s.name ORDER BY s.name",
    ).fetch_all(pool).await?;
    let disc_groups: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT disc_group FROM items WHERE disc_group IS NOT NULL AND disc_group != '' ORDER BY 1",
    ).fetch_all(pool).await?;
    Ok(Facets {
        departments: departments.into_iter().map(|(id, name)| FacetDept { id, name }).collect(),
        sub_departments,
        classes,
        suppliers: suppliers.into_iter().map(|(code, name)| FacetSupplier { code, name }).collect(),
        disc_groups,
    })
}
