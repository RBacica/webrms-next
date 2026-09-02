// Stocktake module — ported from WebRMS (stocktake-v3 lineage) onto the local
// SQLite DB. Reads items/stock_current/barcodes instead of live AKPOS.
pub mod db;
pub mod exports;
pub mod handlers;
