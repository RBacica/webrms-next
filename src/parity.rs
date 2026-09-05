// P4 `webrms-next parity` — the cutover gate (DESIGN §8 P4): re-pull the
// live AKPOS counts/totals table-by-table (same connector methods the seed
// uses) and compare against what the local DB holds. Zero diff on every
// row-count = the local DB is a faithful copy → safe to cut over.
//
// The live side re-uses the Connector trait (probe/pull_*), so parity checks
// the EXACT same filter the seed/ingest uses — a count mismatch can only be
// real drift, never a filter disagreement.
//
// sales_daily is aggregated (branch/upc/day) and 36-mo deep — not re-pulled
// here; its local coverage (row count + date span) is reported as info.

use std::collections::BTreeMap;
use std::path::Path;

use sqlx::sqlite::SqlitePool;

use crate::config::AppConfig;
use crate::connector::hos::HosConnector;
use crate::connector::{Connector, HighWater};

#[derive(Debug, Clone)]
pub struct Row<'a> {
    pub table: &'a str,
    pub live: i64,
    pub local: i64,
}

pub async fn local_count(pool: &SqlitePool, sql: &'static str) -> anyhow::Result<i64> {
    // `sql` is always one of the static literals in table_defs() — sqlx's
    // audit accepts static strings; never build table names at runtime.
    Ok(sqlx::query_scalar(sql).fetch_one(pool).await?)
}

/// Count live rows table-by-table using the connector's own pulls.
async fn live_counts(conn: &HosConnector) -> anyhow::Result<BTreeMap<String, i64>> {
    let mut m = BTreeMap::new();

    m.insert("branches".into(), conn.pull_branches().await?.len() as i64);
    m.insert("departments".into(), conn.pull_departments().await?.len() as i64);
    m.insert("suppliers".into(), conn.pull_suppliers().await?.len() as i64);

    let hw0 = |table: &str| HighWater {
        source: "parity".into(),
        table: table.into(),
        last_key: None,
    };

    let mut items = 0i64;
    let mut hw = hw0("items");
    loop {
        let b = conn.pull_items(&hw, 5000).await?;
        items += b.rows.len() as i64;
        match b.next_key {
            Some(k) => hw.last_key = Some(k),
            None => break,
        }
        if b.rows.len() < 5000 {
            break;
        }
    }
    m.insert("items".into(), items);

    let mut receipts = 0i64;
    let mut hw = hw0("receipts");
    loop {
        let b = conn.pull_receipts(&hw, 5000).await?;
        receipts += b.rows.len() as i64;
        match b.next_key {
            Some(k) => hw.last_key = Some(k),
            None => break,
        }
        if b.rows.len() < 5000 {
            break;
        }
    }
    m.insert("receipts".into(), receipts);

    let mut ap = 0i64;
    let mut hw = hw0("ap");
    loop {
        let b = conn.pull_ap(&hw, 5000).await?;
        ap += b.rows.len() as i64;
        match b.next_key {
            Some(k) => hw.last_key = Some(k),
            None => break,
        }
        if b.rows.len() < 5000 {
            break;
        }
    }
    m.insert("ap_invoices".into(), ap);

    m.insert("promo_rules".into(), conn.pull_promos().await?.len() as i64);
    m.insert("pricing_groups".into(), conn.pull_pricing_groups().await?.len() as i64);
    m.insert("pricing_sets".into(), conn.pull_pricing_sets().await?.len() as i64);

    Ok(m)
}

/// Which local tables/active-filters correspond to each live pull.
/// (label, static local count SQL) — mirrors the live connector filters.
fn table_defs() -> Vec<(&'static str, &'static str)> {
    vec![
        ("branches", "SELECT COUNT(*) FROM branches WHERE is_active = 1"),
        ("departments", "SELECT COUNT(*) FROM departments WHERE is_active = 1"),
        ("suppliers", "SELECT COUNT(*) FROM suppliers WHERE is_active = 1"),
        ("items", "SELECT COUNT(*) FROM items"),
        ("receipts", "SELECT COUNT(*) FROM receipts"),
        ("ap_invoices", "SELECT COUNT(*) FROM ap_invoices"),
        ("promo_rules", "SELECT COUNT(*) FROM promo_rules WHERE is_active = 1"),
        ("pricing_groups", "SELECT COUNT(*) FROM pricing_groups WHERE is_active = 1"),
        ("pricing_sets", "SELECT COUNT(*) FROM pricing_sets"),
    ]
}

/// Local sales coverage info (aggregated table — no live side).
async fn sales_coverage(pool: &SqlitePool) -> anyhow::Result<(i64, Option<String>, Option<String>)> {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sales_daily").fetch_one(pool).await?;
    let min: Option<String> = sqlx::query_scalar("SELECT MIN(sale_date) FROM sales_daily").fetch_one(pool).await?;
    let max: Option<String> = sqlx::query_scalar("SELECT MAX(sale_date) FROM sales_daily").fetch_one(pool).await?;
    Ok((n, min, max))
}

/// Run parity: live vs local row counts. Returns (rows, clean).
pub async fn run(pool: &SqlitePool, cfg: &AppConfig, data_dir: &Path) -> anyhow::Result<(Vec<Row<'static>>, bool)> {
    let _ = data_dir;
    if cfg.database.connection_string.is_empty() {
        anyhow::bail!("no [database] connection_string — parity compares against a live AKPOS; run it on an install with a connector");
    }
    let conn = HosConnector::new(cfg.database.connection_string.clone());
    println!("  parity: pulling live counts from AKPOS (this can take ~30s)…");
    let live = live_counts(&conn).await?;

    let defs = table_defs();
    let mut rows = Vec::new();
    let mut clean = true;
    for (table, sql) in defs {
        let lv = *live.get(table).unwrap_or(&-1);
        let lc = local_count(pool, sql).await?;
        let delta = lv - lc;
        if delta != 0 {
            clean = false;
        }
        rows.push(Row {
            table,
            live: lv,
            local: lc,
        });
    }

    // sales coverage info (no live side)
    if let Ok((n, min, max)) = sales_coverage(pool).await {
        println!(
            "  info: sales_daily (aggregated, not live-compared) = {n} rows covering {} → {}",
            min.as_deref().unwrap_or("-"),
            max.as_deref().unwrap_or("-")
        );
    }
    Ok((rows, clean))
}
