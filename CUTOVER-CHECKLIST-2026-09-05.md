# WebRMS-Next — Store Cutover Checklist (P4)
**Date:** 2026-09-05 · **Applies to:** taking a store from the old AKPOS-bound WebRMS to a
WebRMS-Next install (DESIGN §6, WORKFLOWS). Every step is a gate: do not proceed past a
FAILED step until it is resolved and re-run.

## Pre-flight (on the target install)
- [ ] `webrms-next init` — data dir + schema migrated (`10/10` per `doctor`).
- [ ] `config.toml` correct: `[role] mode`, `[database] connection_string`, `[sync] source`
      + `install_name` (unique per install), `snapshot_key` (same across the group).
- [ ] `webrms-next doctor` → **no FAIL** (integrity ok, migrations applied, connector
      reachable, backups present or taken now).
- [ ] `webrms-next backup` taken → note the file (rollback point).

## Seed (one time, from the live AKPOS)
- [ ] `webrms-next seed <new-hos|old-hos|bos>` completes with the parity banner
      (`items / stock / sales / receipts / ap / promos / rbp` counts).
- [ ] **Parity gate:** `webrms-next parity` → `PARITY OK` on every table
      (branch/department/supplier/item/receipt/ap/promo counts == live).
- [ ] Spot-check values, not just counts: a known supplier's items on the order sheet,
      a known receipt total, an AP invoice amount — agree with the live system.

## Cutover (run the new app for real)
- [ ] HoS first: start the new app → `/api/mode` shows the right role; `/api/health`
      connector ok; ordering/payables/promotions/reports pages serve 200 with the
      expected seeded numbers (compare a couple against the old WebRMS running side by side).
- [ ] BoS installs: start against the HoS (`[sync] source`) → first poll pulls config down;
      `doctor` replication checks PASS (lag < interval).
- [ ] **Stocktake + ordering smoke** on a BoS: one stocktake export writes `.txt`/`.qry`
      byte-identical to the old format; one order → ETL `PurchaseOrder-*.xlsx` appears in
      the HoS Incoming POs and auto-flips on receipt (BillOfLading → trans_no → G-row).
- [ ] Fallback drill (recommended): point a BoS connector at a dead host, confirm the
      snapshot fallback engages within 3 polls and recovers when the host returns.
- [ ] Keep the old system running alongside for one full ordering cycle (read-only
      comparison) before decommissioning.

## Decommission (only after a full verified cycle)
- [ ] Old WebRMS service stopped; data dir archived (not deleted).
- [ ] Nightly backup confirmed scheduling (or documented manual cadence).

## Known open items (2026-09-05 — do not gate cutover on these)
- Payment Mix + Hourly Curve reports: need per-day payment-method / hour-of-day rollups
  (`sales_payment`, `sales_hourly`) + connector pulls — NOT derivable from `sales_daily`.
- Replacement-report (ordering diagnostics), costed stocktake variance (G-3), item
  edit/clone ETL (W6): schema is ready (`item_change_requests`, `stocktake_runs`…),
  modules not yet built — see DESIGN §8 follow-ups.
