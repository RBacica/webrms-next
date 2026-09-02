// Ordering & Demand module — ported from WebRMS (ordering-demand lineage).
// forecast: pure engine ported VERBATIM with its 27 regression tests (O-6 —
// do not "improve": the calendar-denominator + halted-line + no-history rules
// are hard-won).
// etl: byte-compatible Infinity ETL PO writer.
// orders: lifecycle over local SQLite (G-10 cleared_order_ids).
// db/handlers: order sheet (forecast over local data) + post → PO ETL →
// incoming-PO tracking (W5) + supplier confirmation CSV (G-5).
pub mod db;
pub mod etl;
pub mod forecast;
pub mod handlers;
pub mod orders;
