// Stocktake DB layer over the local SQLite schema.
// Ported from WebRMS src/modules/stocktake/db.rs — same endpoint shapes, but
// reads now come from items / stock_current / item_barcodes (connector-fed).

use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePool;

/// Deserialize an optional branch id, treating an empty/whitespace value as
/// `None` (all branches) rather than failing.
fn de_opt_branch<'de, D>(d: D) -> Result<Option<i32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = Option::<String>::deserialize(d)?;
    match s.as_deref() {
        None | Some("") => Ok(None),
        Some(v) => v
            .trim()
            .parse::<i32>()
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct StockItem {
    pub upc: String,
    pub description: String,
    pub department: String,
    pub supplier: String,
    pub stock_on_hand: f64,
    pub parent_upc: String,
    pub selling_qty: f64,
}

#[derive(Debug, Deserialize)]
pub struct SearchQuery {
    pub department: Option<String>,
    pub supplier: Option<String>,
    pub sub_department: Option<String>,
    #[serde(default, deserialize_with = "de_opt_branch")]
    pub branch: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpcQuery {
    pub upc: Option<String>,
    #[serde(default, deserialize_with = "de_opt_branch")]
    pub branch: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct BarcodeQuery {
    pub barcode: String,
    #[serde(default, deserialize_with = "de_opt_branch")]
    pub branch: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct SubDeptQuery {
    pub department: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeptQuery {
    pub department: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SubDepartment {
    pub id: String,
    pub description: String,
    pub dep_id: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Department {
    pub department: String,
    pub label: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Supplier {
    pub supplier: String,
    pub label: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SaveRow {
    pub upc: String,
    pub description: String,
    #[serde(default)]
    pub department: String,
    #[serde(default)]
    pub supplier: String,
    pub stock_on_hand: f64,
    pub count: f64,
    pub variance: f64,
    #[serde(default)]
    pub has_ticket: bool,
    #[serde(default = "default_ticket_qty")]
    pub ticket_qty: i32,
}

fn default_ticket_qty() -> i32 {
    1
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SaveRequest {
    pub rows: Vec<SaveRow>,
    /// "server" | "client"
    pub destination: String,
}

// ── queries over the local SQLite schema ───────────────────────────────────

pub async fn get_departments(pool: &SqlitePool) -> anyhow::Result<Vec<Department>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, name FROM departments WHERE is_active = 1 ORDER BY id",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name)| Department {
            department: id.to_string(),
            label: format!("{name} ({id})"),
        })
        .collect())
}

pub async fn get_suppliers(pool: &SqlitePool) -> anyhow::Result<Vec<Supplier>> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT code, name FROM suppliers WHERE is_active = 1 ORDER BY code",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(code, name)| Supplier {
            supplier: code.clone(),
            label: format!("{code} - {name}"),
        })
        .collect())
}

pub async fn get_suppliers_for_department(pool: &SqlitePool, dept: &str) -> anyhow::Result<Vec<Supplier>> {
    let dept_id: Option<i64> = if dept == "ALL" {
        None
    } else {
        dept.parse().ok()
    };
    let rows: Vec<(String, String)> = match dept_id {
        Some(d) => sqlx::query_as(
            "SELECT DISTINCT s.code, s.name FROM suppliers s \
             JOIN items i ON i.supplier_id = s.id \
             WHERE i.department_id = ?1 AND i.is_active = 1 ORDER BY s.code",
        )
        .bind(d)
        .fetch_all(pool)
        .await?,
        None => sqlx::query_as(
            "SELECT code, name FROM suppliers WHERE is_active = 1 ORDER BY code",
        )
        .fetch_all(pool)
        .await?,
    };
    Ok(rows
        .into_iter()
        .map(|(code, name)| Supplier {
            supplier: code.clone(),
            label: format!("{code} - {name}"),
        })
        .collect())
}

pub async fn get_sub_departments(pool: &SqlitePool, dept: &str) -> anyhow::Result<Vec<SubDepartment>> {
    let dept_id: Option<i64> = if dept.is_empty() || dept == "ALL" {
        None
    } else {
        dept.parse().ok()
    };
    let rows: Vec<(String, String, String)> = match dept_id {
        Some(d) => sqlx::query_as(
            "SELECT DISTINCT sub_department, sub_department, CAST(department_id AS TEXT) \
             FROM items WHERE department_id = ?1 AND is_active = 1 AND sub_department IS NOT NULL \
             ORDER BY sub_department",
        )
        .bind(d)
        .fetch_all(pool)
        .await?,
        None => sqlx::query_as(
            "SELECT DISTINCT sub_department, sub_department, CAST(department_id AS TEXT) \
             FROM items WHERE is_active = 1 AND sub_department IS NOT NULL \
             ORDER BY sub_department",
        )
        .fetch_all(pool)
        .await?,
    };
    Ok(rows
        .into_iter()
        .map(|(id, description, dep_id)| SubDepartment { id, description, dep_id })
        .collect())
}

/// Search items with stock-on-hand from stock_current (G-1). `branch = None`
/// = all branches (latest stock per UPC wins via MAX(id) self-join).
/// Uses sqlx::QueryBuilder — the audit-compliant dynamic-SQL API (sqlx 0.9
/// rejects raw format!-built strings).
pub async fn search_items(
    pool: &SqlitePool,
    dept: &str,
    supplier: &str,
    sub_dept: &str,
    branch: Option<i32>,
) -> anyhow::Result<Vec<StockItem>> {
    let dept_id: Option<i64> = if dept == "ALL" { None } else { dept.parse().ok() };
    let sup_id: Option<i64> = if supplier == "ALL" {
        None
    } else {
        sqlx::query_scalar("SELECT id FROM suppliers WHERE code = ?1")
            .bind(supplier)
            .fetch_optional(pool)
            .await?
    };
    let sub = if sub_dept == "ALL" { None } else { Some(sub_dept.to_string()) };

    let mut qb = sqlx::QueryBuilder::new(
        "SELECT i.upc, i.description, COALESCE(CAST(i.department_id AS TEXT), ''), \
                COALESCE(s.name, ''), CAST(COALESCE(sc.qty, 0) AS REAL), COALESCE(i.parent_upc, ''), \
                CAST(COALESCE(i.pack_units, 1) AS REAL) \
         FROM items i \
         LEFT JOIN suppliers s ON s.id = i.supplier_id ",
    );
    // Stock join: per-branch filter or latest-across-branches
    match branch {
        Some(b) => {
            qb.push("LEFT JOIN stock_current sc ON sc.upc = i.upc AND sc.branch_id = ");
            qb.push_bind(b);
        }
        None => {
            qb.push(
                "LEFT JOIN (SELECT upc, qty FROM stock_current sc \
                   WHERE sc.id = (SELECT MAX(id) FROM stock_current s2 \
                                  WHERE s2.upc = sc.upc)) sc ON sc.upc = i.upc ",
            );
        }
    }
    qb.push(" WHERE i.is_active = 1");
    if let Some(d) = dept_id {
        qb.push(" AND i.department_id = ").push_bind(d);
    }
    if let Some(s) = sup_id {
        qb.push(" AND i.supplier_id = ").push_bind(s);
    }
    if let Some(sd) = &sub {
        qb.push(" AND i.sub_department = ").push_bind(sd);
    }
    qb.push(" ORDER BY i.description ASC");

    let rows: Vec<(String, String, String, String, f64, Option<String>, f64)> = qb
        .build_query_as()
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(upc, description, department, supplier, stock_on_hand, parent_upc, selling_qty)| {
            StockItem {
                upc,
                description,
                department,
                supplier,
                stock_on_hand,
                parent_upc: parent_upc.unwrap_or_default(),
                selling_qty,
            }
        })
        .collect())
}

