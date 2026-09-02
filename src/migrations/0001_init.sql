-- WebRMS-Next initial schema (P0)
-- PK strategy (O-1): replicated tables use TEXT UUID PKs (no AUTOINCREMENT collisions
-- across installs); connector-materialized tables keep INTEGER ids keyed by source_key.
-- Timezone (O-8): UTC timestamps; per-branch offset in branches table.
-- Provenance: every business table carries source / source_key / last_synced_at / is_active.

PRAGMA foreign_keys = ON;
PRAGMA journal_mode = WAL;

-- ── Reference / connector-materialized (INTEGER ids, source_key = AKPOS pk) ──────────

CREATE TABLE branches (
    id              INTEGER PRIMARY KEY,          -- local id (branch-mapped)
    ext_key         INTEGER,                      -- AKPOS Branches.ID (10..17 new / 336..342 old)
    source          TEXT NOT NULL DEFAULT 'seed',
    source_key      TEXT,
    name            TEXT NOT NULL,
    short_name      TEXT,
    address         TEXT, city TEXT, region TEXT, postcode TEXT, country TEXT,
    phone           TEXT, gst_no TEXT,
    is_ho           INTEGER NOT NULL DEFAULT 0,
    tz_offset_min   INTEGER NOT NULL DEFAULT 720,  -- NZST; NZDT adjusted at runtime
    is_active       INTEGER NOT NULL DEFAULT 1,
    last_synced_at  TEXT
);

CREATE TABLE departments (
    id              INTEGER PRIMARY KEY,
    ext_key         INTEGER,                      -- AKPOS Departments.ID
    source          TEXT NOT NULL DEFAULT 'seed',
    source_key      TEXT,
    name            TEXT NOT NULL,
    target_margin   REAL NOT NULL DEFAULT 25.0,
    is_active       INTEGER NOT NULL DEFAULT 1,
    last_synced_at  TEXT
);

CREATE TABLE suppliers (
    id              INTEGER PRIMARY KEY,
    ext_key         TEXT,                         -- AKPOS Customers.Code ('010'..)
    source          TEXT NOT NULL DEFAULT 'seed',
    source_key      TEXT,
    code            TEXT NOT NULL UNIQUE,
    name            TEXT NOT NULL,
    first_name      TEXT, last_name TEXT,         -- Customers names
    disc_group      INTEGER, disc_percent REAL, disc_days INTEGER,  -- Customers.Disc*
    is_active       INTEGER NOT NULL DEFAULT 1,
    last_synced_at  TEXT
);

CREATE TABLE items (
    id              INTEGER PRIMARY KEY,
    upc             TEXT NOT NULL UNIQUE,         -- AKPOS Items.UPC (PK in live system)
    source          TEXT NOT NULL DEFAULT 'seed',
    source_key      TEXT,
    sku             TEXT, description TEXT,
    department_id   INTEGER REFERENCES departments(id),
    supplier_id     INTEGER REFERENCES suppliers(id),
    parent_upc      TEXT,
    class           TEXT, sub_department TEXT,
    cost            REAL, cost_ave REAL, purchase_cost REAL,
    price1          REAL, price2 REAL, price3 REAL, price4 REAL,
    price5          REAL, price6 REAL, price7 REAL, price8 REAL,
    tax_code        TEXT, pack_units REAL, volume_ml REAL,
    non_stock       INTEGER NOT NULL DEFAULT 0,
    is_active       INTEGER NOT NULL DEFAULT 1,   -- soft delete (B7)
    last_synced_at  TEXT
);
CREATE INDEX idx_items_dept ON items(department_id);
CREATE INDEX idx_items_supplier ON items(supplier_id);
CREATE INDEX idx_items_parent ON items(parent_upc);

CREATE TABLE item_barcodes (
    id              INTEGER PRIMARY KEY,
    upc             TEXT NOT NULL REFERENCES items(upc),
    barcode         TEXT NOT NULL,                -- alt barcode (primary UPC lives in items.upc)
    UNIQUE(upc, barcode)
);
CREATE INDEX idx_barcodes_code ON item_barcodes(barcode);

