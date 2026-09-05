// P3 automatic snapshot fallback (5b / O-5) — the wiring that snapshot.rs's
// comment left to "the caller (connector fallback path)".
//
// When a client install's live connector stays dead (reference pull failing
// for FAIL_THRESHOLD consecutive ticks), the poller calls `engage()`:
//
//   1. Download + HMAC-verify the HoS snapshot while the app keeps serving
//      (network op; local DB untouched).
//   2. Close the shared pool — sqlx `close()` waits for in-flight connections,
//      so nothing can write after this point and be lost.
//   3. VACUUM INTO a consistent backup of the CURRENT local DB. This is the
//      critical step: the backup holds this install's app-authored rows
//      (orders, outbox, incoming POs, paid marks, config overrides) and its
//      own connector/replication watermarks — all of which the HoS snapshot
//      does NOT contain (or contains in HoS-authored form).
//   4. Atomically replace data.db with the snapshot (gzip → rename).
//   5. Reopen the pool (migrations re-run; the snapshot is the same schema).
//   6. Re-import the preserved tables from the backup — local app rows win
//      outright; pulled-down config rows (settings/supplier_modes/
//      supplier_terms) merge newest-`updated_at`-wins against the snapshot's.
//   7. Hot-swap the pool handle (AppState.pool is an ArcSwap).
//
// Reads never touched the live AKPOS anyway (they hit the local DB), so after
// the swap the install serves the HoS's materialized catalog — the fallback
// path per DESIGN 5b. When the connector comes back, the next fully-successful
// poll tick clears `engaged` (`clear_if_engaged`) and incremental ingest
// resumes from the preserved high-water marks.
//
// Data-loss analysis (why step 2 precedes step 3): a write between the backup
// and the file swap would land in the old file and be discarded. Closing the
// pool first means any write that completed before close IS in the backup, and
// anything after close fails cleanly (pool closed) instead of being lost.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePool;

use crate::state::SharedState;

pub const STATE_FILE: &str = "fallback.json";
/// Consecutive failed connector ticks before the fallback engages.
pub const FAIL_THRESHOLD: u64 = 3;

/// App-authored / local-state tables that MUST survive a restore: their rows
/// exist only on this install (or replicate from here), so the HoS snapshot's
/// copy would lose or regress them. Re-imported wholesale — local wins.
/// Order matters for FK parents before children (FKs are off during import,
/// so this is belt-and-braces).
const PRESERVE_LOCAL: &[&str] = &[
    "outbox",
    "orders",
    "order_lines",
    "incoming_pos",
    "paid_ledger",
    "rebate_contracts",
    "rebate_contract_lines",
    "rebate_ledger",
    "stocktake_schedules",
    "stocktake_runs",
    "item_etl_exports",
    "item_change_requests",
    "app_overrides",
    "audit_log",
    "high_watermarks",   // connector pull positions — never inherit HoS's
    "sync_watermarks",   // replication pull positions
    "branch_mapping",    // per-install old→new branch mapping config
];

/// HoS-authored config pulled DOWN: newest `updated_at` wins between the
/// local copy and the snapshot's. (pk column per table.)
const PRESERVE_LWW: &[(&str, &str)] = &[
    ("settings", "key"),
    ("supplier_modes", "supplier_code"),
    ("supplier_terms", "supplier_code"),
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FallbackMeta {
    pub engaged: bool,
    pub restored_at: Option<String>,
    pub recovered_at: Option<String>,
    pub via: String,
    pub size_bytes: u64,
    pub last_attempt: Option<String>,
    pub last_error: Option<String>,
    pub attempts: u64,
}

pub fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join(STATE_FILE)
}

pub fn read_state(data_dir: &Path) -> FallbackMeta {
    match std::fs::read_to_string(state_path(data_dir)) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => FallbackMeta::default(),
    }
}

