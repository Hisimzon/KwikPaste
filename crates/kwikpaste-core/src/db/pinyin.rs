//! 拼音检索索引:把中文内容转写成拼音写进 `search_pinyin` / `search_pinyin_initials`,
//! 让「输入拼音搜中文」走 LIKE 命中。
//!
//! 只索引**开头一段**:整体拼音对全文检索没有价值(没人会敲一整段文档的拼音),
//! 存储与 LIKE 扫描却按字符数线性变贵。备注例外——它是用户主动写的,通常很短,
//! 单独给一份预算。
//!
//! 索引内容由 [`build_search_index`] 决定,只依赖 `search_text` / `note` 两列,
//! 所以重算时用这两列做乐观校验(见 [`update_items`]),不会覆盖并发写入的新值。

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{LazyLock, Mutex};

use anyhow::Context;
use pinyin::ToPinyin;
use sqlx::{QueryBuilder, Sqlite, SqlitePool};
use tokio::sync::Notify;

use crate::error::Result;

/// 单个字段只扫描开头这一段字符。
const SCAN_CHARS: usize = 200;
/// 单条最多转写多少个汉字。
const MAX_PINYIN_CHARS: usize = 120;
/// 首字母只取开头这一段:正文靠后的字凑出的首字母串对用户没有意义,只会误命中。
const INITIALS_SCAN_CHARS: usize = 60;
/// 回填/重算一次取多少行。
const BATCH_SIZE: i64 = 100;
/// 单次 worker 唤醒最多处理多少个待更新 id。
const UPDATE_CHUNK: usize = 100;

/// 生成两列拼音索引:`search_pinyin` 是紧凑串加音节串,`search_pinyin_initials` 是首字母串。
///
/// 没有可转写内容时返回空串而不是 `NULL`:两列都是 `NULL` 专门表示「还没算过」,
/// 回填靠它挑待办行;这里也写 `NULL` 的话,那些行会被每轮回填反复选中。
pub(crate) fn build_search_index(
    search_text: Option<&str>,
    note: Option<&str>,
) -> (String, String) {
    let mut compact = String::new();
    let mut spaced = String::new();
    let mut initials = String::new();
    let mut syllables = 0usize;

    // 备注先转写:它是更明确的检索目标,不能被长正文吃掉转写预算。
    // 首字母只取正文开头一段,备注不受这个上限约束。
    let values = [(note, usize::MAX), (search_text, INITIALS_SCAN_CHARS)];

    for (text, initials_limit) in values {
        let Some(text) = text else { continue };
        let mut value_initials = String::new();

        for (index, character) in text.chars().take(SCAN_CHARS).enumerate() {
            if syllables == MAX_PINYIN_CHARS {
                break;
            }

            let Some(pinyin) = character.to_pinyin() else {
                continue;
            };
            let plain = pinyin.plain();

            compact.push_str(plain);
            if !spaced.is_empty() {
                spaced.push(' ');
            }
            spaced.push_str(plain);
            if index < initials_limit {
                if let Some(initial) = plain.chars().next() {
                    value_initials.push(initial);
                }
            }
            syllables += 1;
        }

        if value_initials.is_empty() {
            continue;
        }
        if !initials.is_empty() {
            initials.push(' ');
        }
        initials.push_str(&value_initials);
    }

    // 紧凑串与音节串都留:用户可能敲 `beizhu`,也可能敲 `bei zhu`。
    let search_pinyin = if compact.is_empty() {
        String::new()
    } else if spaced.is_empty() {
        compact
    } else {
        format!("{compact} {spaced}")
    };

    (search_pinyin, initials)
}

/// 补齐所有缺失索引的行(迁移后、覆盖导入/切换存储之后)。
pub(crate) async fn backfill(pool: &SqlitePool) -> Result<()> {
    let mut last_rowid = 0i64;

    loop {
        let rows = sqlx::query_as::<_, (i64, Option<String>, Option<String>)>(
            "SELECT rowid, search_text, note FROM clipboard_items
             WHERE rowid > ? AND (search_pinyin IS NULL OR search_pinyin_initials IS NULL)
             ORDER BY rowid LIMIT ?",
        )
        .bind(last_rowid)
        .bind(BATCH_SIZE)
        .fetch_all(pool)
        .await
        .context("failed to load clipboard items for pinyin backfill")?;

        if rows.is_empty() {
            return Ok(());
        }

        last_rowid = rows.last().map(|row| row.0).unwrap_or(last_rowid);
        write_rows(pool, rows, "backfill").await?;
    }
}

/// 合并高频单条重算请求,由后台 worker 分批处理。
pub(crate) fn queue_update(pool: &SqlitePool, id: &str) {
    {
        let mut pending = lock_pending();
        pending.pool = Some(pool.clone());
        pending.ids.insert(id.to_owned());
    }

    start_worker();
    NOTIFY.notify_one();
}

/// 合并全量回填请求,由后台 worker 分批补齐所有缺失索引。
pub(crate) fn queue_backfill(pool: &SqlitePool) {
    {
        let mut pending = lock_pending();
        pending.full_backfill = true;
        pending.pool = Some(pool.clone());
    }

    start_worker();
    NOTIFY.notify_one();
}

