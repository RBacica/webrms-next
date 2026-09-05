-- 0011: stocktake run LINES (G-3 costed variance). Each export saves its
-- counted rows so Reports → Stocktake & Shrink can show costed variance
-- (shrink $ / overage $ per run) without re-deriving from Infinity movement.
CREATE TABLE stocktake_run_lines (
    id              INTEGER PRIMARY KEY,
    run_id          TEXT NOT NULL REFERENCES stocktake_runs(id),
    upc             TEXT NOT NULL,
    description     TEXT,
    stock_on_hand   REAL NOT NULL DEFAULT 0,
    counted         REAL NOT NULL DEFAULT 0,
    unit_cost       REAL,
    variance_units  REAL,
    variance_cost   REAL
);
CREATE INDEX idx_run_lines_run ON stocktake_run_lines(run_id);