-- History alias (G-9): old UPC -> new UPC after a clone, so forecast/sellout carry history forward
CREATE TABLE history_alias (
    id              INTEGER PRIMARY KEY,
    old_upc         TEXT NOT NULL UNIQUE,
    new_upc         TEXT NOT NULL,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    created_by      TEXT
);
CREATE INDEX idx_history_alias_new ON history_alias(new_upc);

-- Cost + price history (C2/G-8)
CREATE TABLE item_cost_history (
    id              INTEGER PRIMARY KEY,
    upc             TEXT NOT NULL REFERENCES items(upc),
    effective_date  TEXT NOT NULL,
    cost            REAL, cost_ave REAL, purchase_cost REAL,
    source          TEXT NOT NULL DEFAULT 'connector',
    UNIQUE(upc, effective_date)
);
CREATE TABLE item_price_history (
    id              INTEGER PRIMARY KEY,
    upc             TEXT NOT NULL REFERENCES items(upc),
    effective_date  TEXT NOT NULL,
    price1          REAL,
    source          TEXT NOT NULL DEFAULT 'connector',
    UNIQUE(upc, effective_date)
);

-- Stock position per branch (G-1): refreshed every connector poll
CREATE TABLE stock_current (
    id              INTEGER PRIMARY KEY,
    branch_id       INTEGER NOT NULL REFERENCES branches(id),
    upc             TEXT NOT NULL REFERENCES items(upc),
    qty             REAL NOT NULL DEFAULT 0,
    as_of           TEXT NOT NULL,
    source          TEXT NOT NULL DEFAULT 'connector',
    UNIQUE(branch_id, upc)
);
CREATE INDEX idx_stock_branch ON stock_current(branch_id);
CREATE INDEX idx_stock_upc ON stock_current(upc);

-- Aggregated daily sales (never raw ItemMovement rows)
CREATE TABLE sales_daily (
    id              INTEGER PRIMARY KEY,
    branch_id       INTEGER NOT NULL REFERENCES branches(id),
    upc             TEXT NOT NULL REFERENCES items(upc),
    sale_date       TEXT NOT NULL,                -- YYYY-MM-DD (UTC)
    units           REAL NOT NULL DEFAULT 0,
    revenue         REAL NOT NULL DEFAULT 0,
    promo_units     REAL NOT NULL DEFAULT 0,      -- LineType='S'
    normal_units    REAL NOT NULL DEFAULT 0,      -- LineType='N'
    promo_price     REAL,
    cost_amount     REAL NOT NULL DEFAULT 0,
    line_margin     REAL NOT NULL DEFAULT 0,
    UNIQUE(branch_id, upc, sale_date)
);
CREATE INDEX idx_sales_date ON sales_daily(sale_date);
CREATE INDEX idx_sales_upc ON sales_daily(upc);
CREATE INDEX idx_sales_branch_date ON sales_daily(branch_id, sale_date);

-- Receipts incl. 'Z' returns (G-2): net bills-due = goods-in ('G') − returns ('Z')
CREATE TABLE receipts (
    id              INTEGER PRIMARY KEY,
    branch_id       INTEGER NOT NULL REFERENCES branches(id),
    trans_no        INTEGER NOT NULL,
    station         INTEGER NOT NULL,
    trans_type      TEXT NOT NULL,                -- P purchase / G goods-in / I invoice / Z return
    supplier_id     INTEGER REFERENCES suppliers(id),
    invoice_no      TEXT,
    total_cost      REAL NOT NULL DEFAULT 0,
    logged          TEXT NOT NULL,
    UNIQUE(branch_id, trans_no, station)
);
CREATE INDEX idx_receipts_supplier ON receipts(supplier_id);
CREATE INDEX idx_receipts_logged ON receipts(logged);

CREATE TABLE receipt_lines (
    id              INTEGER PRIMARY KEY,
    receipt_id      INTEGER NOT NULL REFERENCES receipts(id),
    upc             TEXT NOT NULL,
    quantity        REAL NOT NULL DEFAULT 0,
    unit_cost       REAL,
    ext_cost        REAL,
    status          TEXT,
    cost_ave_local  REAL
);
CREATE INDEX idx_receipt_lines_upc ON receipt_lines(upc);

