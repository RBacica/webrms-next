-- 0008: receipts PO link via BillOfLading — the live SMHeaders has NO POID
-- column; import is confirmed by SMHeaders.BillOfLading matching the 10-char
-- code written into the ETL xlsx (same mechanism the old WebRMS used).
ALTER TABLE receipts ADD COLUMN bill_of_lading TEXT;
CREATE INDEX idx_receipts_bol ON receipts(bill_of_lading);