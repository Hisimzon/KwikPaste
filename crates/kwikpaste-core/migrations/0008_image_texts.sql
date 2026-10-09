CREATE TABLE image_texts (
    item_id TEXT PRIMARY KEY NOT NULL REFERENCES clipboard_items(id) ON DELETE CASCADE,
    status TEXT NOT NULL CHECK(status IN ('done', 'failed', 'skipped')),
    text TEXT NOT NULL DEFAULT '',
    language TEXT,
    attempts INTEGER NOT NULL DEFAULT 1 CHECK(attempts BETWEEN 1 AND 3),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX image_texts_retry ON image_texts(status, attempts);
CREATE INDEX clipboard_images_ocr_queue ON clipboard_items(created_at DESC, id DESC) WHERE kind = 'image';
CREATE VIRTUAL TABLE image_texts_fts USING fts5(text, content='image_texts', content_rowid='rowid', tokenize='trigram');
CREATE TRIGGER image_texts_ai AFTER INSERT ON image_texts BEGIN
    INSERT INTO image_texts_fts(rowid, text) VALUES (new.rowid, new.text);
END;
CREATE TRIGGER image_texts_ad AFTER DELETE ON image_texts BEGIN
    INSERT INTO image_texts_fts(image_texts_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
END;
CREATE TRIGGER image_texts_au AFTER UPDATE ON image_texts BEGIN
    INSERT INTO image_texts_fts(image_texts_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
    INSERT INTO image_texts_fts(rowid, text) VALUES (new.rowid, new.text);
END;
