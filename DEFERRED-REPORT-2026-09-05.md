# Deferred Tasks — Implementation Report (2026-09-05)

Commit `93ad14d` on `main` (pushed). Release zips + Windows live test deliberately held per instruction.

## 1. G-3 — Costed stocktake variance (done)
- Migration `0011_stocktake_run_lines.sql`: exports now persist every counted row
  (upc, description, SOH, counted, unit cost, variance units, variance $).
- `record_run` computes `shrink_total` / `overage_total` on the run (summed from
  costed variance), with unit cost batch-looked-up from `items`.
- Reports → Stocktake & Shrink now shows Shrink $ / Overage $ per run + expandable lines.
- Regression: export → runs carry costed totals (in `stocktake_export_records_run`).

## 2. Replacement report (done)
- `GET /api/ordering/replacement-report` — local port of old WebRMS semantics:
  inactive same-description predecessors of active items, **match levels**
  `3` = already `OLD_<new>` (SKU convention), `2` = same supplier product code
  (new `items.supplier_prod_code` col, migration 0012), `1` = same supplier,
  `0` = neither; only predecessors with sales history.
- UI: "Replacement report" button on the Ordering toolbar → table.
- Live: **28 candidates**, incl. level-3 Church Road (already renamed) and a
  genuine level-2 (Espolon: SPC match across UPC change).

## 3. W6 — item clone / edit + Item-ETL patch + O-12 (done)
- New `items` module: search (active+inactive), detail, edit, **clone**.
  Clone implements the user's convention exactly: copy item to new UPC → apply
  edits → **retire old** (`SKU = OLD_<new>`, `is_active=0`) → **alt barcode = old
  UPC** on the new item → **history_alias** (old→new) so forecasts/sellout
  inherit old sales.
- Item-ETL patch export: `Item-<kind>-<upc>-<ts>.xlsx`, sheet `Infinity ETL`,
  **column headers verified against a real Item-2026-08-04 export** (incl. the
  `SKU` + `Alternate Barcode` columns, so Infinity can apply the rename +
  alt-barcode via the patch); only changed rows/columns. Tracked in
  `item_etl_exports`, downloadable via `/api/items/patch/<file>`.
- **O-12 external-edit protection**: connector `upsert_item` now consults
  `app_overrides` per pulled UPC — overridden fields keep the **app value** (the
  connector never clobbers an app edit), and when the live AKPOS value differs
  from the override the override is flagged `external_edit` (visible in
  `/api/health`-adjacent state; surfaced on the item card as `overridden`).
- Tests: `items_clone_edit_and_o12_protection` (clone semantics, override rows,
  ETL file written + downloadable).
- Live: cloned `5010677014205` → `9990000000420` w/ cost edit; old retired
  `OLD_9990000000420`; ETL patch downloaded, headers + edited row verified
  (real supplier 014/989453, price2, edited cost 44.9).

## 4. Payment Mix + Hourly reports (done)
- Migration `0013`: `sales_payment` (branch/date/media/txns/value/fees/change)
  and `sales_hourly` (branch/date/hour/dow/station/txns/net).
- Connector: `pull_payments` (TransPayments JOIN TransHeaders, status C) and
  `pull_hourly` (TransHeaders status C, types C/A/M) — full at seed, trailing
  3 days per poll (idempotent date-keyed upserts).
- Endpoints `/api/reports/payments` + `/api/reports/hourly` (from/to/branch);
  Reports UI gains Payment Mix + Hourly Curve tabs (hourly shows a txns bar
  curve).
- Live (Aug 2026, all branches): VISA $249k/5,834 txns, EFTPOS $107k/2,410,
  CASH $85k/2,085; hourly curve opens at 10:00 (14 txns at 9am is **correct** —
  store's first logged sale is 10:01; verified against live AKPOS query).

## Verification
- `cargo test`: **86/86** (51 lib + 35 integration), 0 warnings.
- `cargo check --target x86_64-pc-windows-gnu`: clean (B6 compile check held).
- Cluster running new release build: HoS :8096 (source), BoS :8097 (client).

## Remaining (held / later)
- P5 release zips + B6 Windows service live verify (explicitly held).
- O-12 external-edit UI surfacing beyond the card flag (item edit history view).
- Suggest→approve workflow for BoS item changes (single-operator clone shipped).
