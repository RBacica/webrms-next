# WebRMS vs WebRMS-Next — Full Project Comparison (2026-09-05)

**Compared:** `~/Projects/webrms-sql-infinityrms` (OLD, actix-web, live-DB, HEAD `b285e95`) vs
`~/Projects/webrms-next` (NEXT, axum + SQLite, HEAD `a9ec0c2`). Both on `main`.
Method: repo inspection (git/`cargo test`/route extraction/LOC), live cluster state, module-by-module diff.

---

## 1. Executive Summary

| Dimension | OLD WebRMS | WebRMS-Next | Verdict |
|---|---|---|---|
| Architecture | Direct live-DB reads (deadpool-tiberius) + JSON sidecars | Local SQLite materialization + connector pulls | NEXT (design goal) |
| Data safety | None (reads live; writes via ETL files) | Backup/parity/doctor + snapshot fallback | NEXT |
| Replication | Plain-HTTP file sync (ureq), no signing/fallback | Outbox + gzip/HMAC snapshot + auto-fallback | NEXT |
| Feature coverage (backend) | Full | **~95%** (2 old endpoints missing) | OLD ~edge |
| UI richness | 4,829 JS lines, 6 views, heavy per-view tooling | 1,370 JS lines, 8 views, lean | **OLD (big gap)** |
| Tests | 323 passing (unit-heavy) | 86 (51 lib + 35 HTTP integration) | OLD by count; NEXT by integration depth |
| Ops tooling | launch script, in-app SQL settings, Windows exe | doctor/backup/parity/seed CLI, Windows service | NEXT |
| Release state | Shipped (Windows exe, zips, 4 test instances live) | Pre-release: P5 zips + B6 Windows live NOT done (held) | OLD |
| Docs | 30+ dated plan docs, no index | 11 coherent docs (README index) | NEXT |

**Bottom line:** the backends are now ~feature-parity (NEXT even exceeds OLD in items maintenance, resilience, and ops tooling), but the **OLD web UI is substantially richer** — roughly 3.5× the JS, with per-view toolbars, keyboard nav, print/CSV exports, expandable rows, a dept filter, live-refresh, and a settings page that NEXT hasn't ported. NEXT's next milestone is UI depth, not backend.

---

## 2. Architecture

### OLD — direct-to-live-DB webapp
- actix-web 4 + deadpool-tiberius connection pool → **every request queries the live AKPOS SQL Server** (TransLines, TransHeaders, ItemMovement, SMHeaders, Items, PricingSequence…).
- Local persistent state = **JSON/TOML sidecars** in `data/`: `orders.json`, `paid.json`, `suppliers-config.toml`, `modes.json`, `settings.json`, `db.toml` (SQL overrides), `save-mode.json`.
- Write flows are **file-based**: ordering → ETL `PurchaseOrder-*.xlsx` for Infinity to import; stocktake → `.txt`/`.qry` exports; payables → ledger sidecar.
- Single binary, foreground `run()`, Windows convenience (`pause_before_exit`), in-app **Settings page edits the SQL connection live** (writes `db.toml` overrides, tests + reconnects without restart).

### NEXT — local-first with connector
- axum 0.8 + sqlx SQLite: **27 local tables across 14 migrations**; all queries hit the local DB.
- tiberius **connector pulls** reference data incrementally (high-water marks, keyset pagination, circuit breaker): branches/departments/suppliers/items (incl. disc_group)/stock/sales_daily/receipts/AP/promos/RBP/pricing sets+groups/sales_payment/sales_hourly/sales_basket_dept/sales_basket_band/voids_daily.
- App-authored tables replicated via **outbox** (`orders`, `paid_ledger`, `app_overrides`, `incoming_pos`, `rebate_ledger`, `stocktake_*`, `item_change_requests`, `item_etl_exports`, `history_alias`, `audit_log`).
- Three modes (standalone / alongside / migrated), CLI (`run|init|service|seed|doctor|backup|parity`), tracing, Windows SCM service entry (compile-verified).

---

## 3. Feature / Route Surface

### Module coverage — parity achieved
Both implement ordering, stocktake, payables, promotions, reports, and incoming-PO.
NEXT renamed a few: `payables/supplier-config` → `payables/config`, `ordering/config`+`global` → `ordering/settings`+`modes`, and added `ordering/confirmation-csv`.

### OLD-only (NEXT missing)
| Route | What it is | Impact |
|---|---|---|
| `/api/settings`, `/api/settings/database(+test)`, `/api/settings/save-mode` | In-app SQL connection editor + save-destination chooser | Operator convenience; NEXT is config-file + CLI only |
| `/api/reports/departments` | Dept list → Reports **dept filter dropdown** | Small UI gap (NEXT reports has no dept filter) |
| `/api/reports/specials` | Promo-window analysis (R13, baseline-matched) | Covered by NEXT's `/api/promotions/effectiveness` (ported verbatim) — same numbers, different route |

### NEXT-only (genuinely new)
- `/api/items/*` — **W6 items module** (search w/ 7 filters + facets, edit, clone w/ SKU=OLD_ convention + history_alias, Item-ETL patch export + download). OLD has no items maintenance at all.
- `/api/health`, `/api/version` — freshness/role/fallback + repl lag.
- `/api/sync/outbox`, `/api/sync/snapshot`, `/api/sync/status`, `/api/sync/up` — outbox replication + signed snapshot.
- `/api/reports/promo-summary`, `/api/ordering/confirmation-csv`.
- CLI: `doctor`, `backup`, `parity`, `seed`, `service`.