fn write_state(data_dir: &Path, m: &FallbackMeta) {
    if let Ok(s) = serde_json::to_string_pretty(m) {
        let _ = std::fs::write(state_path(data_dir), s);
    }
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Poller hook after a fully-successful tick: if a fallback was engaged,
/// mark it recovered (the connector is back; data flows again).
pub fn clear_if_engaged(data_dir: &Path) {
    let mut meta = read_state(data_dir);
    if meta.engaged {
        meta.engaged = false;
        meta.recovered_at = Some(now());
        write_state(data_dir, &meta);
        tracing::info!("snapshot fallback: connector recovered — cleared engagement");
    }
}

/// Poller hook after a connector-dead tick: attempt the snapshot fallback,
/// subject to cooldown and role/sync config. Returns true when engaged.
pub async fn maybe_engage(state: SharedState) -> bool {
    let data_dir = state.data_dir.clone();
    let cfg = &state.cfg;
    tracing::debug!(
        "fallback: consider mode={} sync={} source_len={} fb_enabled={}",
        cfg.role.mode, cfg.sync.enabled, cfg.sync.source.len(), cfg.sync.fallback_enabled
    );

    // Role gate: only clients fall back to a HoS snapshot. The HoS *is* the
    // snapshot source; standalone has no source. Sync must be enabled.
    if cfg.role.mode == "hos" || cfg.role.mode == "standalone" {
        return false;
    }
    if !cfg.sync.enabled || cfg.sync.source.is_empty() || !cfg.sync.fallback_enabled {
        return false;
    }

    let meta = read_state(&data_dir);
    if meta.engaged {
        return false; // already serving the snapshot; wait for recovery
    }
    // Cooldown between attempts (a dead box must not hammer the HoS).
    if let Some(at) = &meta.last_attempt {
        if let Ok(elapsed) = chrono::NaiveDateTime::parse_from_str(at, "%Y-%m-%d %H:%M:%S") {
            let cooldown = chrono::Duration::minutes(cfg.sync.fallback_cooldown_minutes as i64);
            let since = chrono::Local::now().naive_local().signed_duration_since(elapsed);
            if since < cooldown {
                return false;
            }
        }
    }

    match engage(state).await {
        Ok(()) => {
            tracing::info!("snapshot fallback: engaged (restored HoS snapshot)");
            true
        }
        Err(e) => {
            tracing::warn!("snapshot fallback: engage failed: {e}");
            false
        }
    }
}

/// Perform the fallback restore (see module docs for the step order).
async fn engage(state: SharedState) -> anyhow::Result<()> {
    let data_dir = state.data_dir.clone();
    let sync = state.cfg.sync.clone();
    let db_path = data_dir.join("data.db");

    let mut meta = read_state(&data_dir);
    meta.last_attempt = Some(now());
    meta.attempts += 1;

    // 1) download + verify while the app keeps running
    let gz = crate::snapshot::download_snapshot(&sync.source, &sync.snapshot_key)
        .await
        .map_err(|e| {
            meta.last_error = Some(format!("download: {e}"));
            write_state(&data_dir, &meta);
            e
        })?;

    // 2) quiesce: close the shared pool (waits for in-flight work to finish).
    //    New requests during the swap fail with a closed-pool error — a few
    //    hundred ms, once per fallback event; logged loudly below.
    //    (The arc-swap Guard is !Send, so scope it out before any await.)
    let old_pool: SqlitePool = {
        let g = state.pool.load_full();
        g.as_ref().clone()
    };
    if let Err(e) = tokio::time::timeout(Duration::from_secs(60), old_pool.close()).await {
        tracing::warn!("snapshot fallback: pool close timed out ({e}) — proceeding");
    }

    // 3) consistent backup of the current DB (app rows + watermarks)
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let backup_dir = data_dir.join("backup");
    std::fs::create_dir_all(&backup_dir)?;
    let backup = backup_dir.join(format!("pre-fallback-{ts}.db"));
    {
        let bk_pool = crate::db::init_pool(&data_dir).await?; // reopens the OLD file
        let dest = backup.display().to_string().replace('\'', "''");
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("VACUUM INTO '{dest}'")))
            .execute(&bk_pool)
            .await?;
        bk_pool.close().await;
    }

    // 4) replace data.db with the snapshot (drop stale WAL sidecars first —
    //    the old file is fully closed/checkpointed by now)
    let _ = std::fs::remove_file(data_dir.join("data.db-wal"));
    let _ = std::fs::remove_file(data_dir.join("data.db-shm"));
    crate::snapshot::install_snapshot(&gz, &db_path).await?;

    // 5) reopen (migrations re-run — the snapshot carries the same schema)
    let new_pool = crate::db::init_pool(&data_dir).await?;

    // 6) re-import this install's rows from the backup
    reimport_app_rows(&new_pool, &backup).await?;

    // 7) hot-swap the pool handle
    state.pool.store(Arc::new(new_pool));

    let pool = state.pool_arc();
    let _ = sqlx::query(
        "INSERT INTO sync_log (direction, status, detail, rows_processed) \
         VALUES ('snapshot', 'ok', ?1, ?2)",
    )
    .bind(format!("fallback engaged; pre-restore backup {}", backup.display()))
    .bind(gz.len() as i64)
    .execute(&*pool)
    .await;

    meta.engaged = true;
    meta.restored_at = Some(now());
    meta.recovered_at = None;
    meta.via = sync.source.clone();
    meta.size_bytes = gz.len() as u64;
    meta.last_error = None;
    write_state(&data_dir, &meta);

    tracing::warn!(
        "snapshot fallback: data.db replaced with HoS snapshot ({} bytes) — pre-restore backup at {}",
        gz.len(),
        backup.display()
    );
    Ok(())
}

