# WebRMS-Next

Self-contained retail platform for the Infinity RMS / AKPOS environment. Every install
owns a local SQLite database and runs fully standalone; it can additionally **pull from a
live AKPOS SQL Server** through a connector layer (to run *alongside* the old system or
make migrating off it scripted and verifiable). Reads never touch the live system at
request time — live systems are data sources, not dependencies.

Succeeds the original WebRMS (`~/Projects/webrms-sql-infinityrms`, actix-web + per-request
AKPOS reads). Full design: `DESIGN-2026-09-02.md`.

## Stack

axum 0.8 · sqlx 0.9 (SQLite, WAL) · tiberius 0.12 (AKPOS connector) · tokio · serde ·
clap · tracing · tower-http static SPA (`web/`).

## Modes & roles

- `[role] mode = hos | bos | remote-hos | standalone` (config.toml next to the exe).
- **HoS** — authority: seeds/pulls the group's data, authors config, serves replication.
- **BoS** — runs its own connector to the branch's AKPOS; if the connector stays dead
  (3 polls) it auto-restores the HoS catalog snapshot (`[sync] fallback_enabled`) and
  recovers on the first good poll. Config writes 403 on a BoS.
- **Remote-HoS** — author that is also a sync client (laptop pointed at the HoS DB).
- **Standalone** — no connector, no sync.

## CLI

```text
webrms-next run                serve (console; graceful ctrl-c/SIGTERM)
webrms-next init               bootstrap data dir + schema
webrms-next seed <source>      full connector seed (auto pre-seed backup first)
webrms-next doctor             diagnostics: integrity/migrations/connector/repl/backups/disk
webrms-next backup [--keep N]  VACUUM INTO snapshot, keep-N retention
webrms-next parity             cutover gate: live AKPOS counts vs local DB table-by-table
webrms-next service …          Windows service (install/start/stop/remove/run — SCM-native)
```

## HTTP surface

Modules and their endpoints (see `PARITY-VS-WEBRMS-2026-09-05.md` for the feature map):

- **Ordering** — suppliers, sheet (forecast), orders (POST → ETL PO .xlsx + BillOfLading),
  settings/modes (author-gated, replicated), confirmation/export CSV, incoming-PO (status
  auto-flip: waiting_import → pending_receipt → receipted).
- **Stocktake** — departments, suppliers(-for-dept), sub-departments, search, refresh-upc,
  barcode-lookup, export (.txt count + .qry tickets, run recorded).
- **Payables** — invoices/returns (GRN incl. 'Z'), config (bulk, author-gated), paid, pay,
  export; BoS read-only.
- **Promotions** — engine, list, items, effectiveness (Rules_Based RBP resolved locally).
- **Reports** — overview (+movers/dept-movers/dept-weekly), daily, depts, stock valuation,
  receipts (GRN↔AP), stocktakes. (Payment Mix + Hourly: open — need new rollups.)
- **System** — mode, health (connector age / repl lag / fallback state), version,
  sync: now / status / outbox / up / snapshot (HMAC-signed gzip DB).

## Operations notes

- Timestamps stored local (`%Y-%m-%d %H:%M:%S`) — never mix in `datetime('now')` (UTC)
  where a local reader parses.
- Backups: `data/backup/webrms-next-backup-<ts>.db` (VACUUM INTO — safe while running).
- Replication: outbox per write (orders bidir per branch, config/paid down, POs up) +
  poll backstop; snapshot fallback preserves local app rows via backup re-import.
- sqlite affinity: wrap aggregates `CAST(COALESCE(SUM(..),0) AS REAL)` when binding to f64.
- UI static files hot-reload on hard refresh (Ctrl+Shift+R); `web/js/app.js?v=N` bump after
  frontend changes.

## Docs index (repo root)

| Doc | What |
|---|---|
| `DESIGN-2026-09-02.md` | Architecture + phase plan (approved) |
| `ENHANCEMENTS-2026-09-02.md` | A1–D4 optimization/reliability scope |
| `WORKFLOWS-2026-09-02.md` | Role × mode workflows |
| `REVIEW-GAP-ANALYSIS-2026-09-02.md` | Plan review: must-adds/optimizations |
| `LIVE-DB-FINDINGS-2026-09-02.md` | Live AKPOS data facts the design rests on |
| `PARITY-VS-WEBRMS-2026-09-05.md` | Feature coverage vs the original WebRMS |
| `CUTOVER-CHECKLIST-2026-09-05.md` | Store cutover gate (P4) |
| `CLEANUP-PLAN-2026-09-05.md` / `CLEANUP-REPORT-2026-09-05.md` | Optimization/cleanup pass |
