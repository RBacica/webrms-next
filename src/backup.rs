// P4 built-in backups (B2/O-9): `webrms-next backup` — VACUUM INTO a compact
// consistent copy of data.db, keep-N retention, and an automatic pre-seed
// backup (a seed/import must never run without a rollback point).
//
// VACUUM INTO produces a single-file, WAL-independent snapshot even while the
// server is running (sqlite takes a consistent read snapshot).

use std::path::{Path, PathBuf};

pub const BACKUP_DIR: &str = "backup";
pub const DEFAULT_KEEP: usize = 5;

/// Create a timestamped backup of `<data_dir>/data.db` and prune to `keep`
/// newest backups. Returns the backup path.
pub async fn create_backup(data_dir: &Path, keep: usize) -> anyhow::Result<PathBuf> {
    let pool = crate::db::init_pool(data_dir).await?;
    let backup_dir = data_dir.join(BACKUP_DIR);
    std::fs::create_dir_all(&backup_dir)?;

    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let path = backup_dir.join(format!("webrms-next-backup-{ts}.db"));
    // VACUUM INTO refuses to overwrite an existing file — unlikely with a
    // second-resolution timestamp, but make it deterministic anyway.
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    let dest = path.display().to_string().replace('\'', "''");
    let res = sqlx::raw_sql(sqlx::AssertSqlSafe(format!("VACUUM INTO '{dest}'")))
        .execute(&pool)
        .await;
    pool.close().await;
    res?;

    prune(&backup_dir, keep);
    Ok(path)
}

/// Remove the oldest backups beyond `keep` (newest kept, by mtime).
pub fn prune(backup_dir: &Path, keep: usize) {
    let mut backups: Vec<PathBuf> = match std::fs::read_dir(backup_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|x| x == "db")
                    && p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("webrms-next-backup-"))
            })
            .collect(),
        Err(_) => return,
    };
    if backups.len() <= keep {
        return;
    }
    backups.sort_by_key(|p| std::fs::metadata(p).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH));
    // newest LAST after sort; remove from the front while over the cap
    let remove = backups.len() - keep;
    for p in backups.into_iter().take(remove) {
        let _ = std::fs::remove_file(p);
    }
}

/// List existing backups newest-first (for `doctor` + operators).
pub fn list_backups(data_dir: &Path) -> Vec<PathBuf> {
    let dir = data_dir.join(BACKUP_DIR);
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort_by_key(|p| {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH)
    });
    v.reverse();
    v
}

/// Size of a backup in bytes (0 when missing).
pub fn size_bytes(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}
