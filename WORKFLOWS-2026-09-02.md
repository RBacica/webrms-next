# WebRMS-Next — Intended User Workflows (roles × modes × ETL loops)
**Date:** 2026-09-02 · **Companion to:** `DESIGN-2026-09-02.md` (v1.1) + `LIVE-DB-FINDINGS-2026-09-02.md`
**Purpose:** define how staff actually operate the system day-to-day — who sets up, who completes, who monitors, who acts — and how the three ETL integration loops (PO, Item, Stocktake) bridge WebRMS-Next ↔ Live Infinity.

---

## 1. Role × mode capability matrix

| Capability | HoS | Remote-HoS (mode-limited) | BoS | Remote BoS |
|---|---|---|---|---|
| Configure suppliers/terms/modes/promos/scanbacks | ✅ author | ✅ author (pushed up) | ❌ read-only (synced down) | ❌ read-only |
| Create stocktake schedules / order cycles | ✅ | ✅ (pushed up) | ❌ (runs assigned ones) | ❌ |
| Order entry + order sheet | ✅ all branches | ✅ all branches | ✅ own branch | ✅ own branch |
| PO ETL export → import into Infinity | ✅ | ✅ | ✅ (pushed up to HoS tree) | ✅ |
| Monitor incoming POs (status: waiting_import → pending_receipt → receipted) | ✅ full (mark-imported/delete) | ✅ full | 🔍 view-only | 🔍 view-only |
| Item maintenance via ETL (cost/pack/desc) | ✅ | ✅ (pushed up) | ❌ (or submit for HoS) | ❌ |
| Stocktake: in-app counts → .txt/.qry export → Infinity restore | ✅ | ✅ | ✅ | ✅ |
| AP: view invoices, pay (paid ledger) | ✅ | ✅ (pushed up) | ✅ own branch (pushed up) | ✅ |
| Scanback recording (active specials) | ✅ | ✅ | ✅ own branch | ✅ |
| Cross-branch reports / monitor dashboards | ✅ (has all branches locally) | ✅ | ❌ (own branch) | ❌ |
| Sync role | source of truth | client (push-then-pull) | client (pull down, push up) | client |

**Mode gates (from DESIGN):** Standalone = all HoS powers, no connector/sync. Alongside = connector on, ETL export loops active, no direct SQL writes. Migrated = connector off, old system read-only. Remote instances are **limited by mode** — e.g. a Remote-HoS never becomes the sync source; its config writes go UP to the main HoS.

---

## 1a. Authority model — "App-authoritative when configured" (user decision 2026-09-02)

**Principle:** the app pulls from Infinity by default, but wherever the app can do better or take over, **once configured/edited in the app, the app is authoritative for that entity/field**. Everything else stays a live pull.

| Domain | Not configured in app | Configured in app → app authoritative |
|---|---|---|
| Suppliers/terms/modes | seed defaults from live `ReorderParameters` + `Customers.Disc*` | app-authored `supplier_terms` / `supplier_modes` (HoS) |
| Promos | connector pulls RBP/Specials rules | app-authored promo rules / scanback contracts win |
| Items (master) | connector pulls item master | an item **edited or cloned in-app** becomes app-authoritative for the changed fields until verified into Infinity (W6) |
| Stocktake schedules | mirror Infinity's 15 native schedules | app-managed schedules (HoS) supersede for assigned runs |
| Orders / incoming-PO / paid / audit | — | always app (already) |
| Scanbacks/rebates | — | app-only (already) |

**How it works mechanically (extends the existing provenance model):**
- Rows keep `source` (connector vs app) + `last_synced_at`. Add an **`app_override` marker / `effective_value`** per app-managed field: display and reports use the app value when present, else the connector value.
- The connector **never overwrites an app-authoritative field** — it pulls, then the merge keeps the app value (LWW: app > connector for overridden fields). This is the existing modes.json override pattern generalized to items/promos/schedules.
- An app-authoritative item edit stays flagged **`pending_import`** until the ETL file is imported into Infinity and the connector pull-back confirms it — at which point app and Infinity agree and the flag clears.
- Config-class app data replicates HoS→branches as always; the authority model is per-install + replicated.