/// Copy preserved tables from the backup file into the (restored) main DB.
/// Local app rows win outright; pulled-down config rows merge LWW on
/// `updated_at`. One connection, FKs off, single transaction.
///
/// DETACH gotcha (hit live 2026-09-05): sqlite refuses `DETACH` with
/// SQLITE_LOCKED when the connection still has prepared statements cached
/// against the attached db (sqlx caches them per connection). So: COMMIT
/// first, then DETACH best-effort — if it still refuses, detach() the raw
/// connection (closing it outright) so no pooled connection keeps `bak`
/// attached for future queries.
async fn reimport_app_rows(pool: &SqlitePool, backup: &Path) -> anyhow::Result<()> {
    let mut conn = pool.acquire().await?;
    sqlx::query("PRAGMA foreign_keys = OFF").execute(&mut *conn).await?;
    sqlx::query("BEGIN").execute(&mut *conn).await?;

    let result = async {
        // ATTACH the backup as `bak`
        let bak = backup.display().to_string().replace('\'', "''");
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!("ATTACH DATABASE '{bak}' AS bak")))
            .execute(&mut *conn)
            .await?;

        for t in PRESERVE_LOCAL {
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "INSERT OR REPLACE INTO main.{t} SELECT * FROM bak.{t}"
            )))
            .execute(&mut *conn)
            .await?;
            tracing::debug!("fallback: re-imported {t} (local wins)");
        }
        for (t, pk) in PRESERVE_LWW {
            sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
                "INSERT OR REPLACE INTO main.{t} SELECT * FROM bak.{t} b \
                 WHERE b.updated_at >= COALESCE( \
                   (SELECT m.updated_at FROM main.{t} m WHERE m.{pk} = b.{pk}), b.updated_at)"
            )))
            .execute(&mut *conn)
            .await?;
            tracing::debug!("fallback: merged {t} (LWW on updated_at)");
        }

        anyhow::Ok(())
    }
    .await;

    match result {
        Ok(()) => {
            sqlx::query("COMMIT").execute(&mut *conn).await?;
            // Best-effort DETACH; on refusal close the raw connection so the
            // pooled handle never carries `bak` around.
            if sqlx::query("DETACH DATABASE bak").execute(&mut *conn).await.is_err() {
                let raw = conn.detach();
                drop(raw);
                return Ok(());
            }
            sqlx::query("PRAGMA foreign_keys = ON").execute(&mut *conn).await?;
            Ok(())
        }
        Err(e) => {
            sqlx::query("ROLLBACK").execute(&mut *conn).await?;
            Err(e)
        }
    }
}
