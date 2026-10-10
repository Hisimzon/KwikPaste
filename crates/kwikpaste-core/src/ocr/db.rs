//! 派生队列与结果写入；只使用会话专用连接，不占用主池的页缓存。
use super::protocol::Outcome;
use anyhow::Result;
use sqlx::SqliteConnection;

pub(super) const NEXT_JOB: &str = "SELECT i.id, i.content, i.content_hash FROM clipboard_items i LEFT JOIN image_texts t ON t.item_id = i.id WHERE i.kind = 'image' AND (t.item_id IS NULL OR (t.status = 'failed' AND t.attempts < 3)) ORDER BY i.created_at DESC, i.id DESC LIMIT 1";

/// 同一条记录、同一内容仍存在时才能写入结果；从不更新原记录或其使用时间。
pub(super) async fn save(
    connection: &mut SqliteConnection,
    id: &str,
    hash: &str,
    outcome: &Outcome,
) -> Result<()> {
    let (status, text, language) = match outcome {
        Outcome::Done { text, language } => ("done", text.as_str(), Some(language.as_str())),
        Outcome::Failed { .. } => ("failed", "", None),
        Outcome::Skipped { .. } => ("skipped", "", None),
    };
    sqlx::query("INSERT INTO image_texts(item_id, status, text, language, attempts, created_at, updated_at) SELECT id, ?, ?, ?, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now'), strftime('%Y-%m-%dT%H:%M:%fZ','now') FROM clipboard_items WHERE id = ? AND kind = 'image' AND content_hash = ? ON CONFLICT(item_id) DO UPDATE SET status = excluded.status, text = excluded.text, language = excluded.language, attempts = MIN(image_texts.attempts + 1, 3), updated_at = excluded.updated_at")
        .bind(status).bind(text).bind(language).bind(id).bind(hash).execute(connection).await?;
    Ok(())
}