**Result:** Infinity remains the default source of truth for anything the app hasn't been configured to own; the app takes over exactly the pieces the operator set up — smooth migration, no big-bang cutover.

---

## 2. Workflow W1 — HoS setup, config & scheduling

**Actor:** HoS operator (or Remote-HoS, edits pushed up).

1. `init` on first run → app DB + schema + role config.
2. Configure suppliers (terms, modes, lead/cycle seeded from live `ReorderParameters`), departments, promos (RBP/Specials rules pulled by connector), scanback contracts.
3. Create **stocktake schedules** — aligned with Infinity's own (`StocktakeSchedules` live rows: "Daily Smokes", "Daily Risk", "Weekly Beer", "Spirits Part 01–03", "Wines Part 01–03"… each `StockTakeScheduleDetails` Type='D' = dept). HoS defines who does what when; schedules replicate down to branches.
4. Set order cycles per supplier (weekly/monthly, lead/cycle).
5. Everything is config-class → **outbox-replicated down** to branches automatically.
6. **Install the service (Windows):** `webrms-next service install` registers boot-start + crash recovery — the box runs **headless** and self-heals (kills → SCM restart, power-loss → auto-start, DB quick_check + outbox replay on boot). Full design: ENHANCEMENTS B6.

**Design additions needed:**
- `stocktake_schedules` table mirroring Infinity's shape (`description, dept, day_of_week, branch_scope`) + a UI to manage them. Sync down with config.
- Order-cycle config already covered by `supplier_modes` (from `ReorderParameters`).

---

## 3. Workflow W2 — Branch daily operations (complete & report)

**Actor:** Branch staff on a BoS instance.

1. **Ordering:** open order sheet for own branch → suggested qtys (forecast) → adjust → place order → PO ETL `.xlsx` generated → pushed UP to HoS incoming-PO tree → tracked.
2. **Stocktake:** run the assigned schedule (or ad-hoc) → scan/search/count in-app (SOH vs counted, variance) → export `.txt` count file + `.qry` ticket file → **Infinity stocktake program Save/Restore** → counts post to Infinity → results flow back (see W7).
3. **Scanbacks:** on Active Specials, record scanback amounts received (expected vs received visible).
4. **AP:** view own-branch invoices, mark paid (pushed up to HoS ledger).
5. Everything the branch does either stays local or pushes up via outbox — HoS sees it without asking.

---

## 4. Workflow W3 — HoS monitor, complete & act

**Actor:** HoS operator (the "monitors / completes / acts directly" role).

