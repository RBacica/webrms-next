// P3 snapshot fallback (O-5): a client whose live connector is unreachable
// restores the HoS's materialized local DB as a read fallback.
//
// Server side: GET /api/sync/snapshot → builds a fresh gzip-compressed copy
// of the local SQLite DB (via VACUUM INTO for a consistent snapshot) with an
// HMAC-SHA256 signature header (key from config; empty key = unsigned dev
// mode). Written to a staging file first, then renamed into place (atomic).
//
// Client side: restore_snapshot() downloads → verifies HMAC → replaces the
// local data.db. Caller (connector fallback path) decides when to use it.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

pub const SNAPSHOT_FILE: &str = "snapshot.sqlite.gz";
pub const SIG_HEADER: &str = "x-snapshot-sig";

/// Server: write a gzip snapshot of `db_path` into `data_dir` (staged then
/// renamed). Returns (final_path, hmac_hex). HMAC empty when key is empty.
pub async fn build_snapshot(pool: &sqlx::SqlitePool, data_dir: &Path, key: &str) -> anyhow::Result<(PathBuf, String)> {
    let snapshot_sqlite = data_dir.join(format!("snapshot-{}.sqlite", Uuid::new_v4()));
    let staged = data_dir.join(format!("snapshot-{}.sqlite.gz.staging", Uuid::new_v4()));
    let final_path = data_dir.join(SNAPSHOT_FILE);

    // VACUUM INTO gives a consistent, compact copy even with WAL.
    // `dest` is a UUID-based filename we generate under our own data_dir —
    // never user input, so we explicitly assert SQL-safety.
    let dest = snapshot_sqlite.display().to_string().replace('\'', "''");
    sqlx::raw_sql(sqlx::AssertSqlSafe(format!("VACUUM INTO '{dest}'")))
        .execute(pool)
        .await?;

    // gzip it
    let raw = std::fs::read(&snapshot_sqlite)?;
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&raw)?;
    let gz = enc.finish()?;
    std::fs::remove_file(&snapshot_sqlite).ok();

    let sig = if key.is_empty() {
        String::new()
    } else {
        let mut mac = HmacSha256::new_from_slice(key.as_bytes())?;
        mac.update(&gz);
        hex::encode(mac.finalize().into_bytes())
    };

    // atomic: write staging, fsync, rename
    let mut f = std::fs::File::create(&staged)?;
    f.write_all(&gz)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&staged, &final_path)?;

    Ok((final_path, sig))
}

/// Client: fetch snapshot from `source`, verify HMAC (when key non-empty),
/// decompress, and atomically replace the local DB at `db_path`.
pub async fn restore_snapshot(
    source: &str,
    key: &str,
    db_path: &Path,
) -> anyhow::Result<u64> {
    let src = source.trim_end_matches('/').to_string();
    let url = format!("{src}/api/sync/snapshot");
    let resp = reqwest::Client::new()
        .get(&url)
        .timeout(std::time::Duration::from_secs(120))
        .send()
        .await?;
    if !resp.status().is_success() {
        anyhow::bail!("snapshot fetch failed: {}", resp.status());
    }
    let sig = resp
        .headers()
        .get(SIG_HEADER)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let gz = resp.bytes().await?;

    if !key.is_empty() {
        let mut mac = HmacSha256::new_from_slice(key.as_bytes())?;
        mac.update(&gz);
        let expect = hex::encode(mac.finalize().into_bytes());
        if !expect.eq_ignore_ascii_case(&sig) {
            anyhow::bail!("snapshot HMAC mismatch — refusing to restore");
        }
    }

    // decompress
    let mut dec = flate2::read::GzDecoder::new(&gz[..]);
    let mut raw = Vec::new();
    std::io::Read::read_to_end(&mut dec, &mut raw)?;

    // atomic replace: write tmp next to target, rename over it
    let tmp = db_path.with_extension(format!("db.tmp-{}", SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()));
    std::fs::write(&tmp, &raw)?;
    std::fs::rename(&tmp, db_path)?;
    Ok(gz.len() as u64)
}
