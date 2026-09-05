-- 0013: payment-mix + hourly rollups (header-level). sales_daily cannot
-- produce these (no payment-media or hour-of-day dimension) — the connector
-- aggregates TransPayments/TransHeaders at pull time instead.
CREATE TABLE sales_payment (
    id          INTEGER PRIMARY KEY,
    branch_id   INTEGER NOT NULL REFERENCES branches(id),
    sale_date   TEXT NOT NULL,             -- YYYY-MM-DD
    media       TEXT NOT NULL,             -- payment media (quotes stripped)
    txns        INTEGER NOT NULL DEFAULT 0,
    value       REAL NOT NULL DEFAULT 0,
    fees        REAL NOT NULL DEFAULT 0,
    change_amt  REAL NOT NULL DEFAULT 0,
    UNIQUE(branch_id, sale_date, media)
);

CREATE TABLE sales_hourly (
    id          INTEGER PRIMARY KEY,
    branch_id   INTEGER NOT NULL REFERENCES branches(id),
    sale_date   TEXT NOT NULL,             -- YYYY-MM-DD
    hour        INTEGER NOT NULL,          -- 0..23
    dow         INTEGER NOT NULL,          -- 0=Sun..6=Sat (SQL datepart)
    station     INTEGER NOT NULL,
    txns        INTEGER NOT NULL DEFAULT 0,
    net         REAL NOT NULL DEFAULT 0,
    UNIQUE(branch_id, sale_date, hour, dow, station)
);
CREATE INDEX idx_sales_payment_date ON sales_payment(branch_id, sale_date);
CREATE INDEX idx_sales_hourly_date ON sales_hourly(branch_id, sale_date);
