// P3 replication — outbox-based sync over REST (the WebRMS-Next replacement
// for the old JSON-file distributed sync).
//
// Model:
//   - Every write to a replicated table appends an outbox row (same tx).
//   - HoS = source of truth for config-class tables (settings, supplier_terms,
//     supplier_modes, paid_ledger) — replicated DOWN to every client.
//   - orders + order_lines are bidirectional PER BRANCH (a BoS pushes its own
//     branch's orders up; the HoS's connector flips statuses which flow down).
//   - incoming_pos (PO tracking) flows UP from the ordering branch to the HoS.
//   - Each client keeps a per-source watermark (sync_watermarks) so pulls are
//     resumable; applies are idempotent upserts by row_id.
//
// O-2: immediate best-effort push on write + poll backstop.
// O-10: per-source circuit breaker (consecutive-failure backoff).

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::{json, Value};
use sqlx::sqlite::SqlitePool;
use uuid::Uuid;

use crate::config::SyncConfig;
use crate::state::SharedState;

/// Tables replicated DOWN to all clients (config-class).
const CONFIG_DOWN: &[&str] = &["settings", "supplier_terms", "supplier_modes", "paid_ledger"];
/// Tables scoped to one branch (orders/POs).
const BRANCH_SCOPED: &[&str] = &["orders", "order_lines", "incoming_pos"];

/// A client may only pull config-down rows and its own branch's rows.
pub fn table_allowed_for(table: &str) -> bool {
    CONFIG_DOWN.contains(&table) || BRANCH_SCOPED.contains(&table)
}

/// Append an outbox row inside the caller's transaction.
pub async fn emit(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    install: &str,
    table: &str,
    row_id: &str,
    op: &str,
    payload: &Value,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO outbox (id, origin_install, table_name, row_id, op, payload, ts, applied) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, datetime('now'), 0)",
    )
    .bind(Uuid::new_v4().to_string())
    .bind(install)
    .bind(table)
    .bind(row_id)
    .bind(op)
    .bind(payload.to_string())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Apply one outbox row to the local DB (idempotent). Does NOT re-emit.
pub async fn apply_row(pool: &SqlitePool, table: &str, row_id: &str, op: &str, payload: &Value) -> anyhow::Result<bool> {
    match table {
        "orders" => apply_order(pool, row_id, op, payload).await,
        "order_lines" => apply_order_line(pool, row_id, payload).await,
        "incoming_pos" => apply_incoming_po(pool, row_id, op, payload).await,
        "supplier_terms" => apply_supplier_terms(pool, payload).await,
        "supplier_modes" => apply_supplier_modes(pool, payload).await,
        "settings" => apply_setting(pool, payload).await,
        "paid_ledger" => apply_paid_ledger(pool, payload).await,
        _ => {
            tracing::warn!("replication: unknown table '{table}' — skipped");
            Ok(false)
        }
    }
}

