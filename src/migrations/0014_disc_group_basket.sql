-- 0014: items.disc_group + overview basket analytics rollups.
-- disc_group mirrors AKPOS Items.DiscGroup (numeric, stored TEXT for parity
-- with class/sub_department).
ALTER TABLE items ADD COLUMN disc_group TEXT;

-- Per-department line-level basket composition (net + units per dept per day).
-- Basket (txn) counts come from sales_hourly on the same date.
CREATE TABLE sales_basket_dept (
    id          INTEGER PRIMARY KEY,
    branch_id   INTEGER NOT NULL REFERENCES branches(id),
    sale_date   TEXT NOT NULL,             -- YYYY-MM-DD
    dept_id     TEXT NOT NULL,
    dept_name   TEXT NOT NULL,
    net         REAL NOT NULL DEFAULT 0,
    units       REAL NOT NULL DEFAULT 0,
    UNIQUE(branch_id, sale_date, dept_id)
);
CREATE INDEX idx_basket_dept_date ON sales_basket_dept(branch_id, sale_date);

-- Basket-size distribution (per-txn TotalAfterTax bands).
CREATE TABLE sales_basket_band (
    id          INTEGER PRIMARY KEY,
    branch_id   INTEGER NOT NULL REFERENCES branches(id),
    sale_date   TEXT NOT NULL,
    band        TEXT NOT NULL,
    txns        INTEGER NOT NULL DEFAULT 0,
    UNIQUE(branch_id, sale_date, band)
);
CREATE INDEX idx_basket_band_date ON sales_basket_band(branch_id, sale_date);

-- Voided transactions (TransHeaders TransStatus='V').
CREATE TABLE voids_daily (
    id          INTEGER PRIMARY KEY,
    branch_id   INTEGER NOT NULL REFERENCES branches(id),
    sale_date   TEXT NOT NULL,
    count       INTEGER NOT NULL DEFAULT 0,
    value       REAL NOT NULL DEFAULT 0,
    UNIQUE(branch_id, sale_date)
);
