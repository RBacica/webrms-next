-- 0012: items.supplier_prod_code (AKPOS Items.SupplierProdCode) — used by the
-- replacement report (match level 2: same supplier product code) and the item
-- edit/clone ETL patch writer (W6). Nullable; populated by the connector.
ALTER TABLE items ADD COLUMN supplier_prod_code TEXT;