-- AP invoices (payables over local DB)
CREATE TABLE ap_invoices (
    id              INTEGER PRIMARY KEY,
    branch_id       INTEGER NOT NULL REFERENCES branches(id),
    supplier_id     INTEGER REFERENCES suppliers(id),
    invoice_number  TEXT,
    description     TEXT,
    invoice_date    TEXT,
    due_date        TEXT,
    discount_date   TEXT,
    invoice_amount  REAL NOT NULL DEFAULT 0,
    paid_amount     REAL NOT NULL DEFAULT 0,
    discount_amount REAL NOT NULL DEFAULT 0,
    discount_pc     REAL,
    po_number       TEXT,
    freight         REAL NOT NULL DEFAULT 0,
    tax_amount1     REAL NOT NULL DEFAULT 0,
    status          TEXT,
    is_matched      INTEGER NOT NULL DEFAULT 0,
    logged          TEXT,
    UNIQUE(branch_id, supplier_id, invoice_number)
);
CREATE INDEX idx_ap_supplier ON ap_invoices(supplier_id);

-- Promo rules — one table covers both HoS generations (Specials OR RBP Pricing*)
CREATE TABLE promo_rules (
    id              INTEGER PRIMARY KEY,
    kind            TEXT NOT NULL,                -- 'special' | 'rbp_condition' | 'app_rule'
    source          TEXT NOT NULL DEFAULT 'connector',
    source_key      TEXT,
    payload         TEXT NOT NULL,                -- JSON (Specials row or PricingCondition)
    sequence_match  TEXT,                         -- bare UPC | groupid|upc | setid|line
    condition_type  TEXT,                         -- RETAIL/LOCAL/PROSET
    adjustment_type TEXT,
    adjustment_value REAL,
    effective_start TEXT,
    effective_end   TEXT,
    branch_scope    INTEGER,
    is_active       INTEGER NOT NULL DEFAULT 1,
    last_synced_at  TEXT
);
CREATE INDEX idx_promo_active ON promo_rules(is_active, effective_start, effective_end);

-- ── Replicated / app-authored (UUID PKs per O-1) ────────────────────────────────────

-- Transactional outbox: every local write to a replicated table
CREATE TABLE outbox (
    id              TEXT PRIMARY KEY,             -- UUID
    origin_install  TEXT NOT NULL,
    table_name      TEXT NOT NULL,
    row_id          TEXT NOT NULL,
    op              TEXT NOT NULL,                -- insert/update/delete
    payload         TEXT NOT NULL,                -- JSON
    ts              TEXT NOT NULL DEFAULT (datetime('now')),
    applied         INTEGER NOT NULL DEFAULT 0,
    UNIQUE(origin_install, id)
);
CREATE INDEX idx_outbox_applied ON outbox(applied);

CREATE TABLE orders (
    id              TEXT PRIMARY KEY,             -- UUID
    origin_install  TEXT NOT NULL,
    branch_id       INTEGER NOT NULL REFERENCES branches(id),
    supplier_id     INTEGER REFERENCES suppliers(id),
    placed_at       TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'open', -- open → receipted → cleared (G-10)
    cleared_at      TEXT,                         -- G-10 cleared_order_ids equivalent
    total_qty       REAL NOT NULL DEFAULT 0,
    total_cost      REAL NOT NULL DEFAULT 0,
    created_by      TEXT,
    updated_at      TEXT
);
CREATE INDEX idx_orders_branch_status ON orders(branch_id, status);

CREATE TABLE order_lines (
    id              TEXT PRIMARY KEY,             -- UUID
    order_id        TEXT NOT NULL REFERENCES orders(id),
    upc             TEXT NOT NULL,
    qty             REAL NOT NULL DEFAULT 0,
    unit_cost       REAL,
    line_total      REAL,
    suggested_qty   REAL
);
CREATE INDEX idx_order_lines_upc ON order_lines(upc);

-- Incoming PO tracking (filename-keyed like today, plus BillOfLading + status)
CREATE TABLE incoming_pos (
    id              TEXT PRIMARY KEY,             -- UUID
    origin_install  TEXT NOT NULL,
    branch_id       INTEGER NOT NULL REFERENCES branches(id),
    supplier_id     INTEGER REFERENCES suppliers(id),
    filename        TEXT NOT NULL,
    bill_of_lading  TEXT,
    poid            INTEGER,
    status          TEXT NOT NULL DEFAULT 'waiting_import',  -- unknown|waiting_import|pending_receipt|receipted
    imported        INTEGER NOT NULL DEFAULT 0,
    placed_at       TEXT,
    updated_at      TEXT,
    UNIQUE(filename)
);

