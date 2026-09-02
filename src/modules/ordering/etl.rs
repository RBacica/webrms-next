// ── ETL Purchase Order export ──────────────────────────────────────
// Generates a ready-to-import Infinity ETL Purchase Order file (.xlsx).
// Matches the guide's "Advanced Imports: Purchase Orders" worked example
// (TQUSR10 pp.130–137): a single-branch PO is one H block (header row +
// branch row) followed by one D block (header row + one row per line item).
// All rows of one PO share the same POID.
//
// FORMAT: ETL requires a real Excel (.xlsx) file. CSV is NOT accepted
// (verified 2026-08-28).
//
// LAYOUT (from the guide's worked example on p.135 — the prose's long 24-col
// list does NOT match the real template): each block has its OWN compact
// header row immediately followed by its data rows. ETL reads columns
// POSITIONALLY.
//   H header: POID,H,Supplier,BranchDestination,AuthoriseBy,BillOfLading,EstArrival,ExtReference,FC,FCRate
//   D header: POID,D,UPC,Quantity,UnitCost,Tax,SupplierProdCode,PurchaseUnit,PurchaseQty,FCCost
// We populate only the fields we carry; the rest stay blank (ETL tolerates
// blanks / auto-calculates totals). Named index constants guard the mapping.
use rust_xlsxwriter::Workbook;

/// File extension for the exported ETL PO file.
pub const PO_EXT: &str = "xlsx";

/// True for a generated ETL PO filename (PurchaseOrder-* with .csv or .xlsx).
pub fn is_po_filename(name: &str) -> bool {
    name.starts_with("PurchaseOrder-")
        && (name.ends_with(".csv") || name.ends_with(".xlsx"))
}

/// One line item of a purchase order to export.
pub struct EtlPoLine {
    pub upc: String,
    pub qty: f64,
    pub cost: f64,
}

/// Generate a unique 10-digit BillOfLading code for a PO.
/// Time-based (epoch seconds, 10 digits) + a per-second counter ensures
/// uniqueness across rapid successive orders. Deterministic given the clock;
/// collisions are astronomically unlikely and would need two orders in the
/// same second AND the same counter (guarded by an atomic).
pub fn generate_bill_of_lading() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let epoch = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // 10 digits: 8 from epoch (mod 1e8) + 2 from a rolling sequence (0-99).
    let base = epoch % 100_000_000;
    let seq = SEQ.fetch_add(1, Ordering::Relaxed) % 100;
    format!("{:08}{:02}", base, seq)
}

/// H-block header (branch/destination level) — the COMPACT set per the guide's
/// worked example (p.135). ETL reads columns positionally; the long 24-col
/// prose list in the guide does NOT match the actual template. Each block has
/// its OWN header row followed by its data rows.
const H_HEADER: &[&str] = &[
    "POID", "H", "Supplier", "BranchDestination", "AuthoriseBy", "BillOfLading",
    "EstArrival", "ExtReference", "FC", "FCRate",
];

/// D-block header (line-item level) — the COMPACT set per the guide's worked
/// example (p.135). FCCost is present (ETL errors if the column is missing).
const D_HEADER: &[&str] = &[
    "POID", "D", "UPC", "Quantity", "UnitCost", "Tax", "SupplierProdCode",
    "PurchaseUnit", "PurchaseQty", "FCCost",
];

/// H-row value indices into H_HEADER (named consts so the mapping is auditable
/// and a column can never be silently misaligned).
const H_POID: usize = 0;
const H_MARKER: usize = 1;
const H_SUPPLIER: usize = 2;
const H_BRANCH: usize = 3;
const H_AUTHORISE: usize = 4;
const H_BILL_OF_LADING: usize = 5;
// EstArrival (index 6) is left blank — optional; not populated.
const H_EXT_REF: usize = 7;

/// D-row value indices into D_HEADER.
const D_POID: usize = 0;
const D_MARKER: usize = 1;
const D_UPC: usize = 2;
const D_QTY: usize = 3;
const D_COST: usize = 4;

/// Build the ETL purchase order workbook bytes for a single-branch PO.
/// Each block carries its OWN header row immediately followed by its data rows
/// (no blank separator), matching the guide's worked example (p.135):
///   row 0  H header,  row 1  H data
///   row 2  D header,  rows 3..  D data
/// Populates only the fields we carry; optional/blank columns are left empty
/// (ETL tolerates blanks, auto-calculates totals). ETL requires a real .xlsx
/// (CSV is not accepted).
pub fn build_purchase_order_xlsx(
    poid: i64,
    supplier: &str,
    branch: i64,
    ext_ref: Option<&str>,
    authorised_by: Option<&str>,
    bill_of_lading: &str,
    lines: &[EtlPoLine],
) -> Result<Vec<u8>, String> {
    let mut workbook = Workbook::new();
    {
        let worksheet = workbook.add_worksheet();
        worksheet.set_name("Infinity ETL").ok();

        // H block: header row + one branch row.
        write_header(worksheet, 0, H_HEADER);
        let mut h_row: Vec<CellValue> = vec![CellValue::None; H_HEADER.len()];
        h_row[H_POID] = CellValue::Int(poid);
        h_row[H_MARKER] = CellValue::Text("H".to_string());
        h_row[H_SUPPLIER] = CellValue::Text(supplier.to_string());
        h_row[H_BRANCH] = CellValue::Int(branch);
        h_row[H_AUTHORISE] = CellValue::Text(authorised_by.unwrap_or("").to_string());
        h_row[H_BILL_OF_LADING] = CellValue::Text(bill_of_lading.to_string());
        h_row[H_EXT_REF] = CellValue::Text(ext_ref.unwrap_or("").to_string());
        write_row(worksheet, 1, &h_row);

        // D block: header row + one row per line, immediately after the H block.
        let d_start: u32 = 2;
        write_header(worksheet, d_start, D_HEADER);
        for (i, line) in lines.iter().enumerate() {
            let mut d_row: Vec<CellValue> = vec![CellValue::None; D_HEADER.len()];
            d_row[D_POID] = CellValue::Int(poid);
            d_row[D_MARKER] = CellValue::Text("D".to_string());
            d_row[D_UPC] = CellValue::Text(line.upc.clone());
            d_row[D_QTY] = CellValue::Num(line.qty);
            d_row[D_COST] = CellValue::Num(line.cost);
            write_row(worksheet, d_start + 1 + i as u32, &d_row);
        }
    } // worksheet borrow ends here

    workbook
        .save_to_buffer()
        .map_err(|e| format!("Failed to serialize xlsx: {e}"))
}

/// A cell value abstraction so one writer handles strings, ints and floats.
#[derive(Clone)]
enum CellValue {
    None,
    Text(String),
    Int(i64),
    Num(f64),
}

fn write_header(ws: &mut rust_xlsxwriter::Worksheet, row: u32, headers: &[&str]) {
    for (col, h) in headers.iter().enumerate() {
        let _ = ws.write_string(row, col as u16, *h);
    }
}

fn write_row(ws: &mut rust_xlsxwriter::Worksheet, row: u32, cells: &[CellValue]) {
    for (col, cell) in cells.iter().enumerate() {
        let c = col as u16;
        let _ = match cell {
            CellValue::None => Ok::<(), rust_xlsxwriter::XlsxError>(()),
            CellValue::Text(s) => {
                let _ = ws.write_string(row, c, s);
                Ok(())
            }
            CellValue::Int(v) => {
                let _ = ws.write_number(row, c, *v as f64);
                Ok(())
            }
            CellValue::Num(v) => {
                let _ = ws.write_number(row, c, *v);
                Ok(())
            }
        };
    }
}
