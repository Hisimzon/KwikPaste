//! 派生队列与结果写入；只使用会话专用连接，不占用主池的页缓存。
use super::protocol::{Outcome, TextLine, MAX_TEXT_CHARS};
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
    let (status, text, language, lines) = match outcome {
        Outcome::Done { lines, language } => {
            let lines = bounded_lines(lines);
            let text = lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            (
                "done",
                text,
                Some(language.as_str()),
                Some(serde_json::to_string(&lines)?),
            )
        }
        Outcome::Failed { .. } => ("failed", String::new(), None, None),
        Outcome::Skipped { .. } => ("skipped", String::new(), None, None),
    };
    sqlx::query("INSERT INTO image_texts(item_id, status, text, language, lines, attempts, created_at, updated_at) SELECT id, ?, ?, ?, ?, 1, strftime('%Y-%m-%dT%H:%M:%fZ','now'), strftime('%Y-%m-%dT%H:%M:%fZ','now') FROM clipboard_items WHERE id = ? AND kind = 'image' AND content_hash = ? ON CONFLICT(item_id) DO UPDATE SET status = CASE WHEN image_texts.status = 'done' AND excluded.status = 'failed' THEN image_texts.status ELSE excluded.status END, text = CASE WHEN image_texts.status = 'done' AND excluded.status = 'failed' THEN image_texts.text ELSE excluded.text END, language = CASE WHEN image_texts.status = 'done' AND excluded.status = 'failed' THEN image_texts.language ELSE excluded.language END, lines = CASE WHEN image_texts.status = 'done' AND excluded.status = 'failed' THEN image_texts.lines ELSE excluded.lines END, attempts = MIN(image_texts.attempts + 1, 3), updated_at = excluded.updated_at")
        .bind(status).bind(text).bind(language).bind(lines).bind(id).bind(hash).execute(connection).await?;
    Ok(())
}

/// 连同换行计数截断行文本，使派生原文与布局文本始终一致。
fn bounded_lines(lines: &[TextLine]) -> Vec<TextLine> {
    let mut result = Vec::new();
    let mut remaining = MAX_TEXT_CHARS;
    for line in lines {
        let separator = usize::from(!result.is_empty());
        if remaining <= separator {
            break;
        }
        let text: String = line.text.chars().take(remaining - separator).collect();
        remaining -= separator + text.chars().count();
        result.push(TextLine {
            text,
            quad: line.quad,
        });
    }
    result
}
