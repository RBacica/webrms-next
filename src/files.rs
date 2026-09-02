// Minimal files module: data dir layout + atomic writes (crash-safe, A1).
use std::path::{Path, PathBuf};

pub const CONFIG_DIR: &str = "config";
pub const STATE_DIR: &str = "state";
pub const OUTPUT_DIR: &str = "output";

/// Atomic file write: temp file in the same dir + rename. A crash mid-write
/// can never leave a half-written ETL/export file (B6 crash-safe writes).
pub fn write_atomic(data_dir: &Path, subdir: &str, filename: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    let dir = data_dir.join(subdir);
    std::fs::create_dir_all(&dir)?;
    let final_path = dir.join(filename);
    let tmp_path = dir.join(format!(".{}.tmp", filename));
    std::fs::write(&tmp_path, bytes)?;
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(final_path)
}

/// Bootstrap the standard data tree (config/state/output + subdirs).
pub fn bootstrap(data_dir: &Path) -> std::io::Result<()> {
    for d in [
        CONFIG_DIR,
        STATE_DIR,
        "output/stocktake",
        "output/tickets",
        "output/payables",
        "output/ordering",
        "output/downloads",
        "output/incoming-po",
        "logs",
    ] {
        std::fs::create_dir_all(data_dir.join(d))?;
    }
    Ok(())
}