/// Refresh a single UPC's stock-on-hand.
pub async fn refresh_upc(pool: &SqlitePool, upc: &str, branch: Option<i32>) -> anyhow::Result<f64> {
    match branch {
        Some(b) => Ok(sqlx::query_scalar(
            "SELECT qty FROM stock_current WHERE upc = ?1 AND branch_id = ?2",
        )
        .bind(upc)
        .bind(b)
        .fetch_optional(pool)
        .await?
        .unwrap_or(0.0)),
        None => Ok(sqlx::query_scalar(
            "SELECT qty FROM stock_current WHERE upc = ?1 \
             ORDER BY id DESC LIMIT 1",
        )
        .bind(upc)
        .fetch_optional(pool)
        .await?
        .unwrap_or(0.0)),
    }
}

/// Barcode lookup: primary UPC lives in items.upc; alt barcodes in
/// item_barcodes (WebRMS pitfall: primary UPCs are usually NOT duplicated in
/// ItemBarcodes — fall back to treating the scan as the UPC).
pub async fn barcode_lookup_upcs(pool: &SqlitePool, barcode: &str) -> anyhow::Result<Vec<String>> {
    let alt: Vec<String> = sqlx::query_scalar(
        "SELECT upc FROM item_barcodes WHERE barcode = ?1",
    )
    .bind(barcode)
    .fetch_all(pool)
    .await?;
    if !alt.is_empty() {
        return Ok(alt);
    }
    // Fallback: the scanned value IS the primary UPC
    let primary: Option<String> = sqlx::query_scalar("SELECT upc FROM items WHERE upc = ?1 AND is_active = 1")
        .bind(barcode)
        .fetch_optional(pool)
        .await?;
    Ok(primary.map(|u| vec![u]).unwrap_or_default())
}
