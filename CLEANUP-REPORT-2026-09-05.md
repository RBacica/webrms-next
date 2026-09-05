# WebRMS-Next — Cleanup & Optimization Report (2026-09-05)
**Plan:** `CLEANUP-PLAN-2026-09-05.md` · **Commit:** `a0508b8` (pushed) · **Tests:** 84/84 green
(51 lib + 33 integration), 0 warnings, `x86_64-pc-windows-gnu` check clean.
**Verified live** on the seeded 3-instance cluster (HoS :8096 / BoS :8097, real AKPOS data).

## Done this pass

### A1 — Order-sheet N+1 eliminated + all-branch history bug fixed (the big one)
- **Before:** `order_sheet` issued ~4 queries per line (stock, on-order,
  sales_daily 92-day, promo windows) — a 450-line supplier (010) ⇒ **~1,800
  SQLite round-trips per sheet load**.
- **After:** 4 fixed queries (+1 per 400-UPC chunk for very large suppliers) with
  chunked `IN` lists; the per-line forecast loop is pure map lookups. The old
  per-upc `product_history` is gone, replaced by a shared pure `assemble_history`.
- **Bug found in the same code:** `branch=None` (HoS "All branches") queried sales
  history at `branch_id = 0`, which matches nothing ⇒ **every all-branch forecast
  had zero sales history** (silent bad suggestions). The batch path now
  `SUM`s sales across branches per (upc, day).
- **Live evidence:** supplier-010 sheet 450 lines: **0.12 s → 0.07 s**; all-branch
  sheet (623 lines) now shows 222 lines with real sales history and sane
  suggested quantities (previously all zero).
- Regression test: `order_sheet_batched_semantics_branch_and_all` (branch-scoped
  stock/on-order/history vs all-branch sums).

### A2 — Overview stock snapshot: JOIN instead of correlated subquery
Per-UPC `(SELECT cost FROM items …)` inside the stock aggregation → `LEFT JOIN items`.
Single-pass. Live: overview (all + branch 17) returns correct items/value/stockout.

### A3 — Stocktake count-session UI (gap closed)
The stocktake view was a stub that searched but **never saved**. Now a real
session: department filter, barcode/UPC quick-add + description search (focus
stays on the input — G-6 barcode UX), count grid with live variance + stats,
**Save → server** (writes `.txt`/`.qry`, records the `stocktake_runs` row),
**Download files** (client `.txt`/`.qry`), session Reset. Field names matched to
the API (`StockItem`, `refresh-upc → stock_on_hand`, `SaveRow` incl. variance).

### A4 — View timer leak
`incoming.js` pushed a `setInterval(60s)` per visit and never cleared it —
navigating away left pollers running and stacking. `app.js route()` now clears
`el._timers` (and an optional `el._cleanup`) before every view swap; views
register their timers on the element.

### A5 — README
The repo had 12 design/plan docs and no index. Added a README: what it is, stack,
modes/roles, CLI, HTTP surface by module, operations notes (TZ, backups, sqlite
affinity, UI refresh), and a doc map.

## Review outcomes: no-action / deferred (with reasons)
- **Reports**: all other report endpoints are already single-statement GROUP BYs —
  no N+1 found.
- **Payment Mix + Hourly reports**: need new `sales_payment`/`sales_hourly` rollups
  + connector pulls (sales_daily lacks hour/payment dimensions) — a separate
  feature batch, not cleanup.
- **Replacement-report / item-edit ETL (W6) / O-12 / costed stocktake variance
  (G-3)**: documented follow-ups needing design-level build-out.
- Repo hygiene was otherwise clean (`.gitignore` correct, tree clean).

## Deferred candidates worth the next session
1. Payment Mix + Hourly report data model (rollups + connector + endpoints + UI).
2. P5 release packaging (three-role zips) + B6 Windows service live verification.
