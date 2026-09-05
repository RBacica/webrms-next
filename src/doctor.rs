// P4 `webrms-next doctor` (B3) — the first thing an operator runs when
// something "feels off": DB integrity, schema state, connector reachability,
// replication/fallback state, outbox backlog, backups, disk space.
// Prints a per-check PASS/FAIL/WARN table; process exit code 1 when any
// check FAILs (warnings don't fail).

use std::path::Path;

use anyhow::Context;
use sqlx::sqlite::SqlitePool;

use crate::config::AppConfig;
use crate::connector::hos::HosConnector;
use crate::connector::Connector;

pub struct CheckResult {
    pub name: &'static str,
    pub level: Level,
    pub detail: String,
}

#[derive(PartialEq, Clone, Copy)]
pub enum Level {
    Pass,
    Warn,
    Fail,
}

impl Level {
    fn tag(&self) -> &'static str {
        match self {
            Level::Pass => "PASS",
            Level::Warn => "WARN",
            Level::Fail => "FAIL",
        }
    }
}

async fn db_checks(pool: &SqlitePool, data_dir: &Path) -> Vec<CheckResult> {
    let mut out = Vec::new();

    // integrity_check
    match sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
        .fetch_one(pool)
        .await
    {
        Ok(s) if s == "ok" => out.push(CheckResult {
            name: "db integrity",
            level: Level::Pass,
            detail: "PRAGMA integrity_check: ok".into(),
        }),
        Ok(s) => out.push(CheckResult {
            name: "db integrity",
            level: Level::Fail,
            detail: format!("integrity_check: {s}"),
        }),
        Err(e) => out.push(CheckResult {
            name: "db integrity",
            level: Level::Fail,
            detail: format!("integrity_check failed: {e}"),
        }),
    }

    // migrations fully applied
    let applied: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _sqlx_migrations WHERE success = 1")
        .fetch_one(pool)
        .await
        .unwrap_or(-1);
    let expected = crate::db::MIGRATOR.iter().count() as i64;
    if applied == expected {
        out.push(CheckResult {
            name: "schema",
            level: Level::Pass,
            detail: format!("{applied}/{expected} migrations applied"),
        });
    } else {
        out.push(CheckResult {
            name: "schema",
            level: Level::Fail,
            detail: format!("{applied}/{expected} migrations applied"),
        });
    }

    // data.db size + WAL sidecar (a runaway WAL signals a write stuck open)
    let db_path = data_dir.join("data.db");
    let size = std::fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0);
    let wal = data_dir.join("data.db-wal");
    let wal_size = std::fs::metadata(&wal).map(|m| m.len()).unwrap_or(0);
    let mb = |b: u64| format!("{:.1} MB", b as f64 / 1_048_576.0);
    out.push(CheckResult {
        name: "db file",
        level: if size > 0 { Level::Pass } else { Level::Fail },
        detail: format!("data.db {} (WAL {})", mb(size), mb(wal_size)),
    });

    // backups present?
    let backups = crate::backup::list_backups(data_dir);
    out.push(CheckResult {
        name: "backups",
        level: if backups.is_empty() { Level::Warn } else { Level::Pass },
        detail: if backups.is_empty() {
            "none — run `webrms-next backup` (or seed creates one)".into()
        } else {
            format!(
                "{} found; newest {} ({})",
                backups.len(),
                backups[0].file_name().unwrap_or_default().to_string_lossy(),
                mb(crate::backup::size_bytes(&backups[0]))
            )
        },
    });

    out
}

async fn connector_check(cfg: &AppConfig) -> CheckResult {
    if cfg.database.connection_string.is_empty() {
        return CheckResult {
            name: "connector",
            level: Level::Pass,
            detail: "not configured (standalone mode)".into(),
        };
    }
    let conn = HosConnector::new(cfg.database.connection_string.clone());
    match tokio::time::timeout(
        std::time::Duration::from_secs(15),
        conn.probe(),
    )
    .await
    {
        Ok(Ok(p)) => CheckResult {
            name: "connector",
            level: Level::Pass,
            detail: format!("reachable — engine {}, {} branches", p.engine, p.branch_ids.len()),
        },
        Ok(Err(e)) => CheckResult {
            name: "connector",
            level: Level::Fail,
            detail: format!("unreachable/error: {e}"),
        },
        Err(_) => CheckResult {
            name: "connector",
            level: Level::Fail,
            detail: "probe timed out (15s)".into(),
        },
    }
}

