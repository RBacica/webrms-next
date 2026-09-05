use arc_swap::ArcSwap;
use sqlx::sqlite::SqlitePool;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use crate::config::AppConfig;

/// Server mode — mirrors the role config, extended by live detection later (P1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ServerMode {
    Hos,
    Bos,
    RemoteHos,
    Standalone,
}

impl ServerMode {
    pub fn from_config(cfg: &AppConfig) -> Self {
        match cfg.role.mode.as_str() {
            "hos" => ServerMode::Hos,
            "bos" => ServerMode::Bos,
            "remote-hos" => ServerMode::RemoteHos,
            _ => ServerMode::Standalone,
        }
    }
    pub fn as_str(&self) -> &'static str {
        match self {
            ServerMode::Hos => "hos",
            ServerMode::Bos => "bos",
            ServerMode::RemoteHos => "remote-hos",
            ServerMode::Standalone => "standalone",
        }
    }
}

/// Detectable server info (today's detection.rs equivalent; P1 extends it with
/// connector probe results).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ServerInfo {
    pub mode: ServerMode,
    pub branch_id: Option<i32>,
    pub db_ok: bool,
    pub version: String,
}

/// Shared application state.
pub struct AppState {
    /// Live DB pool handle. ArcSwap so the snapshot fallback (fallback.rs)
    /// can hot-swap to a restored data.db: every caller re-loads the current
    /// pool via `pool_arc()`, so no code holds a stale handle across a swap.
    pub pool: ArcSwap<SqlitePool>,
    pub cfg: AppConfig,
    pub data_dir: PathBuf,
    pub server_info: RwLock<ServerInfo>,
    /// Connector poller (None when sync/connector disabled — standalone mode).
    pub poller: RwLock<Option<crate::poller::PollerHandle>>,
}

impl AppState {
    /// Load the current live pool (atomic; safe to call after a fallback swap).
    pub fn pool_arc(&self) -> Arc<SqlitePool> {
        Arc::clone(&self.pool.load_full())
    }
}

/// Poll status reported by the connector loop (for /api/health + UI badges).
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct PollStatus {
    pub last_success: Option<String>,
    pub last_error: Option<String>,
    pub tick_count: u64,
    pub connector_enabled: bool,
    pub last_items: u64,
    pub last_sales: u64,
}

impl AppState {
    pub fn new(pool: SqlitePool, cfg: AppConfig, data_dir: PathBuf) -> Arc<Self> {
        let mode = ServerMode::from_config(&cfg);
        let db_ok = true; // pool created + migrated already
        let info = ServerInfo {
            mode,
            branch_id: None,
            db_ok,
            version: env!("CARGO_PKG_VERSION").to_string(),
        };
        Arc::new(Self {
            pool: ArcSwap::new(Arc::new(pool)),
            cfg,
            data_dir,
            server_info: RwLock::new(info),
            poller: RwLock::new(None),
        })
    }

    /// True when this install is a config AUTHOR (HoS or Remote-HoS).
    pub fn config_author(&self) -> bool {
        self.server_info
            .read()
            .map(|i| matches!(i.mode, ServerMode::Hos | ServerMode::RemoteHos))
            .unwrap_or(false)
    }

    /// True for a Remote-HoS workstation: an author that is ALSO a sync client.
    pub fn is_remote_hos(&self) -> bool {
        self.server_info
            .read()
            .map(|i| i.mode == ServerMode::RemoteHos)
            .unwrap_or(false)
    }
}

pub type SharedState = Arc<AppState>;
