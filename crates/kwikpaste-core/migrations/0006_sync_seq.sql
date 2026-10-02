-- Local capture order for LAN sync catch-up (protocol v2). A record captured on this device gets the
-- next counter value when it is captured, or copied again; a peer asks for everything after the last
-- value it has seen. Records received from peers or imported from a backup never get one, so they are
-- never offered to other devices as if they had been copied here.
ALTER TABLE clipboard_items ADD COLUMN sync_seq INTEGER;

CREATE INDEX IF NOT EXISTS idx_clipboard_items_sync_seq
    ON clipboard_items (sync_seq) WHERE sync_seq IS NOT NULL;

-- Single-row counter. Values only grow, even after the newest record is deleted, so a peer's
-- watermark never points at a value that gets handed out again.
CREATE TABLE sync_counter (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    value      INTEGER NOT NULL,
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);

INSERT INTO sync_counter (id, value, created_at, updated_at)
VALUES (1, 0, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));