1. **Incoming POs dashboard** (all branches): status per file — `unknown` → `waiting_import` → `pending_receipt` (P row exists) → `receipted` (P+G). HoS **imports the file into Live Infinity ETL** (the file lives in the HoS incoming tree; operator or scheduled job runs Infinity's ETL), then **marks imported**, or deletes.
2. **Cross-branch reports** (now local queries — C1): stock, sellout, GP% (incl. scanback-corrected), dept weekly, top movers across all stores.
3. **AP:** pay supplier invoices, monitor bills due (net = goods-in − returns), reconcile with APInv/APPayment pulled data.
4. **Scanback confirmation report:** expected vs received per supplier/special across branches; chase outstanding.
5. **Item maintenance** via ETL (W6) — approve/apply cost changes, price lists, and clone/barcode-change requests (BoS suggest → HoS approve).
6. **Alerts** (deferred C5): low stock / overdue POs / unreconciled scanbacks — surfaced on the dashboard.

---

## 5. Workflow W4 — Remote work (mode-limited)

**Actor:** HoS operator offsite (laptop, Remote-HoS) or a branch's remote view.

- **Remote-HoS workstation:** full authoring/action powers (config, orders, stocktake, AP, item ETL) — every write **pushes up to the main HoS** immediately (offline → pending-replay on next sync). Reads come from its own DB via connector (direct AKPOS over Tailscale) with **HoS snapshot fallback** when the DB is unreachable. Cannot act as sync source (mode gate).
- **Remote BoS view:** read-only dashboards + own-branch ordering/stocktake/scanbacks, synced through the HoS (no direct AKPOS needed — snapshot channel covers it).
- Mode rules are enforced server-side (BoS 403 author-gate pattern from WebRMS b285e95 generalizes).

---

## 6. Workflow W5 — PO ETL loop (orders in-app → Live Infinity → tracked)

**Already in design; formalized here.**

1. Branch/HoS places order in-app → `build_purchase_order_xlsx` (compact H/D blocks: `POID,H,Supplier,BranchDestination,AuthoriseBy,BillOfLading,EstArrival,ExtReference,FC,FCRate` + `POID,D,UPC,Quantity,UnitCost,Tax,SupplierProdCode,PurchaseUnit,PurchaseQty,FCCost`).
2. File lands in `incoming-po/<branch>/` on the **HoS** (BoS/remote pushes up; HoS tracks locally). BillOfLading recorded in sidecar/`incoming_pos` table.
3. **Operator imports the .xlsx into Live Infinity** (ETL program reads it). **P** (purchase) row appears → status flips `waiting_import → pending_receipt`.
4. Goods received → **G** row linked via OriginatingTransNo → `receipted`. Connector pulls receipts → app reflects the full trail.
5. HoS can mark-imported / delete / (P2) **edit un-imported lines** (qty/cost/remove) — regenerates the ETL file preserving POID + BillOfLading.
6. **Orders lifecycle (G-10):** a receipted order (P→G complete) is moved to `cleared_order_ids` — removed from active on-order so SOH + on-order never double-count during the P→G window. The order sheet's on-order figure is net of cleared orders, exactly as WebRMS does today.

**Status model is the tracking mechanism** — the app never writes to Infinity for POs; it generates the file Infinity imports and reads the result. Same pattern for Item and Stocktake below.

---

## 7. Workflow W6 — Item maintenance via ETL (user decisions 2026-09-02 folded in)

**Goal:** maintain item data in-app and land it in Live Infinity through the manual ETL import path — instead of hand-editing Infinity. **Manual ETL import is the model** (same reason the incoming-PO tracking exists): the app generates the file, an operator imports it into Infinity's ETL, the app tracks and verifies the result.

**Live mechanism (verified):** Item ETL export = `Item-<timestamp>.xlsx`, single sheet `Infinity ETL`, ~98 columns (Product code, SKU, Description, Department/SubDepartment/Class, Price1–8, Supplier, Cost/CostAve/Pack Cost, InActive, Non-Stock, Tax, Pack Size, Volume…). Round-trip: patch rows → reimport → Infinity updates.

**Authority:** item master is connector-pulled by default (1a); an item **edited or cloned in-app** becomes app-authoritative for the changed fields until the ETL import + connector pull-back verifies it.

**Roles (user decision):** **HoS edits and exports.** **BoS can suggest → HoS approves and exports.** (BoS never exports item ETL directly.)

**The clone-item operation — barcode/UPC change (user's worked example):**
When a product's barcode/UPC changes (supplier re-label):

1. In the app, open the item and choose **"Clone to new UPC"**; enter the current/new UPC.
2. The app creates a **new item row** cloning the old one: description, dept, sub-dept, class, supplier, cost/cost-ave/pack cost, price1–8, tax, pack size — with **UPC = the new UPC**, SKU = the new SKU.
3. The new item's **alternate barcode is set to the OLD item's UPC** — so the old barcode still scans to the new item at the till (ItemBarcodes alt-barcode lookup).
4. The **old item** is kept as a historical record: **SKU renamed to `OLD_<new_upc>`** and marked **InActive** — it stops appearing in ordering/stocktake/promo as its own product, but its history (sales, movement, costs) stays intact and attributable.
5. The app generates a **differential Item ETL `.xlsx`** containing both rows: the new item (insert) and the old item (SKU change + InActive flag).
6. HoS operator imports the file into Infinity ETL (manual) → app flips status `exported → imported` → **connector pull-back verifies** (new UPC present; old item SKU = `OLD_<new_upc>`, InActive; alt barcode on new item = old UPC) → status `verified`. Until verified, the clone is flagged `pending_import`.
7. **History inheritance (G-9):** the clone records a **`history_alias` (old UPC → new UPC)** so the ordering forecast and sellout treat the new item as the same product — otherwise the rule "no history → never auto-suggest" would stop ordering it entirely. Sales history carries forward until the old item's windows age out (operator-confirmed mapping; the old item stays in `stock_current` until its stock is exhausted/transferred).

**Suggestion flow (BoS → HoS):**
1. A branch spots a needed item change (barcode change, cost correction) and creates an **item change request** (type: `edit` | `clone` | `price_list`) in-app → status `suggested`, pushed up to HoS.
2. HoS reviews → **approve** (applies the edit as app-authoritative, generates the differential ETL export, imports manually, verifies) or **reject** (with note back to the branch).

**Design additions:** `item_change_requests` table (type, status `suggested → approved → exported → imported → verified | rejected`, payload JSON, requested_by/approved_by, source_branch), `item_etl_exports` table (tracking + verify diff), differential 98-col item ETL writer, clone-item logic (new row + old-row SKU/InActive + alt-barcode wiring).

---

## 8. Workflow W7 — Stocktake Save/Restore loop (in-app counts → Infinity)

**Live mechanism (verified):** in-app stocktake exports two files Infinity's stocktake program reads via its **Save/Restore** function:
- `.txt` count file — lines `0,<UPC>,<COUNT>,` (4dp) → restores counted qtys.
- `.qry` ticket file — `[Header]/Application=LabelQuery/SaveFileVersion=3`, CriteriaCount blocks, CopiesException when qty>1 → restores label-query tickets.
Infinity posts the count as SMHeaders `S` rows (zeroing sheet + count sheet pair) → ItemMovement updates.

**WebRMS-Next loop:**
1. HoS assigns schedules (W1); branch opens the assigned stocktake in-app.
2. Staff count (scan/barcode/search; SOH vs counted, variance live).
3. Export `.txt` + `.qry` → operator runs Infinity stocktake program → **Restore** the files → counts post to Infinity.
4. Connector pulls the SMHeaders `S` rows + ItemMovement → app marks the stocktake **imported** and shows the **costed result variance** (book vs counted per line at UnitCost, incl. **overages** — negative shrink) and the shrink report (G-3).
5. Status per stocktake: `in_progress → exported → restored/imported → verified`.

**Design additions:** `stocktake_runs` table (id, schedule, branch, dates, status), export already ported from WebRMS module, import-status + result pull-back via connector, shrink report over posted results.

---

## 9. What's already in the design vs what's new

| Workflow | Already designed | New additions |
|---|---|---|
| W1 HoS config/schedule | config-class replication, supplier_modes, orders | `stocktake_schedules` table + UI (mirror Infinity) |
| W2 Branch ops | ordering, stocktake, scanbacks, AP | — (P2 ports) |
| W3 HoS monitor/act | incoming-PO dashboard, cross-branch reports, AP, scanback confirm | item-maintenance approve step (W6) |
| W4 Remote | Remote-HoS write-through, snapshot fallback, mode gates | — |
| W5 PO ETL | ✅ fully (incoming_pos, status model, edit at P2) | — |
| W6 Item ETL | ❌ | item-edit + **clone/barcode-change UI**, differential 98-col ETL export, **item_change_requests (BoS suggest → HoS approve)**, import tracking + verify diff |
| W7 Stocktake | export ported; status ❌ | `stocktake_runs` status, restore→imported tracking, result variance/shrink pull-back |

---

## 10. Plan (phase placement)

- **P0:** schema additions — `stocktake_schedules`, `stocktake_runs`, `item_etl_exports`, `item_change_requests`, **`stock_current`, `item_cost/price_history`, `history_alias`** (+ authority/`app_override` markers per 1a; **PK strategy + timezone per O-1/O-8**) (status tables, no UI yet).
- **P1:** connector pulls `StocktakeSchedules`/`StockTakeScheduleDetails`/`StockCountLocations` (seeds W1) + **`stock_current` refresh + receipts incl. 'Z' returns** + item pull-back diff plumbing for W6/W7 verify + authority-merge rule (connector never overwrites app-authoritative fields) + resumable seed (O-4) + circuit breaker (O-10).
- **P2:** UI — stocktake schedule manager (W1), item-edit + **clone-to-new-UPC (with `history_alias` inheritance, G-9)** + BoS-suggest/HoS-approve + differential ETL export + import tracking (W6), stocktake run status + **costed variance/overages (G-3)**, **orders lifecycle `cleared_order_ids` (G-10)**. PO edit already P2. Carry-overs: promo-effectiveness (G-4), supplier confirmation (G-5), barcode UX (G-6), PO auto-flip (G-7), forecast tests verbatim (O-6), UI polish (O-11).
- **P3:** replication of the new config-class tables (`stocktake_schedules`, item change requests + exports pushed up) + snapshot channel carries their data + **immediate push (O-2)** + snapshot gzip/HMAC/staging (O-5) + external-edit flag (O-12).
- **P4:** parity-report extends to item changes (before/after diff incl. clone results) and stocktake results (counts posted vs exported) + config packs (G-12) + backup retention (O-9).

## 11. Decisions & open items (2026-09-02 round folded in)

**Resolved (user decisions):**
1. **ETL imports are manual** — confirmed; that is exactly why the incoming-PO tracking exists. The app generates files + tracks status; an operator runs Infinity's ETL to import. No automation of the desktop import (app-side "Mark imported" is only the status flip).
2. **Item maintenance scope: full item record incl. identity fields** — the clone/barcode-change operation (UPC, SKU, alt barcode, InActive) is the reference case; cost/pack/description/price edits are the simpler subset. Differential 98-col export carries only changed rows.
3. **Stocktake schedules: app-authoritative when configured** (per 1a) — mirror Infinity's 15 native schedules by default; if HoS sets up schedules in the app, the app's are authoritative for assigned runs.
4. **Branch item edits: BoS suggests → HoS approves and exports** — BoS never exports item ETL directly.
5. **Gap analysis folded in (2026-09-02, user-approved):** G-1 `stock_current`, G-2 returns 'Z', G-3 costed variance, G-9 clone `history_alias` inheritance, G-10 orders lifecycle, O-1 PK strategy, O-3 retention, O-8 timezone, O-12 external-edit flag — all now in §10 and DESIGN §3/§8; carry-overs (G-4–G-7, O-6, O-11) are default-in. Source: `REVIEW-GAP-ANALYSIS-2026-09-02.md`.

**Still open (minor):**
- Clone semantics detail: new item's SKU — auto-generate from new UPC vs let HoS set it? (Default: HoS sets; app suggests `<brand>-<new_upc>`.)
- Old item after clone: InActive (recommended — stops it selling/ordering; history intact) vs keep active with SKU `OLD_<new_upc>` only. (Default: InActive.)
- Whether the app should also expose "clone" as the *only* way to change a UPC (recommended — UPC is the PK; direct re-key breaks history linkage) vs allow direct UPC edits on top of clone.

**Estimated additions:** +1 day P1 (authority-merge rule + clone logic), +1 day P2 (item edit/clone UI + suggest/approve + item ETL tracking) — on top of the workflow plan already in §10.
