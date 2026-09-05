# WebRMS-Next vs WebRMS — Feature Parity Report
**Date:** 2026-09-05 · **Compared:** original WebRMS `webrms-sql-infinityrms` @ `b285e95`
(live production codebase) vs WebRMS-Next `~/Projects/webrms-next` @ `e384430`.
**Method:** route-surface diff (every `/api/*` route in both codebases), UI nav/feature
diff, then a live HTTP sweep of every endpoint against the WebRMS-Next cluster HoS
(seeded from the real gg-core-hos AKPOS, 6,731 items / 78,873 sales rows / 1,074 receipts).
**Tests:** 83/83 green (51 lib + 32 integration), 0 warnings.

## Feature matrix

| Area | Old WebRMS (b285e95) | WebRMS-Next (e384430) | Status |
|---|---|---|---|
| **Stocktake** | dept/supplier search, barcode w/ alt+primary fallback, refresh SOH, count session, export .txt/.qry (server+client) | same backend endpoints + record-run on export | ✅ working (UI thinner — see open items) |
| **Ordering sheet** | forecast sheet, per-supplier weekly/monthly modes (lead/cycle/cover), global ignore-min/max, active-only, mark-ordered, multi-supplier/consolidation | same forecast engine (verbatim + 27 tests), sheet + active-only + show-all + qty entry | ✅ sheet + settings/modes added this session |
| **Ordering writes** | order → ETL PO .xlsx, incoming-PO tracking, confirmation CSV/print | same: POST order → ETL .xlsx + BillOfLading + auto-flip lifecycle; confirmation CSV | ✅ working |
| **Ordering config** | modes + settings author UI (HoS), replicated to clients | settings/modes endpoints (author-gated, outbox-replicated) + ⚙ panel | ✅ added + live-verified HoS→BoS |
| **Ordering export** | CSV to server/client | POST /api/ordering/export → server file + download | ✅ added |
| **Ordering replacement-report** | item replacement diagnostics page | — | ⬜ open (niche diagnostic; sheet already flags possible_replacement) |
| **Payables** | invoices/returns/config(bulk)/paid/pay/export, read-only BoS | same set (config renamed) + 403 enforcement | ✅ working |
| **Promotions** | list/items/engine/effectiveness (RBP + Standard) | same 4 + local RBP resolver | ✅ working |
| **Reports** | overview (KPI + movers + dept-weekly + dept-movers) + **8 pages**: Daily, Dept&Product, Payment Mix, Hourly, Stock Valuation, Stocktakes, GRN↔AP, Promo Effectiveness | overview ✅; Daily ✅; Dept&Product ✅; **Stock Valuation / GRN↔AP / Stocktakes ADDED this session**; Promo Effectiveness via promotions/effectiveness | 🟡 Payment Mix + Hourly open (see below) |
| **Incoming PO** | list + mark-imported + delete (HoS) | list + status auto-flip + delete (HoS-only) | ✅ working |
| **Sync/replication** | JSON-file distributed sync (config/orders down, PO up, remote badge) | outbox over REST + snapshot fallback + immediate push | ✅ superseded by design (better) |
| **Settings UI** | runtime DB connection settings + save-mode chooser | config.toml per install + role model | ✅ superseded by design |
| **Multi-store** | HoS sees all branches via per-request AKPOS reads | every branch materialized locally (connector/snapshot) | ✅ superseded by design |

## Live verification evidence (cluster HoS, real data)
- All 23 parity endpoints return **HTTP 200** (full sweep above; payables/invoices 400 without
  its required range params, 200 with — same contract as old).
- **Stock valuation:** all-branches → 14 depts, **$1,000,552 retail / $692,275 cost**; branch 12
  (Beth) $421k cost vs branch 17 (WelcB) $271k cost — correct per-branch splits.
- **GRN↔AP:** 27 suppliers with activity in Jun–Sep 2026; goods-in totals per supplier vs AP
  (AP 0 in-window on the seed because seeded invoices predate June — mechanics verified by
  the integration test: G 400 − Z 50 vs AP 350 ⇒ variance 0).
- **Ordering config replication:** HoS set supplier 010 → **monthly/5/30/35 + ignore-min ON**;
  the BoS pulled both rows within one replication poll; BoS POSTs return **403**.
- **Export:** POST /api/ordering/export wrote `orders-010-20260905-175828.csv`
  (Supplier,Branch,UPC,Description,Pack,OrderQty) server-side + returned for download.
- Web UI serves `app.js?v=3` with **Reports** in the nav; ordering view has the ⚙ settings
  panel and Export CSV.

## What this session added (commit e384430)
Ordering settings + modes endpoints (outbox-replicated, author-gated), sheet CSV export,
global-switch plumbing into the forecast, Reports backend (stock/receipts/stocktakes),
stocktake run recording, the Reports UI page (6 tabs + CSV), ordering settings/show-all/
export UI. P4 gate doc `CUTOVER-CHECKLIST-2026-09-05.md`.

## Open items (honest list)
1. **Payment Mix + Hourly Curve reports** — need per-day payment-method and hour-of-day
   rollups (`sales_payment`, `sales_hourly`) + connector pulls; `sales_daily` doesn't carry
   those dimensions. Design-level addition, not a query port.
2. **Full-stacktake UI** — Next's stocktake view is a search/count stub; the export UI flow
   (count session → Save/Restore → .txt/.qry) from old isn't ported to the UI yet (the
   backend + run recording are complete).
3. **Replacement-report** (ordering) — old diagnostic page, not ported.
4. **Item edit/clone ETL (W6)** + costed stocktake variance (G-3) — schema ready, modules
   not built (documented in DESIGN follow-ups).
5. UI depth overall: old views are multi-thousand-line flows; Next's are compact ports.
   Verified working end-to-end at the endpoint level; visual/browser E2E still needs a
   local hard-refresh pass on a reachable box.
