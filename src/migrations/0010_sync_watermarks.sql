-- 0010: replication sync watermarks — per-source pull tracking for the
-- outbox (config down, orders bidirectional, POs up). Each client install
-- records the last (ts, id) it applied from its source so pulls are
-- resumable and idempotent.
CREATE TABLE sync_watermarks (
    source      TEXT PRIMARY KEY,   -- e.g. "http://100.71.113.111:8080"
    last_ts     TEXT NOT NULL DEFAULT '',
    last_id     TEXT NOT NULL DEFAULT '',
    updated_at  TEXT
);
