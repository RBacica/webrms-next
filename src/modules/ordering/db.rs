// Ordering DB layer over the local SQLite schema: order-sheet assembly from
// sales_daily + stock_current + promo_rules, then forecast::compute_line.

use chrono::Datelike;
use sqlx::sqlite::SqlitePool;
use std::collections::HashSet;

use super::forecast::{self, LineInput, LineResult, ProductHistory, SaleDay};

/// Order-sheet line: item + forecast inputs + result.
#[derive(Debug, serde::Serialize)]
pub struct SheetLine {
    pub upc: String,
    pub description: String,
    pub department: String,
    pub pack: f64,
    pub on_hand: f64,
    pub on_order: f64,
    pub min_qty: f64,
    pub max_qty: f64,
    pub no_order: bool,
    pub unit_cost: f64,
    pub result: LineResult,
}

/// Per-supplier mode (lead/cycle) — connector-seeded, app-authoritative when set.
#[derive(Debug, Clone, Default)]
pub struct SupplierMode {
    pub lead_days: i64,
    pub cycle_days: i64,
    pub cover_days: Option<i64>,
}

pub async fn supplier_mode(pool: &SqlitePool, code: &str) -> anyhow::Result<SupplierMode> {
    let row: Option<(i64, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT lead_days, cycle_days, cover_days FROM supplier_modes WHERE supplier_code = ?1",
    )
    .bind(code)
    .fetch_optional(pool)
    .await?;
    Ok(match row {
        Some((lead, cycle, cover)) => SupplierMode {
            lead_days: lead.max(1),
            cycle_days: cycle.unwrap_or(7),
            cover_days: cover,
        },
        None => SupplierMode { lead_days: 3, cycle_days: 7, cover_days: None },
    })
}

/// Read a global boolean switch from the settings table ("true"/"1" = on).
pub async fn global_flag(pool: &SqlitePool, key: &str) -> anyhow::Result<bool> {
    let v: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?1")
        .bind(key)
        .fetch_optional(pool)
        .await?;
    Ok(matches!(v.as_deref(), Some("true") | Some("1")))
}

/// Every supplier mode row (for the settings panel).
pub async fn all_supplier_modes(pool: &SqlitePool) -> anyhow::Result<Vec<serde_json::Value>> {
    let rows: Vec<(String, String, i64, Option<i64>, Option<i64>, String)> = sqlx::query_as(
        "SELECT supplier_code, mode, lead_days, cycle_days, cover_days, source \
         FROM supplier_modes ORDER BY supplier_code",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(code, mode, lead, cycle, cover, source)| {
            serde_json::json!({ "supplier_code": code, "mode": mode, "lead_days": lead,
                                "cycle_days": cycle, "cover_days": cover, "source": source })
        })
        .collect())
}

/// Upsert one supplier's ordering mode + emit the outbox row (config-class,
/// replicated down). App-authoritative: source = 'app'.
pub async fn save_supplier_mode(
    pool: &SqlitePool,
    install: &str,
    code: &str,
    mode: &str,
    lead_days: i64,
    cycle_days: Option<i64>,
    cover_days: Option<i64>,
) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    sqlx::query(
        "INSERT INTO supplier_modes (supplier_code, mode, lead_days, cycle_days, cover_days, source, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, 'app', ?6) \
         ON CONFLICT(supplier_code) DO UPDATE SET mode=excluded.mode, lead_days=excluded.lead_days, \
           cycle_days=excluded.cycle_days, cover_days=excluded.cover_days, source='app', updated_at=excluded.updated_at",
    )
    .bind(code).bind(mode).bind(lead_days).bind(cycle_days).bind(cover_days).bind(&now)
    .execute(&mut *tx).await?;
    let payload = serde_json::json!({
        "supplier_code": code, "mode": mode, "lead_days": lead_days,
        "cycle_days": cycle_days, "cover_days": cover_days, "source": "app", "updated_at": now,
    });
    crate::replication::emit(&mut tx, install, "supplier_modes", code, "insert", &payload).await?;
    tx.commit().await?;
    Ok(())
}

