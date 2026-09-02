# WebRMS-Next — Improvements, Optimizations & Additions (revamp scope)
**Date:** 2026-09-02 · **Companion to:** `DESIGN-2026-09-02.md` (v1.1)
**Goal:** fold the best of what we learned in WebRMS + what the new architecture unlocks into the revamp, without bloating it.

---

## A. Optimizations (faster, leaner — mostly free because we own the DB)

| # | Item | Why / what | Phase |
|---|---|---|---|
| A1 | **SQLite production config** | WAL mode, `synchronous=NORMAL`, `busy_timeout`, FK ON, `PRAGMA optimize` on idle. sqlx pool (not a raw conn per request). Single-writer is fine (one process per install). | P0 |
| A2 | **Indexes from day one** | Composite PK `(branch,item,date)` on `sales_daily`; `(upc, source_key)` on items; `(origin_install,id)` unique on outbox-applied; covering indexes for the report shapes we already know (dept weekly, top movers, sellout). The 2,546-LOC reports module tells us exactly which. | P0 |
| A3 | **Batched, transactional ingest** | Connector pulls in batches → single multi-row INSERT/upsert per batch (`ON CONFLICT(source_key) DO UPDATE`), committed per batch. No per-row N+1 (the pitfall that bit reports before). | P1 |
| A4 | **Aggregate at ingest, roll up for reports** | `sales_daily` already aggregates ItemMovement. Add `sales_weekly`/`sales_monthly` rollups maintained at ingest (36 mo daily is ~2–6 GB; rollups make 12-wk/dept reports instant and shrink what the snapshot fallback ships). | P1 |
| A5 | **Connector resilience** | Port the deadpool lessons: connect/recycle timeouts (10s/5s), retry with backoff, per-source poll as a tokio task with graceful shutdown. Never hang a poll on a dead box. | P1 |
| A6 | **Staging + promote pipeline** | Connector writes to `staging_*` tables → validation → promote into main tables in one transaction. A bad pull can never corrupt the working DB; `parity-report` compares staging vs main trivially. | P1 |
| A7 | **Graceful shutdown + health** | axum signal handling (SIGTERM → drain polls, flush outbox, close DB cleanly). `/api/health` = DB ok, connector last-success age, snapshot staleness, replication lag — the thing operators and I check first when something "feels off". | P0 |

## B. Reliability & operations (the things that bit WebRMS)

| # | Item | Why / what | Phase |
|---|---|---|---|
| B1 | **tracing everywhere** | Replace println!-based logging (WebRMS has none). RUST_LOG=debug, spans per request/poll/ingest, log file + stdout. Debugging "works first, dies later" becomes minutes not hours. | P0 |
| B2 | **Backups built-in** | `webrms-next backup` (SQLite online backup / VACUUM INTO) + scheduled nightly + automatic pre-migration backup. Independence is only real if data survives a dead disk. `restore` with a documented flow. | P4 |
| B3 | **`doctor` + `init` CLI** | `init` bootstraps data.db + migrations + role config (first-run zero-touch for staff); `doctor` checks connector reachability, `PRAGMA integrity_check`, replication lag, disk space, and prints a pass/fail report. | P4 |
| B4 | **Auto-update path** | `sqlx migrate run` at startup (binary version ↔ schema version checked); replace-binary-and-restart is the whole upgrade. `/api/version` returns build+commit so a stale deploy is instantly visible (kills the hash-compare ritual). | P0/P5 |
| B5 | **Sync hardening** | Outbox apply tracking with unique `(origin_install, id)` → replays are safe; per-queue retry with backoff; poison-row quarantine. HMAC-signed outbox/snapshot pushes (cheap, stops LAN spoofing while keeping the no-auth trust model). | P3 |
| B6 | **Headless Windows service + self-recovery (CRITICAL — user-required 2026-09-02)** | Proper SCM service mode + watchdog self-restart. Full design below. | P5 (must) |
| B7 | **Soft deletes + tombstones** | Items marked InActive in AKPOS → `is_active` + `deleted_at`, never a hard delete: preserves history (the cost/GP audits depend on it). | P1 |

### Headless Windows service + self-recovery (B6 — elevated to must)

**Requirement:** WebRMS-Next runs **headless and as a Windows service**, and **self-recovers on fail or crash** — a store box must come back on its own after power events, reboots, or process death, with no staff interaction.

