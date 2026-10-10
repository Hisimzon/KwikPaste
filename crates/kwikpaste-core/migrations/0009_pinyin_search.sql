-- Pinyin search index for CJK content: `search_pinyin` holds the compacted and
-- space-separated syllables, `search_pinyin_initials` the first letters, so a
-- query typed in pinyin can match a Chinese record. Both columns stay NULL for
-- rows written before this migration; a background backfill fills them in.
ALTER TABLE clipboard_items ADD COLUMN search_pinyin TEXT;
ALTER TABLE clipboard_items ADD COLUMN search_pinyin_initials TEXT;