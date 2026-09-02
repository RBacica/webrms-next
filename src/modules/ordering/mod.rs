// Ordering & Demand module — ported from WebRMS (ordering-demand lineage).
// forecast: pure engine ported VERBATIM with its 27 regression tests (O-6 —
// do not "improve": the calendar-denominator + halted-line + no-history rules
// are hard-won).
// etl: byte-compatible Infinity ETL PO writer.
// orders: lifecycle over local SQLite (G-10 cleared_order_ids).
// handlers/db land in P2-2 continuation (order sheet + post + confirmation).
pub mod etl;
pub mod forecast;
pub mod orders;