**1. Headless runtime**
- `webrms-next service install | start | stop | remove` subcommands (clap) using the **`windows-service` crate** — native SCM (Services Control Manager) integration, no third-party wrapper required for the primary path.
- `--service` mode: **no console window, no `pause_before_exit`** (that's a dev-mode behavior only); `anchor_cwd_to_exe()` stays so config/data/web resolve next to the exe.
- Logging: tracing → **rotating file** (`data/logs/`) + optional Windows Event Log (log-crate integration via `tracing`'s log bridge). `RUST_LOG` env applies. No stdout dependency in service mode.
- Interactive/dev mode stays as today (console + foreground) — one binary, two run modes.

**2. Service registration & recovery (the self-recovery core)**
- SCM recovery actions on failure: restart after 1s → 10s → 30s, reset failure count after 24h (so a permanently crashing binary doesn't reboot-loop forever, but transient crashes recover fast).
- **Start type = Automatic (delayed)** — boots with Windows, waits for network/Tailscale before the connector starts (Tailscale is a dependency for replication).
- NSSM documented as the fallback wrapper (auto-restart, stdout→file) for environments where a native service build is impractical — but the primary is SCM-native.

**3. In-process self-healing (crash-adjacent failures the SCM can't see)**
- **Supervisor task** in the app: monitors connector polls, replication ticks, and DB health; restarts failed background tasks; per-source circuit breaker (O-10) stops hammering a dead box and recovers automatically.
- **Startup self-healing:** `sqlx migrate run` (B4) → `PRAGMA quick_check` → on corruption signal, attempt auto-recovery (`VACUUM INTO` a fresh file) and log loudly. **Outbox replay** of pending config/orders (already designed) resumes interrupted writes; **resumable seed** (O-4) continues where it left off.
- **Crash-safe writes:** SQLite WAL mode + `synchronous=NORMAL` (A1) and atomic file writes (`write_atomic`) — a hard crash mid-write cannot corrupt the DB or a half-written ETL file.
- **Graceful shutdown:** service-stop signal → drain polls, flush outbox, close DB cleanly (A7) so a restart is always clean.

**4. Verification (P5, live)**
- Kill the process → SCM restarts it within ~1–2s; verify uptime after N kills.
- Stop the service → clean drain (no partial outbox) → start → pending config replayed.
- Simulate connector-dead (unreachable AKPOS) → circuit breaker trips, `/api/health` reports it, snapshot fallback (5b) engages for BoS; connector returns → recovers automatically.
- Power-loss simulation (kill -9 / taskkill /F) → on next boot the service auto-starts, DB passes quick_check, outbox replays.

## C. New capabilities the architecture unlocks (worth doing in the revamp)

| # | Item | Why / what | Phase |
|---|---|---|---|
| C1 | **Cross-branch reporting at HoS** | The HoS now materializes *all* branches' data locally (its connector + BoS fallback snapshots) — so true multi-store dashboards (top movers, sellout, stock, dept weekly across branches) become simple local queries. This was impossible with per-request AKPOS reads. | P2 |
| C2 | **Item cost history** | `item_cost_history (upc, source, effective_date, cost)` captured at ingest. Enables cost-trend reports, GP% audits, and makes the manual supplier cost-update ETL workflow (Hancocks/Federal price lists) verifiable in-system instead of by re-import. | P1 |
| C3 | **Audit trail with operator** | `audit_log (ts, operator, action, entity, before/after)`. The outbox already records origin; add who pressed Pay/ordered/marked-imported. Multi-operator stores (and the migration period) need this. Optional simple auth (argon2+JWT from the devflywheel starter) — default off, single-user stays the zero-friction mode. | P2 |
| C4 | **Export history UI** | Every generated .xlsx (PO, stocktake, AP) listed with re-download in the UI. Replaces hunting in data/output. | P2 |
| C5 | **Alerts (optional, defer)** | Low-stock / sellout / overdue-PO thresholds evaluated over the local DB on a timer (e.g. daily digest). Nice, but scope-able later — leave out of v1. | — |
| C6 | **Incoming-PO edit** | The approved-but-unbuilt feature lands cleanly in P2 (in DB it's an UPDATE + ETL regen, no calamine-for-meta dance). Inherited automatically. | P2 |

## D. UX/quality fixes inherited from WebRMS pain

| # | Item | Why / what | Phase |
|---|---|---|---|
| D1 | **Kill the hard-refresh ritual** | Serve `web/` with ETag/Last-Modified (axum static files) so browsers revalidate and pick up changes automatically — no more `?v=` bumps and Ctrl+Shift+R instructions. | P0 |
| D2 | **Frontend hygiene** | Namespace every view function (the global-collision regression), keep session state across navigation (the count-session wipe), accumulate HTML before single innerHTML assign (the text-blob table). Port the dup-check as a CI/lint script. | P2 |
| D3 | **Data-freshness badges** | Already in design (snapshot fallback age); extend to connector age + replication lag on every screen that depends on them. Operators see staleness, not just numbers. | P3 |
| D4 | **Live verification stays a rule** | Every phase's verify step is a live check (counts vs 100.71.113.111, HTTP 200 on 3 instances, real round-trips) — per the user's "valid means live-verified" rule. | all |

---

## Recommended v1 scope (default) vs defer

**In v1 (all A1–A7, B1–B7, C1–C4/C6, D1–D4):** they're either free at scaffold (A/B/D1) or directly serve the migration/independence goals (C1–C4, B2–B3, B7). Roughly +2–3 days total across P0–P4.

**Defer (flag for later):** C5 alerts; HMAC (B5) optional if Tailscale/Cloudflare Access is deemed sufficient (keep as a config flag). (B6 service mode is now a **must**, not deferred — see the full design above.)

**One judgment call for the user:** optional auth + operator audit (C3) — build the audit_log now (cheap, valuable during migration) and ship auth as config-flag later, or skip both until a second operator actually exists? *(audit_log: approved 2026-09-02, folded into DESIGN §3.)*

**Also see:** `REVIEW-GAP-ANALYSIS-2026-09-02.md` — the phase-by-phase gap fold-in (must-adds G-1/G-2/G-3/G-9/G-10 + O-1/O-3/O-8/O-12, carry-overs G-4–G-7/O-6/O-11, optimizations O-2/O-4/O-5/O-7/O-9/O-10). All now live in DESIGN §3/§8 and WORKFLOWS §10.
