-- 0006: receipt PO linkage — OriginatingTransNo + POID on receipts so the
-- incoming-PO lifecycle (waiting_import → pending_receipt → receipted, G-7)
-- can be auto-flipped locally: a 'P' receipt with our POID confirms import;
-- a 'G' receipt whose OriginatingTransNo points at that P confirms receipt.
ALTER TABLE receipts ADD COLUMN poid TEXT;
ALTER TABLE receipts ADD COLUMN originating_trans_no INTEGER;
CREATE INDEX idx_receipts_poid ON receipts(poid);
