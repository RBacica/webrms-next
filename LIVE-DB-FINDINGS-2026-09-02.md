# WebRMS-Next — Live HoS DB Deep-Dive + Scanback/Rebate Tracking (app-only feature)
**Date:** 2026-09-02 · **Companion to:** `DESIGN-2026-09-02.md` (v1.1) + `ENHANCEMENTS-2026-09-02.md`
**Source probed live:** gg-core-hos `100.71.113.111` AKPOS (read-only `InfinityRO`), 2026-09-02. 243 tables inventoried; promo/rebate candidates + sales schema verified with real data.
**User decision (2026-09-02):** Scanback/Rebate tracking is an **app-only feature** — not based on the live DB. It must work standalone, in alongside mode, and post-migration. Live-DB tables are context/reference only; nothing is pulled for this feature.

---

## 1. Scanback/Rebate Tracking — app-only design

### 1.1 What the feature is (app-native, mode-independent)

- **Contracts are authored in the app** — operator enters supplier, scope (UPC / group / set / dept / supplier-wide), period, and rebate terms. No live-DB source.
- **Expectation** = computed from **the app's own data**: `promo_rules` (already pulled into the app DB by the connector) + `sales_daily` S/N line splits (also app-owned). In standalone mode with no connector, expectations are manual or from locally-entered promo/sales data. This keeps the feature fully functional in every mode.
- **Receipts are entered in the app** — `rebate_ledger`: what was actually received, when, by whom.
- **Report** = expected vs received per supplier/contract; outstanding; GP% correction.

### 1.2 New tables (app schema additions to DESIGN §3)

```
rebate_contracts            -- app-authored contract (mirrors the shape Infinity uses in
  id, description, branch_scope, supplier_code,     its CostProtection table, but is OUR data)
  rebate_type: 'scanback'|'supplier',
  basis: 'fixed_per_unit'|'pct_of_sell'|'price_delta'|'margin_protect',
  amount, rate_pct,
  special_id,               -- NEW: link to a specific active special (promo_rules.id)
                            -- scanbacks are anchored to the Active Specials view; NULL for supplier/dept rebates
  sell_from, sell_to, receipt_from, receipt_to, max_qty, is_active,
  created_by, created_at, updated_at            (provenance + audit)

rebate_contract_lines       -- the "nested product" scope
  id, contract_id, scope_type: 'upc'|'group'|'set'|'dept'|'supplier_all',
  scope_key,                                     (upc / app group id / set id / dept id)
  amount_override, rate_override

rebate_ledger               -- actual amounts received (recorded from the Active Specials view)
  id, contract_id, special_id, branch, supplier_code, invoice_or_ref, received_amount,
  received_date, received_by, note, created_at

sales_daily ADD            -- GP% accuracy columns (from connector ingest of TransLines)
  promo_price, promo_units, normal_units, cost_amount, line_margin
  (split S vs N lines per item/day; margin = Σ(price−cost)×qty per bucket)
```

### 1.3 How it fits the architecture

- **Config-class data, not transactional:** contracts + ledger replicate like `supplier_terms`/`paid_ledger` — HoS-authored, outbox-replicated down (Remote-HoS can author → pushed up); BoS readers. Same ownership rules as shared-settings (b285e95 model). Works identically in all three modes.
- **Expected scanback derives from app data only:** app's `promo_rules` (RBP prices or Specials) × `sales_daily` S-line units. No live-DB query at request time, no connector dependency.
- **Reports:**
  - *Scanback confirmation* — expected vs received per supplier/contract/period.
  - *GP% corrected* — margin per line already excludes promo discount; adding received scanbacks restores true net margin:
    `true GP% = (net_sales + scanback_received − cogs) / (net_sales + scanback_received)`.
- **Expansion to supplier rebates:** same tables, `rebate_type='supplier'`, scope by UPC range/group/dept/supplier; expectation = rate × normal units/$.
- **NOT pulled from live DB:** `CostProtection`, `RptTransPromo`, `TransLines.CostProtect` — reference for schema shape only (and empty on new-gen anyway). The app owns this feature end-to-end.

### 1.3a Scanbacks anchored to the Active Specials view (user note 2026-09-02)

The scanback entry point is the **Active Specials list itself** — per special, not a separate standalone screen:

- **Active Specials view gains a scanback column/panel:** for each active special row (and its nested products — the set/group members of a `promo_rules` entry), show expected vs received vs outstanding scanback amounts.
- **Record-in-place:** a compact "Record scanback" input/action right on the special's row → writes a `rebate_ledger` row linked via `special_id`. No jumping between screens to reconcile promos against rebate money.
- **Correlated by construction:** the ledger row carries the special id, so the confirmation report can join `promo_rules` (what was on special, at what price) → `sales_daily` (units sold at S price) → `rebate_ledger` (what came back) per special. The correlation is structural, not inferred from descriptions.
- **Nested products:** the special's scope (RBP set/group UPCs, or Specials product list) expands into one expected amount per member product from `sales_daily` S-units × price delta; the record action can be per-special (aggregate) or per-product (drill-down), both writing the same `special_id`-linked ledger rows.
- **Supplier/dept-level rebates** (no special) use the same tables with `special_id = NULL` and scope via `rebate_contract_lines`.