/// Set one global ordering switch (whitelisted keys only) + emit outbox.
pub async fn save_global_setting(pool: &SqlitePool, install: &str, key: &str, value: &str) -> anyhow::Result<()> {
    if !matches!(key, "ignore_min_qty" | "ignore_max_qty") {
        anyhow::bail!("unknown setting key '{key}'");
    }
    if !matches!(value, "true" | "false") {
        anyhow::bail!("value must be true|false");
    }
    let mut tx = pool.begin().await?;
    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at, updated_by) VALUES (?1, ?2, ?3, ?4) \
         ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at, updated_by=excluded.updated_by",
    )
    .bind(key).bind(value).bind(&now).bind(install)
    .execute(&mut *tx).await?;
    let payload = serde_json::json!({ "key": key, "value": value, "updated_at": now, "updated_by": install });
    crate::replication::emit(&mut tx, install, "settings", key, "insert", &payload).await?;
    tx.commit().await?;
    Ok(())
}

/// Active item-master + mode defaults for one supplier.
pub struct ItemMeta {
    pub upc: String,
    pub description: String,
    pub department: String,
    pub pack: f64,
    pub min_qty: f64,
    pub max_qty: f64,
    pub no_order: bool,
    pub cost: f64,
}

/// Build the order sheet for a supplier (optionally one branch).
/// `branch = None` = all branches (stock = latest row per UPC; sales history
/// summed across branches — the per-branch-0 lookups of the old code made
/// all-branch forecasts see zero history).
///
/// Batched (2026-09-05): the sheet used to run ~4 queries PER LINE (stock,
/// on-order, sales history ×2) — ~1,800 round-trips for a 450-line supplier.
/// Now the whole supplier loads in 4 fixed queries + 1 per 400-UPC chunk,
/// and the per-line forecast loop is pure map lookups.
pub async fn order_sheet(
    pool: &SqlitePool,
    supplier_code: &str,
    branch: Option<i64>,
    active_only: bool,
) -> anyhow::Result<Vec<SheetLine>> {
    let sup_id: Option<i64> = sqlx::query_scalar("SELECT id FROM suppliers WHERE code = ?1")
        .bind(supplier_code)
        .fetch_optional(pool)
        .await?;
    let Some(sup_id) = sup_id else {
        return Ok(vec![]);
    };
    let mode = supplier_mode(pool, supplier_code).await?;
    let shrink: f64 = 0.0; // config later
    // Global switches from the settings table (app-authored, replicated down).
    let ignore_min = global_flag(pool, "ignore_min_qty").await?;
    let ignore_max = global_flag(pool, "ignore_max_qty").await?;

    // Items for this supplier (active; optional InActive filter)
    let mut qb = sqlx::QueryBuilder::new(
        "SELECT i.upc, i.description, COALESCE(d.name, ''), \
                CAST(COALESCE(i.pack_units, 1) AS REAL), \
                CAST(COALESCE(i.min_qty, 0) AS REAL), CAST(COALESCE(i.max_qty, 0) AS REAL), \
                COALESCE(i.no_order, 0), CAST(COALESCE(i.cost, 0) AS REAL) \
         FROM items i LEFT JOIN departments d ON d.id = i.department_id \
         WHERE i.supplier_id = ",
    );
    qb.push_bind(sup_id);
    if active_only {
        qb.push(" AND i.is_active = 1");
    }
    qb.push(" ORDER BY i.description");
    let items: Vec<(String, String, String, f64, f64, f64, bool, f64)> =
        qb.build_query_as().fetch_all(pool).await?;
    let upcs: Vec<&str> = items.iter().map(|i| i.0.as_str()).collect();

    // ── batched lookups (one pass per table, chunked IN-lists) ──────────────
    let today = chrono::Local::now().date_naive();
    let today_dow = today.weekday().num_days_from_monday() as i64;

    // stock
    let stock = if let Some(b) = branch {
        let mut m = std::collections::HashMap::new();
        for chunk in upcs.chunks(400) {
            let mut q = sqlx::QueryBuilder::new(
                "SELECT upc, CAST(COALESCE(qty,0) AS REAL) FROM stock_current \
                 WHERE branch_id = ",
            );
            q.push_bind(b).push(" AND upc IN (");
            let mut sep = q.separated(", ");
            for u in chunk {
                sep.push_bind(*u);
            }
            q.push(")");
            for (upc, qty) in q.build_query_as::<(String, f64)>().fetch_all(pool).await? {
                m.insert(upc, qty);
            }
        }
        m
    } else {
        // latest stock_current row per UPC (matches the old per-upc
        // ORDER BY id DESC LIMIT 1)
        let mut m = std::collections::HashMap::new();
        for chunk in upcs.chunks(400) {
            let mut q = sqlx::QueryBuilder::new(
                "SELECT upc, CAST(qty AS REAL) FROM stock_current \
                 WHERE id IN (SELECT MAX(id) FROM stock_current GROUP BY upc) AND upc IN (",
            );
            let mut sep = q.separated(", ");
            for u in chunk {
                sep.push_bind(*u);
            }
            q.push(")");
            for (upc, qty) in q.build_query_as::<(String, f64)>().fetch_all(pool).await? {
                m.insert(upc, qty);
            }
        }
        m
    };

    // on-order (only meaningful branch-scoped; the old code returned 0 for
    // branch=None)
    let mut on_order: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
    if let Some(b) = branch {
        for chunk in upcs.chunks(400) {
            let mut q = sqlx::QueryBuilder::new(
                "SELECT ol.upc, CAST(COALESCE(SUM(ol.qty),0) AS REAL) \
                 FROM order_lines ol JOIN orders o ON o.id = ol.order_id \
                 WHERE o.branch_id = ",
            );
            q.push_bind(b)
                .push(" AND o.status != 'cleared' AND ol.upc IN (");
            let mut sep = q.separated(", ");
            for u in chunk {
                sep.push_bind(*u);
            }
            q.push(") GROUP BY ol.upc");
            for (upc, qty) in q.build_query_as::<(String, f64)>().fetch_all(pool).await? {
                on_order.insert(upc, qty);
            }
        }
    }

    // sales history (92 days): branch-scoped, or ALL branches summed per day
    let from = (today - chrono::Duration::days(91)).format("%Y-%m-%d").to_string();
    let mut sales_rows: Vec<(String, String, f64, f64)> = Vec::new();
    for chunk in upcs.chunks(400) {
        let mut q = sqlx::QueryBuilder::new(
            "SELECT upc, sale_date, CAST(SUM(units) AS REAL), CAST(SUM(promo_units) AS REAL) \
             FROM sales_daily \
             WHERE sale_date >= ",
        );
        q.push_bind(&from);
        if let Some(b) = branch {
            q.push(" AND branch_id = ").push_bind(b);
        }
        q.push(" AND upc IN (");
        let mut sep = q.separated(", ");
        for u in chunk {
            sep.push_bind(*u);
        }
        // GROUP BY collapses multi-branch rows per (upc, day) when branch=None
        q.push(") GROUP BY upc, sale_date ORDER BY sale_date ASC");
        sales_rows.extend(q.build_query_as::<(String, String, f64, f64)>().fetch_all(pool).await?);
    }
    // per-upc day rows (already one row per upc+day from the GROUP BY)
    let mut daily_by_upc: std::collections::HashMap<String, Vec<(String, f64, f64)>> =
        std::collections::HashMap::new();
    for (upc, date, units, promo_units) in &sales_rows {
        daily_by_upc
            .entry(upc.clone())
            .or_default()
            .push((date.clone(), *units, *promo_units));
    }

    // promo windows (next 60 days) for this supplier's UPCs
    let mut promo_by_upc: std::collections::HashMap<String, Vec<(Option<String>, Option<String>)>> =
        std::collections::HashMap::new();
    for chunk in upcs.chunks(400) {
        let mut q = sqlx::QueryBuilder::new(
            "SELECT sequence_match, effective_start, effective_end FROM promo_rules \
             WHERE is_active = 1 AND (effective_end IS NULL OR effective_end >= ",
        );
        q.push_bind(today.format("%Y-%m-%d").to_string())
            .push(") AND sequence_match IN (");
        let mut sep = q.separated(", ");
        for u in chunk {
            sep.push_bind(*u);
        }
        q.push(")");
        for (upc, s, e) in q
            .build_query_as::<(String, Option<String>, Option<String>)>()
            .fetch_all(pool)
            .await?
        {
            promo_by_upc.entry(upc).or_default().push((s, e));
        }
    }

    let mut lines = Vec::new();
    for (upc, description, department, pack, min_qty, max_qty, no_order, cost) in items {
        let hist = assemble_history(
            today,
            daily_by_upc.get(&upc).map(|v| v.as_slice()).unwrap_or(&[]),
            promo_by_upc.get(&upc).map(|v| v.as_slice()).unwrap_or(&[]),
        );
        let cover = mode.cover_days.unwrap_or(mode.lead_days + mode.cycle_days);
        let inp = LineInput {
            on_hand: stock.get(&upc).copied().unwrap_or(0.0),
            on_order: on_order.get(&upc).copied().unwrap_or(0.0),
            pack_size: pack,
            min_qty,
            max_qty,
            no_order,
            lead_days: mode.lead_days,
            cover_days: cover,
            shrink_pct: shrink,
            promo_uplift_default: 1.5,
            dept_rate: 0.0,
            ignore_min_qty: ignore_min,
            ignore_max_qty: ignore_max,
        };
        let result = forecast::compute_line(&inp, &hist, today_dow);
        lines.push(SheetLine {
            upc,
            description,
            department,
            pack,
            on_hand: inp.on_hand,
            on_order: inp.on_order,
            min_qty,
            max_qty,
            no_order,
            unit_cost: cost,
            result,
        });
    }
    Ok(lines)
}

