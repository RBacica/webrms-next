-- 0003: ordering fields on items (min/max qty + no-order flags — ported from
-- the ordering-demand LineInput surface).
ALTER TABLE items ADD COLUMN min_qty REAL NOT NULL DEFAULT 0;
ALTER TABLE items ADD COLUMN max_qty REAL NOT NULL DEFAULT 0;
ALTER TABLE items ADD COLUMN no_order INTEGER NOT NULL DEFAULT 0;