-- Config-class (HoS-authored, replicated down)
CREATE TABLE settings (
    key             TEXT PRIMARY KEY,
    value           TEXT,
    updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_by      TEXT
);

CREATE TABLE supplier_modes (
    supplier_code   TEXT PRIMARY KEY,
    mode            TEXT NOT NULL DEFAULT 'weekly',
    lead_days       INTEGER NOT NULL DEFAULT 3,
    cycle_days      INTEGER,
    cover_days      INTEGER,
    source          TEXT NOT NULL DEFAULT 'connector',  -- 'connector'|'app' (authority 1a)
    updated_at      TEXT
);

CREATE TABLE supplier_terms (
    supplier_code   TEXT PRIMARY KEY,
    term_type       TEXT,
    term_days       INTEGER,
    order_type      TEXT,
    payment_type    TEXT,
    configured      INTEGER NOT NULL DEFAULT 0,
    source          TEXT NOT NULL DEFAULT 'connector',
    updated_at      TEXT
);

CREATE TABLE paid_ledger (
    id              TEXT PRIMARY KEY,             -- UUID
    branch_id       INTEGER,
    supplier_code   TEXT,
    invoice_no      TEXT,
    paid_at         TEXT NOT NULL,
    amount          REAL NOT NULL DEFAULT 0,
    note            TEXT,
    origin_install  TEXT,
    UNIQUE(branch_id, supplier_code, invoice_no)
);

-- ── Scanback / rebate tracking (app-only, config-class) ─────────────────────────────

CREATE TABLE rebate_contracts (
    id              TEXT PRIMARY KEY,             -- UUID
    description     TEXT,
    branch_scope    INTEGER,
    supplier_code   TEXT,
    rebate_type     TEXT NOT NULL DEFAULT 'scanback',  -- scanback|supplier
    basis           TEXT NOT NULL DEFAULT 'price_delta', -- fixed_per_unit|pct_of_sell|price_delta|margin_protect
    amount          REAL,
    rate_pct        REAL,
    special_id      INTEGER REFERENCES promo_rules(id), -- NULL for supplier/dept rebates
    sell_from       TEXT, sell_to TEXT,
    receipt_from    TEXT, receipt_to TEXT,
    max_qty         REAL,
    is_active       INTEGER NOT NULL DEFAULT 1,
    created_by      TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT
);

CREATE TABLE rebate_contract_lines (
    id              TEXT PRIMARY KEY,             -- UUID
    contract_id     TEXT NOT NULL REFERENCES rebate_contracts(id),
    scope_type      TEXT NOT NULL,                -- upc|group|set|dept|supplier_all
    scope_key       TEXT,
    amount_override REAL,
    rate_override   REAL
);

CREATE TABLE rebate_ledger (
    id              TEXT PRIMARY KEY,             -- UUID
    contract_id     TEXT REFERENCES rebate_contracts(id),
    special_id      INTEGER REFERENCES promo_rules(id),
    branch_id       INTEGER,
    supplier_code   TEXT,
    invoice_or_ref  TEXT,
    received_amount REAL NOT NULL DEFAULT 0,
    received_date   TEXT NOT NULL,
    received_by     TEXT,
    note            TEXT,
    created_at      TEXT NOT NULL DEFAULT (datetime('now'))
);

-- ── Workflow tables (stocktake / item ETL) ──────────────────────────────────────────

CREATE TABLE stocktake_schedules (
    id              TEXT PRIMARY KEY,             -- UUID
    description     TEXT NOT NULL,
    department_id   INTEGER REFERENCES departments(id),
    day_of_week     INTEGER,                      -- NULL = daily
    branch_scope    INTEGER,                      -- NULL = all branches
    source          TEXT NOT NULL DEFAULT 'connector',  -- mirror Infinity; app-authoritative when configured
    is_active       INTEGER NOT NULL DEFAULT 1,
    updated_at      TEXT
);