#[derive(Default)]
struct Pending {
    full_backfill: bool,
    ids: HashSet<String>,
    pool: Option<SqlitePool>,
}

impl Pending {
    fn take(&mut self) -> Option<(SqlitePool, bool, Vec<String>)> {
        let pool = self.pool.take()?;
        let full_backfill = std::mem::take(&mut self.full_backfill);
        let ids = self.ids.drain().collect();
        Some((pool, full_backfill, ids))
    }
}

static PENDING: LazyLock<Mutex<Pending>> = LazyLock::new(|| Mutex::new(Pending::default()));
static NOTIFY: LazyLock<Notify> = LazyLock::new(Notify::new);
static WORKER_STARTED: AtomicBool = AtomicBool::new(false);

fn lock_pending() -> std::sync::MutexGuard<'static, Pending> {
    PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn start_worker() {
    if !WORKER_STARTED.swap(true, Ordering::AcqRel) {
        tokio::spawn(run_worker());
    }
}

/// 索引是检索加速用的派生数据,失败只记日志:不阻断入库,也不影响正文数据。
async fn run_worker() {
    loop {
        NOTIFY.notified().await;

        // 先把待办取出来再 await:锁守卫不能跨 await,否则 worker future 不是 Send。
        loop {
            let Some((pool, full_backfill, ids)) = lock_pending().take() else {
                break;
            };

            if full_backfill {
                if let Err(err) = backfill(&pool).await {
                    log::warn!("failed to backfill pinyin search index: {err:#}");
                }
                continue;
            }

            for chunk in ids.chunks(UPDATE_CHUNK) {
                if let Err(err) = update_items(&pool, chunk).await {
                    log::warn!("failed to update pinyin search index: {err:#}");
                }
            }
        }
    }
}

/// 重算单条/一批 id 的拼音索引。单测直接调用它预置索引;写入前按 `search_text` / `note` 乐观校验,
/// 不会覆盖并发写入的新值。
pub(crate) async fn update_items(pool: &SqlitePool, ids: &[String]) -> Result<()> {
    if ids.is_empty() {
        return Ok(());
    }

    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT rowid, search_text, note FROM clipboard_items WHERE id IN (",
    );
    for (index, id) in ids.iter().enumerate() {
        if index > 0 {
            query.push(", ");
        }
        query.push_bind(id.as_str());
    }
    query.push(")");

    let rows = query
        .build_query_as::<(i64, Option<String>, Option<String>)>()
        .fetch_all(pool)
        .await
        .context("failed to load clipboard items for the pinyin index")?;

    write_rows(pool, rows, "update").await
}

/// 逐行重算并写回。`search_text` / `note` 是索引的全部输入,写入前再比一次,
/// 避免把并发写入的新正文/备注的索引覆盖成旧值。
async fn write_rows(
    pool: &SqlitePool,
    rows: Vec<(i64, Option<String>, Option<String>)>,
    label: &str,
) -> Result<()> {
    let indexed = tokio::task::spawn_blocking(move || {
        rows.into_iter()
            .map(|(rowid, search_text, note)| {
                let index = build_search_index(search_text.as_deref(), note.as_deref());
                (rowid, search_text, note, index)
            })
            .collect::<Vec<_>>()
    })
    .await
    .context("pinyin index worker panicked")?;

    for (rowid, search_text, note, (search_pinyin, search_pinyin_initials)) in indexed {
        sqlx::query(
            "UPDATE clipboard_items
             SET search_pinyin = ?, search_pinyin_initials = ?
             WHERE rowid = ? AND search_text IS ? AND note IS ?",
        )
        .bind(search_pinyin)
        .bind(search_pinyin_initials)
        .bind(rowid)
        .bind(search_text)
        .bind(note)
        .execute(pool)
        .await
        .with_context(|| format!("failed to write the pinyin index ({label})"))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{build_search_index, INITIALS_SCAN_CHARS};

    #[test]
    fn index_keeps_compact_spaced_and_initial_forms() {
        let (pinyin, initials) = build_search_index(Some("备注是对的"), None);

        assert!(pinyin.contains("beizhushiduide"));
        assert!(pinyin.contains("bei zhu shi dui de"));
        assert_eq!(initials, "bzsdd");
    }

    #[test]
    fn content_without_hanzi_gets_empty_indexes_not_null() {
        // 空串表示「算过了、没有拼音」;`NULL` 只留给回填待办,不能在这里出现。
        assert_eq!(
            build_search_index(Some("hello world"), Some("plain")),
            (String::new(), String::new())
        );
        assert_eq!(
            build_search_index(None, None),
            (String::new(), String::new())
        );
    }

    #[test]
    fn body_initials_stop_after_the_scan_window() {
        let body = format!("{}明", "字".repeat(INITIALS_SCAN_CHARS));
        let (_, initials) = build_search_index(Some(&body), Some("备注"));

        assert!(initials.starts_with("bz "));
        assert!(!initials.contains('m'));
    }

    #[test]
    fn a_short_note_is_indexed_even_when_the_body_is_long() {
        let body = "汉".repeat(1000);
        let (pinyin, _) = build_search_index(Some(&body), Some("备注"));

        assert!(pinyin.contains("beizhu"));
    }
}
