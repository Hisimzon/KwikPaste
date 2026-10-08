-- User-defined local ordering for pinned and favorite sections.
ALTER TABLE clipboard_items ADD COLUMN pin_order INTEGER;
ALTER TABLE clipboard_items ADD COLUMN favorite_order INTEGER;

-- Preserve the order users already saw under the default sort when assigning the
-- first manual order values. Larger values are displayed first.
WITH ranked AS (
    SELECT id, ROW_NUMBER() OVER (ORDER BY updated_at ASC, created_at ASC) AS rank
    FROM clipboard_items
    WHERE is_pinned = 1
)
UPDATE clipboard_items
SET pin_order = (SELECT rank FROM ranked WHERE ranked.id = clipboard_items.id)
WHERE is_pinned = 1;

WITH ranked AS (
    SELECT id, ROW_NUMBER() OVER (ORDER BY updated_at ASC, created_at ASC) AS rank
    FROM clipboard_items
    WHERE is_favorite = 1
)
UPDATE clipboard_items
SET favorite_order = (SELECT rank FROM ranked WHERE ranked.id = clipboard_items.id)
WHERE is_favorite = 1;

-- Keep one 0003 index for retention/count cleanup, whose ORDER BY intentionally
-- omits the manual rank. The other three list indexes are superseded below.
DROP INDEX IF EXISTS idx_clipboard_items_pinned_updated;
DROP INDEX IF EXISTS idx_clipboard_items_kind_pinned_updated;
DROP INDEX IF EXISTS idx_clipboard_items_group_pinned_updated;

-- The leading columns mirror the five list ORDER BY shapes. Do not add an index
-- for every sort variant: the manual-rank prefix is the expensive part and the
-- remaining sort variants are allowed to use a temporary b-tree.
CREATE INDEX IF NOT EXISTS idx_clipboard_items_pinned_updated_manual
    ON clipboard_items (is_pinned DESC, pin_order DESC, updated_at DESC, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_clipboard_items_pinned_created_manual
    ON clipboard_items (is_pinned DESC, pin_order DESC, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_clipboard_items_kind_pinned_updated_manual
    ON clipboard_items (kind, is_pinned DESC, pin_order DESC, updated_at DESC, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_clipboard_items_group_pinned_updated_manual
    ON clipboard_items (group_id, is_pinned DESC, pin_order DESC, updated_at DESC, created_at DESC);
CREATE INDEX IF NOT EXISTS idx_clipboard_items_favorite_manual_updated
    ON clipboard_items (is_favorite, is_pinned DESC, pin_order DESC, favorite_order DESC, updated_at DESC, created_at DESC);
