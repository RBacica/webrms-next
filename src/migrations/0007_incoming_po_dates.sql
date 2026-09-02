-- 0007: incoming_pos lifecycle timestamps (auto-flip G-7).
ALTER TABLE incoming_pos ADD COLUMN imported_at TEXT;
ALTER TABLE incoming_pos ADD COLUMN receipted_at TEXT;
