// Payables module — local SQLite port of WebRMS payables.
// Bills = ap_invoices (APInv materialized by connector) net of paid_ledger;
// returns = 'Z' receipts (credits); due dates from supplier_terms (EOM+20
// fallback when unconfigured); mark-paid → paid_ledger (outbox-down config
// class); supplier terms config HoS-write / BoS read-only.
pub mod db;
pub mod handlers;