async fn replication_check(pool: &SqlitePool, cfg: &AppConfig, data_dir: &Path) -> Vec<CheckResult> {
    let mut out = Vec::new();
    if !cfg.sync.enabled || cfg.sync.source.is_empty() {
        out.push(CheckResult {
            name: "replication",
            level: Level::Pass,
            detail: "disabled (this install is the source)".into(),
        });
        return out;
    }

    // fallback engagement (client with a dead connector)
    let fb = crate::fallback::read_state(data_dir);
    if fb.engaged {
        out.push(CheckResult {
            name: "fallback",
            level: Level::Warn,
            detail: format!(
                "ACTIVE — serving HoS snapshot since {} ({} bytes)",
                fb.restored_at.as_deref().unwrap_or("?"),
                fb.size_bytes
            ),
        });
    } else {
        out.push(CheckResult {
            name: "fallback",
            level: Level::Pass,
            detail: if fb.recovered_at.is_some() {
                format!("recovered {}", fb.recovered_at.as_deref().unwrap_or(""))
            } else {
                "not engaged".into()
            },
        });
    }

    // replication lag from sync_watermarks
    match sqlx::query_scalar::<_, String>(
        "SELECT MAX(updated_at) FROM sync_watermarks WHERE source = ?1",
    )
    .bind(&cfg.sync.source)
    .fetch_optional(pool)
    .await
    {
        Ok(Some(ts)) => {
            let lag = lag_minutes(&ts);
            out.push(CheckResult {
                name: "replication lag",
                level: if lag <= 60 { Level::Pass } else { Level::Warn },
                detail: format!("last successful pull {ts} (~{lag} min ago)"),
            });
        }
        Ok(None) => out.push(CheckResult {
            name: "replication lag",
            level: Level::Warn,
            detail: "no sync_watermarks yet — never pulled?".into(),
        }),
        Err(e) => out.push(CheckResult {
            name: "replication lag",
            level: Level::Fail,
            detail: format!("query failed: {e}"),
        }),
    }

    // outbox backlog (rows never acknowledged by the source)
    let backlog: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM outbox WHERE applied = 0")
        .fetch_one(pool)
        .await
        .unwrap_or(-1);
    out.push(CheckResult {
        name: "outbox backlog",
        level: if backlog == 0 { Level::Pass } else { Level::Warn },
        detail: if backlog == 0 {
            "no pending rows".into()
        } else {
            format!("{backlog} rows unacknowledged by the source")
        },
    });

    out
}

fn lag_minutes(ts: &str) -> i64 {
    match chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S") {
        Ok(t) => chrono::Local::now().naive_local().signed_duration_since(t).num_minutes(),
        Err(_) => i64::MAX,
    }
}

/// Disk free for the data dir (unix statvfs; windows reports 0 = unchecked).
#[allow(unused_variables)]
fn disk_free_mb(data_dir: &Path) -> Option<u64> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let path = std::ffi::CString::new(data_dir.as_os_str().as_bytes()).ok()?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        if unsafe { libc::statvfs(path.as_ptr(), &mut st) } == 0 {
            let free = st.f_bavail as u128 * st.f_frsize as u128;
            return Some((free / 1_048_576) as u64);
        }
        None
    }
    #[cfg(not(unix))]
    {
        None
    }
}

/// Run the full doctor report. Returns true when no check FAILs.
pub async fn run(cfg: &AppConfig, data_dir: &Path) -> anyhow::Result<bool> {
    std::fs::create_dir_all(data_dir)?;
    let pool = crate::db::init_pool(data_dir)
        .await
        .context("cannot open data.db — is the data dir valid?")?;

    let mut checks = db_checks(&pool, data_dir).await;
    checks.push(connector_check(cfg).await);
    checks.extend(replication_check(&pool, cfg, data_dir).await);

    // disk
    match disk_free_mb(data_dir) {
        Some(free) => checks.push(CheckResult {
            name: "disk free",
            level: if free < 1024 { Level::Warn } else { Level::Pass },
            detail: format!("{free} MB free on the data volume"),
        }),
        None => checks.push(CheckResult {
            name: "disk free",
            level: Level::Warn,
            detail: "not checked on this platform".into(),
        }),
    }

    pool.close().await;

    let mut failed = false;
    println!("\n  WebRMS-Next doctor — {}", data_dir.display());
    println!("  ─────────────────────────────────────────────");
    for c in &checks {
        if c.level == Level::Fail {
            failed = true;
        }
        println!("  {:>4}  {:<18} {}", c.level.tag(), c.name, c.detail);
    }
    println!("  ─────────────────────────────────────────────");
    println!(
        "  {}",
        if failed {
            "FAILURES PRESENT — fix the FAIL lines before relying on this install."
        } else {
            "All checks pass. (WARN lines are advisories.)"
        }
    );
    Ok(!failed)
}
