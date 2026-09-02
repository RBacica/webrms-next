-- 0002: promo_rules needs a uniqueness target for ON CONFLICT DO NOTHING
-- (kind + source + source_key = one rule per source row, no dupes on re-pull).
CREATE UNIQUE INDEX IF NOT EXISTS uq_promo_rule ON promo_rules(kind, source, source_key);

-- items upsert keys on upc (already UNIQUE in 0001) — no change needed.
-- sales_daily upsert keys on (branch_id, upc, sale_date) — already UNIQUE in 0001.
