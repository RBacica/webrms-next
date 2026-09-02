// Stocktake export writers — .txt count file + .qry ticket file.
// Ported byte-identical from WebRMS (stocktake-v3 formats) so downstream
// Infinity Save/Restore workflows don't change.

use std::path::Path;

use crate::files;
use crate::modules::stocktake::db::SaveRow;

pub struct ExportResult {
    pub count_path: Option<String>,
    pub ticket_path: Option<String>,
    pub count_rows: usize,
    pub ticket_rows: usize,
    pub timestamp: String,
}

/// Write count + ticket files into the data tree (output/stocktake + output/tickets).
pub fn export(data_dir: &Path, rows: &[SaveRow]) -> Result<ExportResult, std::io::Error> {
    let now = chrono::Local::now();
    let ts_file = now.format("%Y-%m-%d-%H-%M-%S").to_string();
    let ts_qry = now.format("%Y%m%d_%H%M%S").to_string();

    let mut result = ExportResult {
        count_path: None,
        ticket_path: None,
        count_rows: 0,
        ticket_rows: 0,
        timestamp: ts_file.clone(),
    };

    // ── count file (.txt) ────────────────────────────────────────
    let genuine_rows: Vec<&SaveRow> = rows
        .iter()
        .filter(|r| r.count != 0.0 || !r.has_ticket)
        .collect();

    if !genuine_rows.is_empty() {
        let fname = format!("stocktake-{}.txt", ts_file);
        let mut out = String::with_capacity(genuine_rows.len() * 32 + 32);
        for r in &genuine_rows {
            out.push_str(&format!("0,{},{:.4},\n", r.upc, r.count));
        }
        let path = files::write_atomic(
            data_dir,
            &format!("{}/stocktake", files::OUTPUT_DIR),
            &fname,
            out.as_bytes(),
        )?;
        result.count_path = Some(path.display().to_string());
        result.count_rows = genuine_rows.len();
    }

    // ── ticket file (.qry) ───────────────────────────────────────
    let ticket_rows: Vec<&SaveRow> = rows.iter().filter(|r| r.has_ticket).collect();
    if !ticket_rows.is_empty() {
        let fname = format!("tickets-{}.qry", ts_qry);
        let mut out = String::new();
        out.push_str("[Header]\r\nApplication=LabelQuery\r\nSaveFileVersion=3\r\n");
        out.push_str(&format!("CriteriaCount={}\r\n", ticket_rows.len()));
        for (i, r) in ticket_rows.iter().enumerate() {
            out.push_str(&format!("[Criteria_{}]\r\n", i + 1));
            out.push_str(&format!("Upc={}\r\n", r.upc));
            if r.ticket_qty > 1 {
                out.push_str(&format!("CopiesException={}\r\n", r.ticket_qty));
            }
        }
        let path = files::write_atomic(
            data_dir,
            &format!("{}/tickets", files::OUTPUT_DIR),
            &fname,
            out.as_bytes(),
        )?;
        result.ticket_path = Some(path.display().to_string());
        result.ticket_rows = ticket_rows.len();
    }

    Ok(result)
}