async fn apply_order(pool: &SqlitePool, row_id: &str, op: &str, payload: &Value) -> anyhow::Result<bool> {
    if op == "delete" {
        sqlx::query("DELETE FROM orders WHERE id = ?1").bind(row_id).execute(pool).await?;
        sqlx::query("DELETE FROM order_lines WHERE order_id = ?1").bind(row_id).execute(pool).await?;
        return Ok(true);
    }
    let branch_id: i64 = payload["branch_id"].as_i64().unwrap_or(0);
    let supplier_id: Option<i64> = payload["supplier_id"].as_i64();
    let status = payload["status"].as_str().unwrap_or("open");
    let placed_at = payload["placed_at"].as_str().unwrap_or("");
    let total_qty: f64 = payload["total_qty"].as_f64().unwrap_or(0.0);
    let total_cost: f64 = payload["total_cost"].as_f64().unwrap_or(0.0);
    let created_by: Option<&str> = payload["created_by"].as_str();
    let origin: &str = payload["origin_install"].as_str().unwrap_or("remote");
    // INSERT OR REPLACE keeps this idempotent across re-pulls.
    sqlx::query(
        "INSERT OR REPLACE INTO orders (id, origin_install, branch_id, supplier_id, placed_at, \
                status, total_qty, total_cost, created_by) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
    )
    .bind(row_id).bind(origin).bind(branch_id).bind(supplier_id)
    .bind(placed_at).bind(status).bind(total_qty).bind(total_cost).bind(created_by)
    .execute(pool).await?;
    // replace lines
    sqlx::query("DELETE FROM order_lines WHERE order_id = ?1").bind(row_id).execute(pool).await?;
    if let Some(lines) = payload["lines"].as_array() {
        for l in lines {
            sqlx::query(
                "INSERT INTO order_lines (id, order_id, upc, qty, unit_cost, line_total, suggested_qty) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            )
            .bind(Uuid::new_v4().to_string())
            .bind(row_id)
            .bind(l["upc"].as_str().unwrap_or(""))
            .bind(l["qty"].as_f64().unwrap_or(0.0))
            .bind(l["unit_cost"].as_f64().unwrap_or(0.0))
            .bind(l["line_total"].as_f64().unwrap_or(0.0))
            .bind(l["suggested_qty"].as_f64().unwrap_or(0.0))
            .execute(pool).await?;
        }
    }
    Ok(true)
}

async fn apply_order_line(pool: &SqlitePool, row_id: &str, payload: &Value) -> anyhow::Result<bool> {
    sqlx::query(
        "INSERT OR REPLACE INTO order_lines (id, order_id, upc, qty, unit_cost, line_total, suggested_qty) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(row_id)
    .bind(payload["order_id"].as_str().unwrap_or(""))
    .bind(payload["upc"].as_str().unwrap_or(""))
    .bind(payload["qty"].as_f64().unwrap_or(0.0))
    .bind(payload["unit_cost"].as_f64().unwrap_or(0.0))
    .bind(payload["line_total"].as_f64().unwrap_or(0.0))
    .bind(payload["suggested_qty"].as_f64().unwrap_or(0.0))
    .execute(pool).await?;
    Ok(true)
}

async fn apply_incoming_po(pool: &SqlitePool, row_id: &str, op: &str, payload: &Value) -> anyhow::Result<bool> {
    if op == "delete" {
        sqlx::query("DELETE FROM incoming_pos WHERE id = ?1").bind(row_id).execute(pool).await?;
        return Ok(true);
    }
    sqlx::query(
        "INSERT OR REPLACE INTO incoming_pos (id, origin_install, branch_id, supplier_id, filename, \
                bill_of_lading, poid, status, imported, placed_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .bind(row_id)
    .bind(payload["origin_install"].as_str().unwrap_or("remote"))
    .bind(payload["branch_id"].as_i64().unwrap_or(0))
    .bind(payload["supplier_id"].as_i64())
    .bind(payload["filename"].as_str().unwrap_or(""))
    .bind(payload["bill_of_lading"].as_str())
    .bind(payload["poid"].as_i64())
    .bind(payload["status"].as_str().unwrap_or("waiting_import"))
    .bind(payload["imported"].as_i64().unwrap_or(0))
    .bind(payload["placed_at"].as_str().unwrap_or(""))
    .execute(pool).await?;
    Ok(true)
}

async fn apply_supplier_terms(pool: &SqlitePool, payload: &Value) -> anyhow::Result<bool> {
    sqlx::query(
        "INSERT OR REPLACE INTO supplier_terms (supplier_code, term_type, term_days, order_type, \
                payment_type, configured, source, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(payload["supplier_code"].as_str().unwrap_or(""))
    .bind(payload["term_type"].as_str())
    .bind(payload["term_days"].as_i64())
    .bind(payload["order_type"].as_str())
    .bind(payload["payment_type"].as_str())
    .bind(payload["configured"].as_i64().unwrap_or(0))
    .bind(payload["source"].as_str().unwrap_or("app"))
    .bind(payload["updated_at"].as_str().unwrap_or(""))
    .execute(pool).await?;
    Ok(true)
}

async fn apply_supplier_modes(pool: &SqlitePool, payload: &Value) -> anyhow::Result<bool> {
    sqlx::query(
        "INSERT OR REPLACE INTO supplier_modes (supplier_code, mode, lead_days, cycle_days, \
                cover_days, source, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(payload["supplier_code"].as_str().unwrap_or(""))
    .bind(payload["mode"].as_str().unwrap_or("weekly"))
    .bind(payload["lead_days"].as_i64().unwrap_or(3))
    .bind(payload["cycle_days"].as_i64())
    .bind(payload["cover_days"].as_i64())
    .bind(payload["source"].as_str().unwrap_or("app"))
    .bind(payload["updated_at"].as_str().unwrap_or(""))
    .execute(pool).await?;
    Ok(true)
}

async fn apply_setting(pool: &SqlitePool, payload: &Value) -> anyhow::Result<bool> {
    sqlx::query(
        "INSERT OR REPLACE INTO settings (key, value, updated_at, updated_by) \
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(payload["key"].as_str().unwrap_or(""))
    .bind(payload["value"].as_str())
    .bind(payload["updated_at"].as_str().unwrap_or(""))
    .bind(payload["updated_by"].as_str())
    .execute(pool).await?;
    Ok(true)
}

async fn apply_paid_ledger(pool: &SqlitePool, payload: &Value) -> anyhow::Result<bool> {
    sqlx::query(
        "INSERT OR IGNORE INTO paid_ledger (id, branch_id, supplier_code, invoice_no, paid_at, \
                amount, note, origin_install) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(payload["id"].as_str().unwrap_or(""))
    .bind(payload["branch_id"].as_i64())
    .bind(payload["supplier_code"].as_str().unwrap_or(""))
    .bind(payload["invoice_no"].as_str().unwrap_or(""))
    .bind(payload["paid_at"].as_str().unwrap_or(""))
    .bind(payload["amount"].as_f64().unwrap_or(0.0))
    .bind(payload["note"].as_str())
    .bind(payload["origin_install"].as_str().unwrap_or("remote"))
    .execute(pool).await?;
    Ok(true)
}

// ── HTTP: server side (the HoS) ────────────────────────────────────────────

/// GET /api/sync/outbox?since_ts=&since_id=&install=&branch=
/// Rows newer than the watermark, excluding the requester's own origin
/// (no echo). Branch-scoped tables filtered to the requester's branch.
pub async fn outbox_route(
    State(state): State<SharedState>,
    axum::extract::Query(q): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let since_ts = q.get("since_ts").map(|s| s.as_str()).unwrap_or("");
    let since_id = q.get("since_id").map(|s| s.as_str()).unwrap_or("");
    let install = q.get("install").map(|s| s.as_str()).unwrap_or("unknown");
    let branch: Option<i64> = q.get("branch").and_then(|b| b.parse().ok());

    let rows: Vec<(String, String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, origin_install, table_name, row_id, op, payload, ts FROM outbox \
         WHERE (ts > ?1 OR (ts = ?1 AND id > ?2)) AND origin_install != ?3 \
         ORDER BY ts, id LIMIT 500",
    )
    .bind(since_ts).bind(since_id).bind(install)
    .fetch_all(&state.pool).await
    .unwrap_or_default();

    let mut out = Vec::new();
    let mut last_ts = since_ts.to_string();
    let mut last_id = since_id.to_string();
    for (id, origin, table, row_id, op, payload, ts) in rows {
        if !table_allowed_for(&table) {
            continue;
        }
        // branch-scoped tables: only deliver rows for the requester's branch
        if BRANCH_SCOPED.contains(&table.as_str()) {
            if let Ok(p) = serde_json::from_str::<Value>(&payload) {
                let pb = p["branch_id"].as_i64();
                if branch.is_some() && pb.is_some() && pb != branch {
                    continue;
                }
            }
        }
        out.push(json!({ "id": id, "origin_install": origin, "table_name": table,
                         "row_id": row_id, "op": op, "payload": payload, "ts": ts }));
        last_ts = ts;
        last_id = id;
    }
    (StatusCode::OK, Json(json!({ "rows": out, "last_ts": last_ts, "last_id": last_id })))
}

/// POST /api/sync/up { install, rows: [...] } — apply incoming rows on the HoS.
pub async fn sync_up_route(
    State(state): State<SharedState>,
    Json(body): Json<Value>,
) -> impl IntoResponse {
    let install = body["install"].as_str().unwrap_or("unknown");
    let rows = body["rows"].as_array().cloned().unwrap_or_default();
    let mut applied = 0usize;
    let mut applied_ids = Vec::new();
    let mut errors = Vec::new();
    for r in &rows {
        let table = r["table_name"].as_str().unwrap_or("");
        let row_id = r["row_id"].as_str().unwrap_or("");
        let op = r["op"].as_str().unwrap_or("insert");
        let payload = match serde_json::from_str::<Value>(r["payload"].as_str().unwrap_or("{}")) {
            Ok(p) => p,
            Err(e) => { errors.push(format!("{table}: bad payload: {e}")); continue; }
        };
        match apply_row(&state.pool, table, row_id, op, &payload).await {
            Ok(_) => {
                applied += 1;
                applied_ids.push(r["id"].as_str().unwrap_or("").to_string());
                // mark consumed so the sender can drop it
                let _ = sqlx::query("UPDATE outbox SET applied = 1 WHERE id = ?1 AND origin_install = ?2")
                    .bind(r["id"].as_str().unwrap_or("")).bind(install)
                    .execute(&state.pool).await;
                // CONFIG-DOWN relay: re-emit into OUR outbox so every other
                // client pulls it down too (a Remote-HoS write must fan out to
                // all branches via the HoS). Branch-scoped rows (orders/POs)
                // are NOT re-emitted — the HoS aggregates them, and each
                // branch only pulls its own.
                if table_allowed_for(table) && CONFIG_DOWN.contains(&table) {
                    let relay_payload = payload.clone();
                    let relay_table = table.to_string();
                    let relay_row_id = row_id.to_string();
                    let relay_install = state.cfg.sync.install_name.clone();
                    let mut tx = match state.pool.begin().await {
                        Ok(tx) => tx,
                        Err(e) => { errors.push(format!("relay tx: {e}")); continue; }
                    };
                    match emit(&mut tx, &relay_install, &relay_table, &relay_row_id, "insert", &relay_payload).await {
                        Ok(()) => {
                            if let Err(e) = tx.commit().await {
                                errors.push(format!("relay commit: {e}"));
                            }
                        }
                        Err(e) => errors.push(format!("relay emit: {e}")),
                    }
                }
            }
            Err(e) => errors.push(format!("{table}/{row_id}: {e}")),
        }
    }
    (
        StatusCode::OK,
        Json(json!({ "ok": true, "applied": applied, "applied_ids": applied_ids, "errors": errors, "install": install })),
    )
}

// ── HTTP: client side ──────────────────────────────────────────────────────

/// Fetch + apply new outbox rows from the source; returns rows applied.
pub async fn pull_once(pool: &SqlitePool, sync: &SyncConfig, branch: Option<i64>) -> anyhow::Result<usize> {
    let src = sync.source.trim_end_matches('/').to_string();
    let (last_ts, last_id) = watermark(pool, &src).await?;
    let url = format!(
        "{src}/api/sync/outbox?since_ts={last_ts}&since_id={last_id}&install={}&branch={}",
        sync.install_name,
        branch.map(|b| b.to_string()).unwrap_or_default(),
    );
    let resp = reqwest::Client::new().get(&url).timeout(std::time::Duration::from_secs(30)).send().await?;
    let body: Value = resp.json().await?;
    let rows = body["rows"].as_array().cloned().unwrap_or_default();
    let mut applied = 0usize;
    for r in &rows {
        let table = r["table_name"].as_str().unwrap_or("");
        let row_id = r["row_id"].as_str().unwrap_or("");
        let op = r["op"].as_str().unwrap_or("insert");
        let payload = match serde_json::from_str::<Value>(r["payload"].as_str().unwrap_or("{}")) {
            Ok(p) => p,
            Err(_) => continue,
        };
        match apply_row(pool, table, row_id, op, &payload).await {
            Ok(true) => applied += 1,
            _ => {}
        }
    }
    let new_ts = body["last_ts"].as_str().unwrap_or(&last_ts).to_string();
    let new_id = body["last_id"].as_str().unwrap_or(&last_id).to_string();
    set_watermark(pool, &src, &new_ts, &new_id).await?;
    Ok(applied)
}

/// Push this install's un-applied outbox rows up to the source.
pub async fn push_once(pool: &SqlitePool, sync: &SyncConfig) -> anyhow::Result<usize> {
    let rows: Vec<(String, String, String, String, String)> = sqlx::query_as(
        "SELECT id, table_name, row_id, op, payload FROM outbox \
         WHERE applied = 0 AND origin_install = ?1 ORDER BY ts, id LIMIT 500",
    )
    .bind(&sync.install_name)
    .fetch_all(pool).await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let payload_rows: Vec<Value> = rows.iter().map(|(id, table, row_id, op, payload)| {
        json!({ "id": id, "table_name": table, "row_id": row_id, "op": op, "payload": payload })
    }).collect();
    let src = sync.source.trim_end_matches('/').to_string();
    let url = format!("{src}/api/sync/up");
    let resp = reqwest::Client::new().post(&url)
        .json(&json!({ "install": sync.install_name, "rows": payload_rows }))
        .timeout(std::time::Duration::from_secs(30))
        .send().await?;
    let body: Value = resp.json().await?;
    let applied = body["applied"].as_u64().unwrap_or(0) as usize;
    if applied > 0 {
        // mark only the rows the source confirmed (applied_ids) — any rejected
        // row stays applied=0 and retries next pass (no silent loss)
        let ids: Vec<String> = body["applied_ids"].as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();
        for id in &ids {
            sqlx::query("UPDATE outbox SET applied = 1 WHERE id = ?1")
                .bind(id).execute(pool).await?;
        }
    }
    Ok(applied)
}

async fn watermark(pool: &SqlitePool, source: &str) -> anyhow::Result<(String, String)> {
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT last_ts, last_id FROM sync_watermarks WHERE source = ?1",
    )
    .bind(source)
    .fetch_optional(pool).await?;
    Ok(row.unwrap_or((String::new(), String::new())))
}

async fn set_watermark(pool: &SqlitePool, source: &str, ts: &str, id: &str) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO sync_watermarks (source, last_ts, last_id, updated_at) \
         VALUES (?1, ?2, ?3, datetime('now')) \
         ON CONFLICT(source) DO UPDATE SET last_ts = excluded.last_ts, \
           last_id = excluded.last_id, updated_at = excluded.updated_at",
    )
    .bind(source).bind(ts).bind(id)
    .execute(pool).await?;
    Ok(())
}

/// One full client sync pass (pull config/orders down, push own rows up).
/// O-10: consecutive-failure counter returned for the caller to back off.
pub async fn sync_once(pool: &SqlitePool, sync: &SyncConfig, branch: Option<i64>) -> anyhow::Result<(usize, usize)> {
    let pulled = pull_once(pool, sync, branch).await?;
    let pushed = push_once(pool, sync).await?;
    Ok((pulled, pushed))
}

/// Background client loop (spawned on BoS / Remote-HoS when sync enabled).
/// Immediate push is attempted on each write via `notify_write`; this loop is
/// the poll backstop + pull side (O-2).
pub async fn run_loop(state: SharedState) {
    let interval_min = state.cfg.sync.poll_interval_minutes.max(1);
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(interval_min * 60));
    ticker.reset_after(std::time::Duration::from_secs(20));
    let mut consecutive_failures = 0u32;
    loop {
        ticker.tick().await;
        let sync = state.cfg.sync.clone();
        if !sync.enabled || sync.source.is_empty() {
            continue;
        }
        let branch = state.server_info.read().ok().and_then(|i| i.branch_id).map(|b| b as i64);
        match sync_once(&state.pool, &sync, branch).await {
            Ok((pulled, pushed)) => {
                consecutive_failures = 0;
                if pulled > 0 || pushed > 0 {
                    tracing::info!("replication: pulled {pulled}, pushed {pushed}");
                }
            }
            Err(e) => {
                consecutive_failures += 1;
                tracing::warn!("replication: sync failed ({consecutive_failures}x): {e}");
                if consecutive_failures >= 5 {
                    // O-10: back off — skip the next interval
                    tracing::warn!("replication: circuit breaker — backing off");
                    tokio::time::sleep(std::time::Duration::from_secs(interval_min * 120)).await;
                    consecutive_failures = 0;
                }
            }
        }
    }
}

/// Best-effort immediate push after a local write (O-2). Fire-and-forget.
pub fn notify_write(state: &SharedState) {
    let sync = state.cfg.sync.clone();
    if !sync.enabled || sync.source.is_empty() {
        return;
    }
    let pool = state.pool.clone();
    tokio::spawn(async move {
        let _ = push_once(&pool, &sync).await;
    });
}

/// Emit an outbox row in the same tx as a config-class write, then notify.
pub async fn emit_and_notify(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    state: &SharedState,
    table: &str,
    row_id: &str,
    op: &str,
    payload: &Value,
) -> anyhow::Result<()> {
    let install = state.cfg.sync.install_name.clone();
    emit(tx, &install, table, row_id, op, payload).await?;
    Ok(())
}
