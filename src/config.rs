use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    #[serde(default)]
    pub paths: PathsConfig,
    #[serde(default)]
    pub database: DatabaseConfig,
    #[serde(default)]
    pub sync: SyncConfig,
    #[serde(default)]
    pub role: RoleConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PathsConfig {
    #[serde(default = "default_data_dir")]
    pub data_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DatabaseConfig {
    /// Connector connection string (tiberius, key=value). Empty = standalone.
    #[serde(default)]
    pub connection_string: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub source: String,
    #[serde(default = "default_poll")]
    pub poll_interval_minutes: u64,
    /// This install's identity (outbox origin_install + sync auth label).
    #[serde(default = "default_install")]
    pub install_name: String,
    /// HMAC key for snapshot signing (O-5). Empty = unsigned dev mode.
    #[serde(default)]
    pub snapshot_key: String,
    /// Automatic snapshot fallback (5b): when the connector stays dead, a
    /// client restores the HoS snapshot so reads keep serving. Default on.
    #[serde(default = "default_fallback_enabled")]
    pub fallback_enabled: bool,
    /// Min minutes between fallback attempts while the connector stays dead.
    #[serde(default = "default_fallback_cooldown")]
    pub fallback_cooldown_minutes: u64,
}

fn default_install() -> String { "local".into() }
fn default_fallback_enabled() -> bool { true }
fn default_fallback_cooldown() -> u64 { 15 }

impl Default for SyncConfig {
    /// Programmatic default matches the serde defaults used when parsing
    /// config.toml (fallback ON, 15-min cooldown) — derive(Default) would
    /// silently give `false`/0 and defeat the snapshot fallback.
    fn default() -> Self {
        Self {
            enabled: false,
            source: String::new(),
            poll_interval_minutes: default_poll(),
            install_name: default_install(),
            snapshot_key: String::new(),
            fallback_enabled: default_fallback_enabled(),
            fallback_cooldown_minutes: default_fallback_cooldown(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoleConfig {
    #[serde(default = "default_role")]
    pub mode: String, // "hos" | "bos" | "remote-hos"
}

impl Default for RoleConfig {
    fn default() -> Self {
        Self { mode: default_role() }
    }
}

fn default_host() -> String { "0.0.0.0".into() }
fn default_port() -> u16 { 8080 }
fn default_data_dir() -> String { "data".into() }
fn default_poll() -> u64 { 15 }
fn default_role() -> String { "hos".into() }

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig { host: default_host(), port: default_port() },
            paths: PathsConfig { data_dir: default_data_dir() },
            database: DatabaseConfig::default(),
            sync: SyncConfig::default(),
            role: RoleConfig { mode: default_role() },
        }
    }
}

impl AppConfig {
    /// Load config from `<exe_dir>/config.toml`, falling back to a default
    /// config if the file is absent (first run). Returns (config, path_used).
    pub fn load() -> Result<(AppConfig, PathBuf), anyhow::Error> {
        let exe_dir = anchor_cwd_to_exe();
        let path = exe_dir.join("config.toml");
        if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            let cfg: AppConfig = toml::from_str(&text)
                .map_err(|e| anyhow::anyhow!("config.toml parse error: {e}"))?;
            Ok((cfg, path))
        } else {
            Ok((AppConfig::default(), path))
        }
    }

    /// Resolve the data dir to an absolute path anchored at the exe dir.
    pub fn data_dir_abs(&self) -> PathBuf {
        let exe_dir = anchor_cwd_to_exe();
        let p = Path::new(&self.paths.data_dir);
        if p.is_absolute() { p.to_path_buf() } else { exe_dir.join(p) }
    }

    pub fn is_author(&self) -> bool {
        self.role.mode == "hos" || self.role.mode == "remote-hos"
    }
    pub fn is_remote_hos(&self) -> bool {
        self.role.mode == "remote-hos"
    }
}

/// Anchor the process CWD to the exe's directory so config.toml / data / web
/// resolve regardless of how the app was launched (service, shortcut, shell).
pub fn anchor_cwd_to_exe() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let _ = std::env::set_current_dir(dir);
            return dir.to_path_buf();
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}