/// Pure assembly of a ProductHistory from raw daily-sales + promo-window rows.
/// Extracted so the batched sheet path and any single-UPC caller share the
/// exact same offset/promo-day semantics.
fn assemble_history(
    today: chrono::NaiveDate,
    daily_rows: &[(String, f64, f64)], // (sale_date, units, promo_units)
    promo_rows: &[(Option<String>, Option<String>)], // (start, end)
) -> ProductHistory {
    let mut daily: Vec<SaleDay> = Vec::new();
    let mut promo_days: HashSet<i64> = HashSet::new();
    for (date, units, promo_units) in daily_rows {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d") {
            let offset = (d - today).num_days();
            if offset <= 0 {
                daily.push(SaleDay { offset, units: *units });
                if *promo_units > 0.0 {
                    promo_days.insert(offset);
                }
            }
        }
    }
    let mut upcoming_promos: Vec<(i64, i64)> = Vec::new();
    for (s, e) in promo_rows {
        let s_d = s.as_deref().and_then(|x| chrono::NaiveDate::parse_from_str(x, "%Y-%m-%d").ok());
        let e_d = e.as_deref().and_then(|x| chrono::NaiveDate::parse_from_str(x, "%Y-%m-%d").ok());
        if let Some(sd) = s_d {
            let start = (sd - today).num_days();
            let end = e_d.map(|ed| (ed - today).num_days()).unwrap_or(60);
            upcoming_promos.push((start.max(0), end.max(0)));
        }
    }
    ProductHistory { daily, promo_days, upcoming_promos }
}
