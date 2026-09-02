-- 0005: promo_rules.description — the source condition/special description was
-- being dropped by the connector (pulled but never mapped). Needed by the
-- promotions list + effectiveness views.
ALTER TABLE promo_rules ADD COLUMN description TEXT;
