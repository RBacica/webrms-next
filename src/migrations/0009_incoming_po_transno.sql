-- 0009: incoming_pos.trans_no — the Infinity TransNo the imported PO becomes
-- (set when the BillOfLading import match fires). Goods-in 'G' rows link back
-- via OriginatingTransNo = the P's TransNo, so the receipted step resolves
-- through this transID rather than re-matching the Bol.
ALTER TABLE incoming_pos ADD COLUMN trans_no INTEGER;