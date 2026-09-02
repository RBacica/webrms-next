use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::migrate::Migrator;
use std::path::Path;
use std::str::FromStr;

pub static MIGRATOR: Migrator = sqlx::migrate!("src/migrations");

/// Create/connect the SQLite pool at `data_dir/data.db`, run migrations,
/// and apply the production pragmas (A1): WAL, synchronous=NORMAL, busy_timeout, FK ON.
pub async fn init_pool(data_dir: &Path) -> Result<SqlitePool, sqlx::Error> {
    std::fs::create_dir_all(data_dir)?;
    let db_path = data_dir.join("data.db");
    let opts = SqliteConnectOptions::from_str(&format!("sqlite://{}", db_path.display()))?
        .create_if_missing(true)
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(5))
        .foreign_keys(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await?;

    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

/// Quick health probe: SELECT 1 against the pool.
pub async fn ping(pool: &SqlitePool) -> bool {
    sqlx::query("SELECT 1").execute(pool).await.is_ok()
}
