# WebRMS-Next — Cleanup, Gap-Close & Optimization Plan (2026-09-05)
Review output → planned actions. Companion: `PARITY-VS-WEBRMS-2026-09-05.md` (feature
coverage) and the final `CLEANUP-REPORT-2026-09-05.md` (post-execution evidence).

## Review findings

### Hot-path performance (measured by code inspection)
- **[P1] `order_sheet` N+1** — per sheet LINE it issues ~4 queries: `stock_current`
  (branch/all), `active_on_order` (order_lines×orders), `product_history` ×2
  (sales_daily 92d + promo_rules windows). A 450-line supplier (010) ⇒ ~1,800
  round-trips per sheet load. Reports are single-query (verified: dept_sales/daily/
  movers/dept-weekly/stock/receipts all one GROUP BY statement each) — no action there.
- **[P1-correction] all-branch history bug** — `order_sheet` with `branch=None` (HoS
  "All branches") calls `product_history(pool, 0, upc)`; branch 0 matches no
  `sales_daily` rows ⇒ **every forecast on an all-branch sheet has zero sales
  history** (silent wrong suggestions). Batch refactor must aggregate sales across
  branches for branch=None.
- **[P2] overview stock cost correlated subquery** — per-UPC `(SELECT cost FROM items
  …)` inside the stock subquery; one statement but replaceable with a JOIN.

### Cleanup
- **[P1] view timer leak** — `incoming.js` `setInterval(load, 60000)` is never cleared
  when the user navigates away; each Incoming-PO visit stacks another 60s poller.
- **[P2] missing README** — repo has 12 design/plan docs and no index; add a README
  (what/why/commands/doc map) so an operator or future session isn't lost.
- Repo hygiene otherwise clean: `.gitignore` correct (target/data/config.toml/run-*/zips),
  no untracked files, tree clean.

### Gaps from the parity report — closure decision
| Gap | Decision this pass | Why |
|---|---|---|
| Stocktake count-session UI (search/barcode/count grid/Save→.txt+.qry) | **CLOSE** — compact port over the existing endpoints | Daily-driver feature; backend already complete incl. run recording |
| Payment Mix + Hourly reports | **DEFER** (documented) | Needs new `sales_payment`/`sales_hourly` rollups + connector pulls (new data model, ~a day incl. migration + ingest + 2 report endpoints + UI) — separate feature batch, not cleanup |
| Replacement-report (ordering) | **DEFER** | Niche diagnostic; sheet already surfaces `possible_replacement` |
| Item edit/clone ETL (W6) + O-12 | **DEFER** | Requires the app_overrides/item-change schema build-out (documented in DESIGN follow-ups) |
| Costed stocktake variance (G-3) | **DEFER** | Depends on stocktake run→book reconciliation design |

## Actions
1. **A1** — Rewrite `order_sheet` to batched loads: one stock query (branch-scoped or
   latest-across-branches), one on-order GROUP BY (branch-scoped), one sales_daily
   92-day query for the whole supplier (branch-scoped; branch=None ⇒ SUM across
   branches — fixes the all-branch empty-history bug), one promo-window query; build
   per-UPC maps, then the existing pure forecast loop. ~1,800 → ~5 statements.
2. **A2** — overview stock: correlated cost subquery → LEFT JOIN items.
3. **A3** — stocktake count-session UI: dept/supplier linked filters, search
   (UPC/description/barcode), barcode quick-add, count grid w/ SOH+variance, Save →
   POST /api/stocktake/export (server + client download), run recorded.
4. **A4** — view timer hygiene: `#view` element carries `_timers`; `app.js route()`
   clears before each view swap; `incoming.js` registers its poll interval there.
5. **A5** — README.md index + doc map.
6. **Verify** — `cargo test` (expect ≥83), live sheet-load timing before/after on the
   seeded cluster (supplier 010 ~450 lines), endpoint sweep, then report.
