// Connector layer — pull from live AKPOS systems (read-only, high-water marked).
// One implementation per live-system type; all tiberius-backed.

pub mod hos;

use serde::{Deserialize, Serialize};

/// Incremental pull position for one source+table.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HighWater {
    pub source: String,
    pub table: String,
    pub last_key: Option<String>,
}

/// A batch of rows returned by a pull. Carries the next high-water key so the
/// caller can persist it once the batch is committed.
#[derive(Debug)]
pub struct PullResult<T> {
    pub rows: Vec<T>,
    pub next_key: Option<String>,
}

/// Core connector trait. Every method is a pure read from a live system.
/// Async-trait keeps it dyn-compatible (the poller holds `Box<dyn PollConn>`).
#[async_trait::async_trait]
pub trait Connector: Send + Sync {
    async fn probe(&self) -> anyhow::Result<ProbeInfo>;

    async fn pull_branches(&self) -> anyhow::Result<Vec<LiveBranch>>;
    async fn pull_departments(&self) -> anyhow::Result<Vec<LiveDepartment>>;
    async fn pull_suppliers(&self) -> anyhow::Result<Vec<LiveSupplier>>;
    async fn pull_items(&self, hw: &HighWater, limit: i64) -> anyhow::Result<PullResult<LiveItem>>;
    async fn pull_barcodes(&self, upcs: &[String]) -> anyhow::Result<Vec<LiveBarcode>>;
    async fn pull_stock(&self, branch_id: i32) -> anyhow::Result<Vec<LiveStock>>;
    async fn pull_sales(&self, hw: &HighWater, limit: i64) -> anyhow::Result<PullResult<LiveSaleLine>>;
    async fn pull_receipts(&self, hw: &HighWater, limit: i64) -> anyhow::Result<PullResult<LiveReceipt>>;
    async fn pull_ap(&self, hw: &HighWater, limit: i64) -> anyhow::Result<PullResult<LiveApInvoice>>;
    async fn pull_promos(&self) -> anyhow::Result<Vec<LivePromoRule>>;
    async fn pull_pricing_groups(&self) -> anyhow::Result<Vec<LivePricingGroup>>;
    async fn pull_pricing_sets(&self) -> anyhow::Result<Vec<LivePricingSet>>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeInfo {
    pub db_ok: bool,
    pub engine: String, // "Rules_Based" | "Standard"
    pub branch_ids: Vec<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveBranch {
    pub id: i64,
    pub is_ho: bool,
    pub name: String,
    pub short_name: String,
    pub address: Option<String>,
    pub city: Option<String>,
    pub region: Option<String>,
    pub postcode: Option<String>,
    pub country: Option<String>,
    pub phone: Option<String>,
    pub gst_no: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveDepartment {
    pub id: i64,
    pub name: String,
    pub target_margin: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveSupplier {
    pub code: String,
    pub last_name: String,
    pub first_name: Option<String>,
    pub disc_group: Option<i32>,
    pub disc_percent: Option<f64>,
    pub disc_days: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveItem {
    pub upc: String,
    pub sku: String,
    pub description: String,
    pub department: Option<i32>,
    pub sub_department: Option<i32>,
    pub class: Option<i32>,
    pub supplier: Option<String>,
    pub parent_upc: Option<String>,
    pub cost: f64,
    pub cost_ave: f64,
    pub purchase_cost: f64,
    pub price1: f64,
    pub price2: f64,
    pub price3: f64,
    pub price4: f64,
    pub price5: f64,
    pub price6: f64,
    pub price7: f64,
    pub price8: f64,
    pub tax_no: Option<i32>,
    pub pack_units: f64,
    pub volume_ml: Option<f64>,
    pub non_stock: bool,
    pub inactive: bool,
    pub updated: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveBarcode {
    pub upc: String,
    pub barcode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveStock {
    pub branch_id: i32,
    pub upc: String,
    pub qty: f64,
    pub as_of: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveSaleLine {
    pub branch_id: i32,
    pub upc: String,
    pub sale_date: String, // YYYY-MM-DD
    pub units: f64,
    pub revenue: f64,
    pub line_type: String, // 'N' | 'S'
    pub cost: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveReceipt {
    pub branch_id: i32,
    pub trans_no: i64,
    pub station: i32,
    pub trans_type: String, // P/G/I/Z
    pub supplier: Option<String>,
    pub invoice_no: Option<String>,
    pub total_cost: f64,
    pub logged: String,
    pub lines: Vec<LiveReceiptLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveReceiptLine {
    pub upc: String,
    pub quantity: f64,
    pub unit_cost: f64,
    pub ext_cost: f64,
    pub status: Option<String>,
    pub cost_ave_local: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveApInvoice {
    pub branch_id: i32,
    pub supplier_code: Option<String>,
    pub invoice_number: Option<String>,
    pub description: Option<String>,
    pub invoice_date: Option<String>,
    pub due_date: Option<String>,
    pub discount_date: Option<String>,
    pub invoice_amount: f64,
    pub paid_amount: f64,
    pub discount_amount: f64,
    pub discount_pc: Option<f64>,
    pub po_number: Option<String>,
    pub freight: f64,
    pub tax_amount1: f64,
    pub status: Option<String>,
    pub is_matched: bool,
    pub logged: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LivePromoRule {
    /// 'special' (Standard engine) | 'rbp_condition' (Rules_Based engine)
    pub kind: String,
    pub source_key: String,
    pub description: Option<String>,
    pub sequence_match: Option<String>,
    pub condition_type: Option<String>,
    pub adjustment_type: Option<String>,
    pub adjustment_value: Option<f64>,
    pub effective_start: Option<String>,
    pub effective_end: Option<String>,
    pub branch_scope: Option<i32>,
    pub inactive: bool,
    pub payload: String, // raw JSON of the source row
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LivePricingGroup {
    pub group_id: i64,
    pub description: String,
    pub data_key: String, // item UPC (Type='Items')
    pub type_: String,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LivePricingSet {
    pub set_id: i64,
    pub set_line: i64,
    pub group_id: i64,
    pub min_qty: f64,
    pub max_qty: f64,
}
