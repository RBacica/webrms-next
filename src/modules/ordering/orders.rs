// Orders lifecycle (G-10) over the local SQLite schema.
// Ported from WebRMS orders.json semantics: open → receipted → cleared;
// active on-order = SUM of open+receipted minus cleared (never double-counts
// with SOH once goods land).

use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

#[derive(Debug, Clone, serde::Serialize)]
pub struct OrderLine {
    pub upc: String,
    pub qty: f64,
    pub unit_cost: f64,
    pub line_total: f64,
    pub suggested_qty: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Order {
    pub id: String,
    pub origin_install: String,
    pub branch_id: i64,
    pub supplier_id: Option<i64>,
    pub supplier_code: Option<String>,
    pub placed_at: String,
    pub status: String,
    pub total_qty: f64,
    pub total_cost: f64,
    pub created_by: Option<String>,
    pub lines: Vec<OrderLine>,
}

/// Total active on-order quantity for a (branch, upc): all non-cleared order
/// lines for that item. Receipted-but-not-cleared still counts (goods may be
/// on the way / in the back room) — cleared means fully received & verified.
pub async fn active_on_order(pool: &SqlitePool, branch_id: i64, upc: &str) -> anyhow::Result<f64> {
    let qty: Option<f64> = sqlx::query_scalar(
        "SELECT COALESCE(SUM(ol.qty), 0) \
         FROM order_lines ol \
         JOIN orders o ON o.id = ol.order_id \
         WHERE o.branch_id = ?1 AND ol.upc = ?2 AND o.status != 'cleared'",
    )
    .bind(branch_id)
    .bind(upc)
    .fetch_optional(pool)
    .await?;
    Ok(qty.unwrap_or(0.0))
}

/// Create an order + lines, generating the PO. Returns the order id.
/// Status starts 'open'. The ETL file + incoming-PO tracking is the caller's
/// job (handlers layer) — this is pure persistence.
pub async fn create_order(
    pool: &SqlitePool,
    origin_install: &str,
    branch_id: i64,
    supplier_id: Option<i64>,
    created_by: Option<&str>,
    lines: &[(String, f64, f64, f64)], // (upc, qty, unit_cost, suggested)
) -> anyhow::Result<String> {
    let id = Uuid::new_v4().to_string();
    let placed_at = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let total_qty: f64 = lines.iter().map(|l| l.1).sum();
    let total_cost: f64 = lines.iter().map(|l| l.1 * l.2).sum();

    let mut tx = pool.begin().await?;
    sqlx::query(
        "INSERT INTO orders (id, origin_install, branch_id, supplier_id, placed_at, \
                status, total_qty, total_cost, created_by) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'open', ?6, ?7, ?8)",
    )
    .bind(&id)
    .bind(origin_install)
    .bind(branch_id)
    .bind(supplier_id)
    .bind(&placed_at)
    .bind(total_qty)
    .bind(total_cost)
    .bind(created_by)
    .execute(&mut *tx)
    .await?;

    for (upc, qty, unit_cost, suggested) in lines {
        let line_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO order_lines (id, order_id, upc, qty, unit_cost, line_total, suggested_qty) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .bind(&line_id)
        .bind(&id)
        .bind(upc)
        .bind(qty)
        .bind(unit_cost)
        .bind(qty * unit_cost)
        .bind(suggested)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(id)
}

/// Mark an order cleared once its PO is fully receipted (P+G present).
/// Removes it from active on-order (G-10).
pub async fn clear_order(pool: &SqlitePool, order_id: &str) -> anyhow::Result<()> {
    sqlx::query("UPDATE orders SET status = 'cleared', cleared_at = datetime('now') WHERE id = ?1")
        .bind(order_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// List orders (optionally filtered by branch + status).
pub async fn list_orders(
    pool: &SqlitePool,
    branch_id: Option<i64>,
    status: Option<&str>,
) -> anyhow::Result<Vec<Order>> {
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT o.id, o.origin_install, o.branch_id, o.supplier_id, COALESCE(s.code, ''), \
                o.placed_at, o.status, o.total_qty, o.total_cost, o.created_by \
         FROM orders o LEFT JOIN suppliers s ON s.id = o.supplier_id",
    );
    let mut sep = " WHERE";
    if let Some(b) = branch_id {
        qb.push(sep).push(" o.branch_id = ").push_bind(b);
        sep = " AND";
    }
    if let Some(st) = status {
        qb.push(sep).push(" o.status = ").push_bind(st);
    }
    qb.push(" ORDER BY o.placed_at DESC");

    let rows: Vec<(String, String, i64, Option<i64>, String, String, String, f64, f64, Option<String>)> =
        qb.build_query_as().fetch_all(pool).await?;
    let mut orders = Vec::new();
    for (id, origin, branch_id, supplier_id, supplier_code, placed_at, status, tq, tc, by) in rows {
        let lines: Vec<(String, f64, f64, f64, f64)> = sqlx::query_as(
            "SELECT upc, qty, unit_cost, line_total, suggested_qty FROM order_lines WHERE order_id = ?1 ORDER BY upc",
        )
        .bind(&id)
        .fetch_all(pool)
        .await?;
        orders.push(Order {
            id,
            origin_install: origin,
            branch_id,
            supplier_id,
            supplier_code: if supplier_code.is_empty() { None } else { Some(supplier_code) },
            placed_at,
            status,
            total_qty: tq,
            total_cost: tc,
            created_by: by,
            lines: lines
                .into_iter()
                .map(|(upc, qty, unit_cost, line_total, suggested_qty)| OrderLine {
                    upc,
                    qty,
                    unit_cost,
                    line_total,
                    suggested_qty,
                })
                .collect(),
        });
    }
    Ok(orders)
}
