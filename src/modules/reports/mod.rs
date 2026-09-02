// Reports module — local SQLite port of WebRMS reports.
// Sales windows read sales_daily (connector-aggregated); stock from
// stock_current; GP uplifted by scanback/rebate receipts (C2).
pub mod db;
pub mod handlers;