CREATE TABLE stocktake_runs (
    id              TEXT PRIMARY KEY,             -- UUID
    schedule_id     TEXT REFERENCES stocktake_schedules(id),
    branch_id       INTEGER NOT NULL REFERENCES branches(id),
    started_at      TEXT NOT NULL,
    completed_at    TEXT,
    status          TEXT NOT NULL DEFAULT 'in_progress',  -- in_progress|exported|restored|verified
    count_file      TEXT,                         -- .txt path
    ticket_file     TEXT,                         -- .qry path
    shrink_total    REAL,                         -- costed result after pull-back (G-3)
    overage_total   REAL
);

CREATE TABLE item_etl_exports (
    id              TEXT PRIMARY KEY,             -- UUID
    kind            TEXT NOT NULL,                -- edit|clone|price_list
    filename        TEXT NOT NULL,
    rows_changed    INTEGER NOT NULL DEFAULT 0,
    status          TEXT NOT NULL DEFAULT 'exported',  -- exported|imported|verified
    verify_diff     TEXT,                         -- JSON before/after
    requested_by    TEXT, approved_by TEXT,
    source_branch   INTEGER,
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    verified_at     TEXT
);

CREATE TABLE item_change_requests (
    id              TEXT PRIMARY KEY,             -- UUID
    request_type    TEXT NOT NULL,                -- edit|clone|price_list
    payload         TEXT NOT NULL,                -- JSON (item fields / clone spec)
    status          TEXT NOT NULL DEFAULT 'suggested',  -- suggested|approved|exported|imported|verified|rejected
    requested_by    TEXT,
    source_branch   INTEGER,
    approved_by     TEXT,
    reject_note     TEXT,
    etl_export_id   TEXT REFERENCES item_etl_exports(id),
    created_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at      TEXT
);

-- ── Ops / infra ─────────────────────────────────────────────────────────────────────

-- Authority model (1a): app-overridden fields the connector must never overwrite
CREATE TABLE app_overrides (
    entity_type     TEXT NOT NULL,                -- item|promo|schedule|supplier_mode...
    entity_key      TEXT NOT NULL,
    field           TEXT NOT NULL,
    value           TEXT,                         -- JSON
    conflict_state  TEXT NOT NULL DEFAULT 'clean',  -- clean|external_edit (O-12)
    updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
    updated_by      TEXT,
    PRIMARY KEY (entity_type, entity_key, field)
);

CREATE TABLE high_watermarks (
    source          TEXT NOT NULL,
    table_name      TEXT NOT NULL,
    last_key        TEXT,                         -- last id/timestamp pulled
    updated_at      TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (source, table_name)
);

CREATE TABLE sync_log (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    ts              TEXT NOT NULL DEFAULT (datetime('now')),
    direction       TEXT NOT NULL,                -- up|down|snapshot|ingest
    status          TEXT NOT NULL,                -- ok|failed|partial
    detail          TEXT,
    rows_processed  INTEGER
);

CREATE TABLE audit_log (
    id              TEXT PRIMARY KEY,             -- UUID
    ts              TEXT NOT NULL DEFAULT (datetime('now')),
    operator        TEXT,
    action          TEXT NOT NULL,
    entity_type     TEXT NOT NULL,
    entity_key      TEXT,
    before_json     TEXT,
    after_json      TEXT,
    origin_install  TEXT,
    outbox_id       TEXT
);
CREATE INDEX idx_audit_entity ON audit_log(entity_type, entity_key);

CREATE TABLE users (
    id              TEXT PRIMARY KEY,             -- UUID
    code            TEXT NOT NULL UNIQUE,         -- AKPOS B_Users code ('012')
    name            TEXT,
    is_active       INTEGER NOT NULL DEFAULT 1,
    last_synced_at  TEXT
);

-- Branch mapping: old-system branch ids → new-system branch ids (new HoS 10-17 vs old 336-342)
CREATE TABLE branch_mapping (
    source          TEXT NOT NULL,
    source_branch   INTEGER NOT NULL,
    branch_id       INTEGER NOT NULL REFERENCES branches(id),
    PRIMARY KEY (source, source_branch)
);
