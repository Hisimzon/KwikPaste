-- Last time a record was actually used: first capture, a repeated copy of the same content,
-- or a copy / paste from history. Automatic cleanup ages records by this column. It moves even
-- when "update on reuse" is off, which keeps updated_at (the list order) unchanged on paste.
ALTER TABLE clipboard_items ADD COLUMN last_used_at TEXT;

UPDATE clipboard_items SET last_used_at = updated_at;

-- Retention cleanup filters plain records by last_used_at; storage and count cleanup walk it in order.
CREATE INDEX IF NOT EXISTS idx_clipboard_items_pinned_last_used
    ON clipboard_items (is_pinned, last_used_at);
