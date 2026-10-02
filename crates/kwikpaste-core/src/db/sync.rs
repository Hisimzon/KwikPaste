//! 局域网同步的本机采集序号（迁移 0006）：本机复制的记录按复制先后编号，对端按「上次收到的最大序号」补齐。
//!
//! 序号来自单行计数器 `sync_counter`，只增不减；删掉最新的记录后序号也不会重新发出去。
//! 只有本机采集（监听、手动读取、宿主入库）会拿到序号；同步收到的、备份导入的记录 `sync_seq` 为空，
//! 永远不会被当成本机复制的内容补齐给别的设备。

use anyhow::Context;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

use crate::error::Result;

/// 给一条本机采集的记录发下一个序号（复制已有内容时也重新编号，补齐时按「最近用过」排在后面）。
/// 记录不存在时返回 `None`。
pub async fn assign_sync_seq(pool: &SqlitePool, item_id: &str) -> Result<Option<i64>> {
    let mut tx = pool
        .begin()
        .await
        .context("failed to begin sync sequence update")?;
    let seq: i64 = sqlx::query_scalar(
        "UPDATE sync_counter SET value = value + 1, updated_at = ? WHERE id = 1 RETURNING value",
    )
    .bind(Utc::now())
    .fetch_one(&mut *tx)
    .await
    .context("failed to advance the sync counter")?;
    let updated = sqlx::query("UPDATE clipboard_items SET sync_seq = ? WHERE id = ?")
        .bind(seq)
        .bind(item_id)
        .execute(&mut *tx)
        .await
        .context("failed to assign the sync sequence")?
        .rows_affected();
    if updated == 0 {
        tx.rollback()
            .await
            .context("failed to roll back sync sequence update")?;
        return Ok(None);
    }
    tx.commit()
        .await
        .context("failed to commit sync sequence update")?;
    Ok(Some(seq))
}

/// 计数器当前值：已经发出去的最大序号。
pub async fn sync_counter(pool: &SqlitePool) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT value FROM sync_counter WHERE id = 1")
            .fetch_one(pool)
            .await
            .context("failed to read the sync counter")?,
    )
}

/// 一页补齐：序号大于 `after`、`cutoff` 之后用过的记录，按序号从小到大，最多 `page` 条；
/// 总量只取最新的 `total` 条（更早的跳过）。返回 `(id, 序号)` 与后面是否还有。
pub async fn catch_up_page(
    pool: &SqlitePool,
    after: i64,
    cutoff: DateTime<Utc>,
    total: i64,
    page: i64,
) -> Result<(Vec<(String, i64)>, bool)> {
    let floor: Option<i64> = sqlx::query_scalar(
        "SELECT sync_seq FROM clipboard_items \
         WHERE sync_seq > ? AND COALESCE(last_used_at, updated_at) >= ? \
         ORDER BY sync_seq DESC LIMIT 1 OFFSET ?",
    )
    .bind(after)
    .bind(cutoff)
    .bind((total - 1).max(0))
    .fetch_optional(pool)
    .await
    .context("failed to bound the catch-up window")?;
    let start = floor.map_or(after, |floor| after.max(floor - 1));

    let mut rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT id, sync_seq FROM clipboard_items \
         WHERE sync_seq > ? AND COALESCE(last_used_at, updated_at) >= ? \
         ORDER BY sync_seq ASC LIMIT ?",
    )
    .bind(start)
    .bind(cutoff)
    .bind(page + 1)
    .fetch_all(pool)
    .await
    .context("failed to read the catch-up page")?;
    let more = rows.len() as i64 > page;
    rows.truncate(page.max(0) as usize);
    Ok((rows, more))
}

