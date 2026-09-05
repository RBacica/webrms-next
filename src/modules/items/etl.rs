// Item maintenance ETL patch (W6) — an Item-ETL workbook Infinity can import.
// Column HEADERS are exact matches to the real Item-*.xlsx export (verified
// against Item-2026-08-04-14-33-19.xlsx: sheet 'Infinity ETL', 98 cols; ETL
// locates fields by header name). We emit ONLY the columns that matter for the
// change set — Infinity tolerates a header-named subset (same mechanism the
// PO ETL uses).
//
// Rows emitted (see handlers):
//   edit  → one row per changed UPC (Product code + SKU + edited columns)
//   clone → new-UPC row (create) + old-UPC row (SKU = OLD_<new>, InActive = 1)

use rust_xlsxwriter::Workbook;

/// The canonical header row (subset we write, exact export names).
pub const HEADERS: &[&str] = &[
    "Product code", "SKU", "Description", "Department", "SubDepartment", "Class",
    "Price1", "Price2", "Price3", "Price4", "Price5", "Price6", "Price7", "Price8",
    "Supplier", "Supplier Product Code", "Alternate Barcode",
    "Cost", "CostAve", "Pack Cost", "Pack Size (Units)", "InActive",
];

#[derive(Debug, Clone, Default)]
pub struct EtlRow {
    pub product_code: String,
    pub sku: Option<String>,
    pub description: Option<String>,
    pub supplier: Option<String>,          // numeric supplier code
    pub supplier_prod_code: Option<String>,
    pub alternate_barcode: Option<String>,
    pub cost: Option<f64>,
    pub cost_ave: Option<f64>,
    pub pack_cost: Option<f64>,
    pub pack_size: Option<f64>,
    pub price1: Option<f64>,
    pub inactive: Option<bool>,
}

/// Build an Item-ETL .xlsx patch. Returns the bytes (caller writes the file).
pub fn build_item_patch(rows: &[EtlRow]) -> Vec<u8> {
    let mut wb = Workbook::new();
    let sheet = wb.add_worksheet();
    sheet.set_name("Infinity ETL").ok();
    // header row
    for (c, h) in HEADERS.iter().enumerate() {
        sheet.write_string(0, c as u16, *h).unwrap();
    }
    for (r, row) in rows.iter().enumerate() {
        let r = (r + 1) as u32;
        sheet.write_string(r, 0, &row.product_code).unwrap();
        sheet.write_string(r, 1, row.sku.as_deref().unwrap_or(&row.product_code)).unwrap();
        if let Some(d) = &row.description { sheet.write_string(r, 2, d).unwrap(); }
        if let Some(p) = row.price1 { sheet.write_number(r, 7, p).unwrap(); }
        if let Some(s) = &row.supplier { sheet.write_string(r, 14, s).unwrap(); }
        if let Some(s) = &row.supplier_prod_code { sheet.write_string(r, 15, s).unwrap(); }
        if let Some(a) = &row.alternate_barcode { sheet.write_string(r, 16, a).unwrap(); }
        if let Some(v) = row.cost { sheet.write_number(r, 17, v).unwrap(); }
        if let Some(v) = row.cost_ave { sheet.write_number(r, 18, v).unwrap(); }
        if let Some(v) = row.pack_cost { sheet.write_number(r, 19, v).unwrap(); }
        if let Some(v) = row.pack_size { sheet.write_number(r, 20, v).unwrap(); }
        if let Some(v) = row.inactive { sheet.write_string(r, 21, if v { "1" } else { "0" }).unwrap(); }
    }
    wb.save_to_buffer().unwrap_or_default()
}
