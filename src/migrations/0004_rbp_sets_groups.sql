-- 0004: RBP set/group materialization so PROSET promo rules resolve locally
-- (standalone mode must not need the live PricingProductSet/PricingGroup).
CREATE TABLE pricing_groups (
    id          INTEGER PRIMARY KEY,
    group_id    INTEGER NOT NULL,           -- live PricingGroup.GroupID
    description TEXT,
    data_key    TEXT,                       -- item UPC when Type='Items'
    type        TEXT,                       -- 'Items' | others
    is_active   INTEGER NOT NULL DEFAULT 1,
    UNIQUE(group_id, data_key)
);
CREATE INDEX idx_pg_group ON pricing_groups(group_id);

CREATE TABLE pricing_sets (
    id          INTEGER PRIMARY KEY,
    set_id      INTEGER NOT NULL,           -- live PricingProductSet.SetID
    set_line    INTEGER NOT NULL,           -- SetLine (1..N)
    group_id    INTEGER NOT NULL,
    min_qty     REAL NOT NULL DEFAULT 1,
    max_qty     REAL NOT NULL DEFAULT 0,
    UNIQUE(set_id, set_line)
);
CREATE INDEX idx_ps_set ON pricing_sets(set_id);
