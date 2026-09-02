# WebRMS-Next — Full-Plan Review: Optimizations & Feature Gaps
**Date:** 2026-09-02 · **Reviews:** DESIGN-2026-09-02.md (v1.1) + ENHANCEMENTS-2026-09-02.md + LIVE-DB-FINDINGS-2026-09-02.md + WORKFLOWS-2026-09-02.md
**Scope:** find optimizations and missing feature gaps across the whole plan, with phase placement. Nothing here is decided — recommendations only.

---

## A. What the plan already gets right (confidence check)

- Authority model ("app-authoritative when configured") — sound, and the `app_override`/LWW merge is the right generalization of the proven modes.json override.
- App-only scanback/rebates, manual ETL imports with status tracking, dual-path BoS (direct + snapshot fallback), three operating modes, parity-report gate, provenance columns on every row — all coherent and grounded in the live DB findings.
- The reviews below are refinements, not corrections.

---

## B. Optimizations (worth folding in)

| # | Item | What / why | Phase |
|---|---|---|---|
| O-1 | **PK/ID strategy for replicated tables** | SQLite AUTOINCREMENT ids collide across installs (branch A order #1 vs branch B order #1). Decide per table: **UUID PK** (or `origin_install + local_id` composite) for every outbox-replicated table (orders, paid, config, rebates, requests); integer ids keyed by `source_key` stay for connector-materialized tables. Must be decided at P0 — retrofitting is painful. | P0 |
| O-2 | **Immediate push on write + poll backstop** | Outbox write → fire-and-forget push to HoS right away; 15-min poll stays as backstop. Cuts perceived latency for orders/POs/paid from ≤15 min to seconds on healthy links. | P3 |
| O-3 | **sales_daily retention + pruning policy** | 36-mo daily grows unbounded. Policy: keep 24 mo daily + monthly rollup forever (A4 exists; add explicit retention + prune job; `staging_*` pruned aggressively). | P1/P4 |
| O-4 | **Resumable seed** | 36-mo seed can be interrupted (laptop sleep, network). Seed must checkpoint per table-batch via high-water marks and resume where it left off. | P1 |
| O-5 | **Snapshot channel: gzip + HMAC + staging/promote** | Snapshot rows compress well (gzip); sign with the B5 HMAC; route through the A6 staging→promote pipeline so a partial transfer can never corrupt the working DB. | P3 |
| O-6 | **Forecast: port pure fns + tests verbatim, don't "improve"** | The forecast rules are hard-won (calendar-day denominator not sale-day entries; halted lines halve stale 90d; no-history → never auto-suggest; weekday factors; trend guard). Port as pure functions WITH the regression tests. Temptation to "clean up" will silently reintroduce the 2× over-order bug. | P2 |
| O-7 | **Reports on rollups + "as-of" header** | Compute on `sales_weekly/monthly` + covering indexes (A2/A4); every cross-branch report header shows its data "as-of" timestamp — branches have different connector freshness. | P2 |
| O-8 | **Timezone convention now** | NZST/NZDT: store UTC + per-branch offset; render local. The old daily-report pattern (`CONVERT(varchar(10),Logged,120)`) is server-timezone-dependent — decide P0, cheap now, painful later. | P0 |
| O-9 | **Backup before seed/import + retention** | B2 exists; add: automatic backup before any seed/ETL-verify step, and keep-last-N retention. `VACUUM INTO` is atomic-safe on a live DB. | P4 |
| O-10 | **Connector circuit breaker per source** | A5 has timeouts/retry; add per-source breaker (N failures → back off, stop hammering a dead box, surface state in `/api/health`). | P1 |
| O-11 | **UI polish ported, not redesigned** | Carry the proven light-grey theme, colgroup column alignment, segmented button clusters, destructive-action separation, and the stocktake barcode focus rules — the port must not regress the polished UI. | P2 |
| O-12 | **External-edit detection on app-authoritative fields** | Gap in the LWW rule: if someone edits an item directly in Infinity after the app took it over, "app > connector" silently wins and the external change is invisible. Fix: when the connector sees a newer external value for an app-authoritative field → mark `conflict_state = external_edit` and surface in UI, don't just ignore. | P3 |

---

## C. Missing feature gaps

| # | Item | What / why | Priority | Phase |
|---|---|---|---|---|
| G-1 | **`stock_current` table — the #1 gap** | The design has `sales_daily` but **no stock-position table**. Order sheets, sellout, and reports all need **on-hand per branch**, which today comes from the latest `ItemMovement QtyOnHand+Quantity` per branch. Add `stock_current(branch, upc, qty, asof, source)` refreshed every poll (also naturally handles transfers 'T' between branches). Without it, ordering can't compute suggestions. | **Must** | P1 |
| G-2 | **Returns 'Z' in the receipts model** | Net bills-due = goods-in ('G') − returns ('Z'); RTS is TransType 'Z' (positive credit). The schema lists receipts as P/G/I — add 'Z' rows + net-due in AP reconciliation (W3 mentions it verbally; make it a table/report). | Must | P1/P2 |
| G-3 | **Costed stocktake variance + overages** | W7 has result variance; add costed variance per line (book vs counted at UnitCost) and negative shrink (overages) — the proven R8 semantics. Shrink $ reporting is a primary reason for stocktake. | Must | P2 |
| G-4 | **Promo effectiveness report** | The current WebRMS R13 (uplift vs baseline per special) is a key report; the design lists reports generically. Carry it over explicitly over `sales_daily` promo split + `promo_rules`. | Carry | P2 |
| G-5 | **Supplier order confirmation print + CSV** | Done in WebRMS (cf87634); list it explicitly in the ordering port so it isn't lost. | Carry | P2 |
| G-6 | **Stocktake barcode UX rules** | Focus-management rules (barcode mode focus, count-entry return focus) are user-required; port with the module (part of D2 hygiene). | Carry | P2 |
| G-7 | **PO status auto-flip on connector detection** | Keep today's behavior: connector seeing the P row auto-flips `waiting_import → pending_receipt` without manual mark; manual "Mark imported" only covers the import itself. | Carry | P2 |
| G-8 | **Price history alongside cost history** | C2 adds cost history; add `item_price_history` (Price1 at ingest/change) — cheap, powers promo-effectiveness and margin audits. | Nice | P1 |
| G-9 | **Clone history inheritance — cross-feature gap** | After a clone, the new UPC has **zero sales history** → the forecast rule "no history → never auto-suggest" would stop ordering the product. Add a `history_alias` (old UPC → new UPC) so forecast/sellout carry the old item's sales forward (operator-confirmed "same product"). This connects W6 to the ordering engine — the kind of thing a per-feature design misses. | **Must** | P2 |
| G-10 | **Orders lifecycle closure** | Port `cleared_order_ids`: orders need explicit status (open → receipted → cleared) so "active on-order" decrements when receipted — otherwise SOH + on-order double-counts during the P→G window. | Must | P2 |
| G-11 | **Fleet view (optional)** | Installs send heartbeats → HoS shows every install's version + last-sync + snapshot state. Nice ops view; defer. | Defer | P5+ |
| G-12 | **Config pack import/export** | Export a JSON config pack from a working install → apply on a new one (`init --apply-pack`). Matches the ready-to-click onboarding preference. | Defer | P4 |

---

## D. Priority summary

**Must-add for v1 (fold into existing phases, +~2–3 days total):**
G-1 stock_current · G-2 returns 'Z' · G-3 costed variance · G-9 clone history inheritance · G-10 orders lifecycle · O-1 PK strategy · O-8 timezone · O-12 external-edit flag · O-3 retention policy

**Carry-over explicitly (no new cost, prevents regression):** G-4 promo effectiveness · G-5 supplier confirmation · G-6 barcode UX · G-7 PO auto-flip · O-6 forecast tests · O-11 UI polish

**Optimizations (cheap, fold into phase):** O-2 immediate push · O-4 resumable seed · O-5 snapshot gzip/HMAC/staging · O-7 as-of headers · O-9 backup retention · O-10 circuit breaker

**Defer:** G-11 fleet view · G-12 config packs (both P4/P5+)

---

## E. Phase deltas (net effect)

- **P0:** + O-1 (PK strategy), O-8 (timezone) — both schema-level, must be first.
- **P1:** + G-1 (stock_current), G-2 ('Z' in receipts), G-8 (price history), O-4 (resumable seed), O-10 (circuit breaker), O-3 retention scaffolding.
- **P2:** + G-3 (costed variance), G-9 (clone history inheritance), G-10 (orders lifecycle), G-4/G-5/G-6/G-7 carry-overs.
- **P3:** + O-2 (immediate push), O-5 (snapshot hardening), O-12 (external-edit flag).
- **P4:** + G-12 (config packs, optional), O-9 (backup retention).

Net: roughly +2–3 days across P0–P4 on top of the existing plan (which already included +2–3 days from ENHANCEMENTS). The design stays lean; nothing here expands the architecture.