### 1.4 audit_log (approved — fold in now)

```
audit_log
  id, ts, operator,          -- operator = B_Users login/name via connector
  action, entity_type, entity_key,
  before_json, after_json, origin_install, outbox_id
```
Writes to replicated tables append an audit row in the same transaction as the outbox row. Operator name from the live `B_Users` (code + name, e.g. '012' Grace) mapped into `users` on ingest — **only used as a label**; the audit feature itself works with single-user too (operator = default/unknown when unauthenticated).

---

## 2. Other live-DB data worth pulling (verified useful — this is the connector scope)

| # | Table | What it adds | Phase |
|---|---|---|---|
| 1 | `Branches` | Full store detail incl. addresses, GST no, phone, names (9 stores: 1 HoS + 8 BoS) — better than bare IDs; UI + migration docs | P1 |
| 2 | `B_Users` | Operator codes + names (Rachel/Harley/Grace/Blake/Jamie…) — feeds `audit_log.operator` label + staff-activity views | P1 |
| 3 | `ReorderParameters` | Per-supplier `CoreOrderOption` lead/cycle (e.g. 010–015 → 7/3; 011 branch 17 → 7/5) — **the live system's own ordering config**; seed supplier modes from it instead of hand-entry | P1 |
| 4 | `Customers` `DiscGroup/DiscPercent/DiscDays` | Supplier terms (discount group/%, days) — seed `supplier_terms` (010 ARG, 011 Lion NZ, 012 Asahi…) | P1 |
| 5 | `Departments` + `SubDepartments` + `Classes` | New-gen dept IDs 10–70 (Beer 10 … Wine 70, each 25% target) + sub-dept taxonomy — ordering/reports dimensions | P1 |
| 6 | `Taxes` | GST 15% / non-taxable config — tax math source instead of hardcoding | P1 |
| 7 | `APInv` + `APPayment` | Full money view: `InvoiceAmount, PaidAmount, DiscountAmount/PC, DueDate, PONumber, Freight, TaxAmount1…` — richer payables than today (paid amounts from `APPayment.ChequeAmount`, not just our paid.json) | P2 |
| 8 | `TransPayments` + `MediaLogged` | Payment media per txn — daily takings reconciliation, EFTPOS/cash split (proven pattern in akpos-reports) | P2 |
| 9 | `DataChangesUp/Down` | **The live system's own sync mechanism** (field-level change rows, 31 cols) — a reference model for our outbox; study only, not a data source | P3 (study) |
| 10 | `EventLog` | 257k rows; ops 1010/1011 (item sold w/ price+qty), 1025, 1255, 30162/30165 — alternative sale-stream + audit source; optional (only if TransLines insufficient) | defer |
| 11 | `Config` | Engine flags incl. `RBP logging type`, `Item movement history` — tells us per-box engine/mode without probing code | P1 (engine detect) |
| 12 | `NotFoundUPC` | Unknown-UPC log — barcode/scan hygiene stats | optional |

### Verified-empty on new-gen (don't waste connector time): `CostProtection`, `PricingBonus`, `RptTransPromo`, `RptTransNotesPromo`, `Commissions`, `ManualDiscounts`, `ItemPriceChanges`, `DataChangesUp/Down`, `GLHeaders/Lines`, `BranchPORatios`, `KitSets`, `BranchStockLink`, `OrderAllocations*`, `PricingScale`. Several become **populated** on the Standard-engine old HoS (`Specials`, `RptTransPromo`) → connector must be engine-aware per box (the RBP-vs-Specials rule already in the skill). **None of these feed the scanback feature.**

---

## 3. Open items for review

1. **Expectation basis:** computed from app's own `promo_rules` × `sales_daily` S-units (recommended — zero double-entry) vs manual per-contract rate. Both supported by `basis`; scanbacks default to computed, supplier rebates to manual. **Anchored to the Active Specials view** (1.3a) either way.
2. **Supplier rebate scope:** per-UPC/group/set/dept/supplier — confirm dept-level is wanted in v1 (your expansion path).
3. **audit_log operator:** seed from B_Users as a label now; simple auth stays deferred (single-user default = the seeded default user).
4. **sales_daily S/N split:** confirm the connector should carry `promo_units`/`normal_units` + `line_margin` from day one (needed for both GP% accuracy and computed scanback expectations).
5. **Record granularity on Active Specials:** per-special aggregate (recommended v1) vs per-product drill-down within a special — the schema supports both; UI starts aggregate.

**Estimated additions:** schema + Active-Specials scanback panel + confirmation report ≈ 1–1.5 days (P1 schema, P2 UI/report; anchoring to the existing specials view is cheaper than a standalone screen); audit_log ≈ 0.5 day P0.