---

## 4. UI — the decisive gap (OLD wins)

| View | OLD (lines) | NEXT (lines) | What OLD has that NEXT doesn't |
|---|---|---|---|
| ordering.js | 1062 | 219 | keyboard nav + custom steppers, active-only on-order filter, expandable overrides, confirmation print w/ branch details, per-supplier settings UI |
| reports.js | 1094 | 208 | 8 report pages + **specials page** + **dept filter** + print/CSV on every page + ● Live auto-refresh toggle + expandable mover children |
| stocktake.js | 838 | 208 | filter toolbar (dept/supplier/uncounted), save-destination chooser, session survives navigation |
| payables.js | 734 | 83 | per-supplier config UI, TSV export, pay modal, paid-ledger view |
| promotions.js | 368 | 73 | expandable special rows, per-product scanback drill |
| incoming.js | 317 | 51 | grouped PO action buttons, status tags |
| overview (reports) | — | ~260 (just ported) | NEXT now matches the 10-block landing |

NEXT's UI is functional and self-contained (ES modules, no globals) but lean. The reports dept filter, ordering keyboard nav, payables export, stocktake toolbar breadth, and the settings page are the concrete porting backlog. Note NEXT added Items (128) + full Overview — two things OLD lacks or had only in reports.

---

## 5. Replication & Resilience (NEXT wins decisively)

| | OLD | NEXT |
|---|---|---|
| Transport | ureq plain HTTP (no TLS) | reqwest rustls |
| Down-sync | file copies: suppliers-config.toml/modes.json/settings.json/paid.json | outbox rows (config_down) + fanout via HoS relay |
| Up-sync | orders.json merge + ETL PO upload | outbox push w/ origin_install routing |
| Incoming-PO | list/imported/delete on HoS | auto-flip lifecycle (waiting_import→pending_receipt→receipted) + delete |
| Snapshot | none | gzip + HMAC-SHA256, staged atomic restore |
| Fallback | none (branch dies if HoS unreachable) | auto-engage after 3 dead ticks, pre-restore backup, local-wins re-import, auto-recovery |
| Watermarks | none | per-table high-water, resumable seed, lag metrics |
| Health | /api/mode only | /api/health (connector age, repl lag, fallback state, role) |
| Data gate | none | `parity` (9 tables zero-diff), `doctor`, `backup --keep N` |

---

## 6. Testing & Quality

- **OLD: 323 tests passing** — deep unit coverage (forecast engine port w/ 27 tests, R13 promo measurements, ETL xlsx verified via calamine, overrides, paid ledger, sync merge). Mostly unit-level; fewer end-to-end HTTP tests.
- **NEXT: 86 tests (51 lib + 35 integration in `tests/smoke.rs`)** — integration tests boot the real axum router against a temp SQLite and exercise HTTP round-trips (order → ETL → receipts flip, fallback engage/recover, clone/O-12, payment/hourly, etc.). 0 warnings; `x86_64-pc-windows-gnu` check clean.
- OLD repo state: **1 unpushed commit (`b285e95` shared-settings) + modified `launch-test-instances.sh`** (run-bos-mount addition, uncommitted). NEXT: clean, all pushed.

---

## 7. Operations & Release

| | OLD | NEXT |
|---|---|---|
| Local instances | `launch-test-instances.sh` (4: hos 8080, bsc 8081, welcb 8082, mount 8083) | manual cluster `/tmp/wrms-cluster` (hos 8096, bos 8097) |
| Windows | `dist/webrms-sql-infinityrms.exe` + `start-web-rms.bat`, release zips shipped | SCM-native service (`service install/start/stop/remove`, failure actions 1s→10s→30s) — **compile-verified only** |
| In-app settings | SQL editor + save-mode | none (config.toml + CLI) |
| Backup/parity/doctor | none | full CLI tooling + cutover checklist |

---

## 8. Known NEXT gaps to close (in priority order)

1. **Reports dept filter + `departments` endpoint** (small, user-visible).
2. **Payables UI depth**: per-supplier config panel, TSV export, paid ledger view (backend routes exist).
3. **Ordering UX**: keyboard nav/steppers, active-only filter, confirmation print (backend CSV exists).
4. **Stocktake toolbar**: supplier/uncounted filters, save-destination chooser.
5. **Settings page**: SQL editor is out of scope by design (config.toml), but a read-only status page (mode/branch/last poll/lag/backups) would replace it.
6. **P5**: release zips + B6 Windows service live verification (held per instruction).
7. OLD's unpushed `b285e95` + dirty launch script — push when convenient.

## 9. Verdict

- **Keep OLD as the production UI reference** — port its per-view tooling, not its architecture.
- **NEXT is the correct target**: local DB, outbox replication, snapshot fallback, ops tooling, items maintenance. Backend parity is essentially done.
- The honest headline: **backend ≈ parity (NEXT ahead on resilience/items/ops), UI ≈ 35% of OLD's depth**. The next workstream is UI-depth parity on the NEXT SPA, then P5 release.