/// 覆盖导入备份后调用：导入进来的记录都不是本机采集的，清掉它们的序号；计数器不回退，
/// 已配对设备记着的水位线仍然有效。
pub async fn reset_after_import(pool: &SqlitePool, counter_at_least: i64) -> Result<()> {
    let mut tx = pool
        .begin()
        .await
        .context("failed to begin sync state reset")?;
    sqlx::query("UPDATE clipboard_items SET sync_seq = NULL WHERE sync_seq IS NOT NULL")
        .execute(&mut *tx)
        .await
        .context("failed to clear imported sync sequences")?;
    sqlx::query("UPDATE sync_counter SET value = MAX(value, ?), updated_at = ? WHERE id = 1")
        .bind(counter_at_least)
        .bind(Utc::now())
        .execute(&mut *tx)
        .await
        .context("failed to keep the sync counter")?;
    tx.commit()
        .await
        .context("failed to commit sync state reset")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;
    use crate::db::items::{content_hash, insert_item};
    use crate::db::models::{ClipboardItem, ClipboardKind, Platform};
    use crate::db::test_support::memory_pool;

    fn item(id: &str, used: DateTime<Utc>) -> ClipboardItem {
        let content = format!("content {id}");
        ClipboardItem {
            id: id.to_owned(),
            kind: ClipboardKind::Text,
            sub_kind: None,
            group_id: None,
            source_app_id: None,
            content_hash: content_hash(ClipboardKind::Text, &content),
            content,
            search_text: None,
            summary: None,
            file_types: None,
            size: None,
            width: None,
            height: None,
            use_count: 1,
            is_favorite: false,
            is_pinned: false,
            is_sensitive: false,
            platform: Platform::Windows,
            note: None,
            created_at: used,
            updated_at: used,
            origin_device_id: None,
            source_app_name: None,
            source_app_icon_file: None,
        }
    }

    #[tokio::test]
    async fn sequences_only_grow_even_after_the_newest_record_is_deleted() {
        let pool = memory_pool().await;
        let now = Utc::now();
        insert_item(&pool, &item("a", now)).await.unwrap();
        insert_item(&pool, &item("b", now)).await.unwrap();

        assert_eq!(sync_counter(&pool).await.unwrap(), 0);
        assert_eq!(assign_sync_seq(&pool, "a").await.unwrap(), Some(1));
        assert_eq!(assign_sync_seq(&pool, "b").await.unwrap(), Some(2));
        sqlx::query("DELETE FROM clipboard_items WHERE id = 'b'")
            .execute(&pool)
            .await
            .unwrap();
        insert_item(&pool, &item("c", now)).await.unwrap();
        assert_eq!(assign_sync_seq(&pool, "c").await.unwrap(), Some(3));
        // 复制已有内容时重新编号。
        assert_eq!(assign_sync_seq(&pool, "a").await.unwrap(), Some(4));
        assert_eq!(assign_sync_seq(&pool, "missing").await.unwrap(), None);
        assert_eq!(sync_counter(&pool).await.unwrap(), 4);
    }

    #[tokio::test]
    async fn catch_up_pages_respect_the_window_and_the_total() {
        let pool = memory_pool().await;
        let now = Utc::now();
        insert_item(&pool, &item("old", now - Duration::days(10)))
            .await
            .unwrap();
        assign_sync_seq(&pool, "old").await.unwrap();
        for index in 0..7 {
            let id = format!("n{index}");
            insert_item(&pool, &item(&id, now)).await.unwrap();
            assign_sync_seq(&pool, &id).await.unwrap();
        }
        // 没有序号的（同步收到的、导入的）永远不补齐。
        insert_item(&pool, &item("received", now)).await.unwrap();
        let cutoff = now - Duration::days(7);

        let (page, more) = catch_up_page(&pool, 0, cutoff, 500, 3).await.unwrap();
        let ids: Vec<_> = page.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["n0", "n1", "n2"]);
        assert!(more);
        let (page, more) = catch_up_page(&pool, page[2].1, cutoff, 500, 3)
            .await
            .unwrap();
        assert_eq!(page.len(), 3);
        assert!(more);
        let (page, more) = catch_up_page(&pool, page[2].1, cutoff, 500, 3)
            .await
            .unwrap();
        assert_eq!(page.len(), 1);
        assert!(!more);

        // 总量只取最新的 4 条。
        let (page, _) = catch_up_page(&pool, 0, cutoff, 4, 10).await.unwrap();
        let ids: Vec<_> = page.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["n3", "n4", "n5", "n6"]);
    }

    #[tokio::test]
    async fn import_reset_clears_sequences_but_keeps_the_counter() {
        let pool = memory_pool().await;
        let now = Utc::now();
        insert_item(&pool, &item("a", now)).await.unwrap();
        assign_sync_seq(&pool, "a").await.unwrap();

        reset_after_import(&pool, 10).await.unwrap();

        let seq: Option<i64> =
            sqlx::query_scalar("SELECT sync_seq FROM clipboard_items WHERE id = 'a'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(seq, None);
        assert_eq!(sync_counter(&pool).await.unwrap(), 10);
        reset_after_import(&pool, 3).await.unwrap();
        assert_eq!(sync_counter(&pool).await.unwrap(), 10);
    }
}
