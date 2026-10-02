//! 自动清理用到的删除与统计：保留规则按「最后使用时间」过期，条数与存储上限按最久未用的顺序裁剪。
//!
//! 收藏、置顶、放进自定义分组或写了备注的记录都算用户整理过，自动清理一律不动（见 [`removable!`]）。
//! 删除函数都接收单个连接，调用方可以放进事务里一起提交，也可以预演后回滚。

use anyhow::Context;
use chrono::{DateTime, Utc};
use sqlx::{QueryBuilder, Sqlite, SqliteConnection, SqlitePool};

use crate::db::items::{absorb_deleted, CleanupOutcome, DeletedRow};
use crate::db::models::ClipboardKind;
use crate::db::overview::ContentCategory;
use crate::error::Result;

/// 自动清理可以删除的普通记录。宏形式便于 `concat!` 拼出静态 SQL。
macro_rules! removable {
    () => {
        "is_pinned = 0 AND is_favorite = 0 AND group_id IS NULL AND (note IS NULL OR note = '')"
    };
}

/// 按存储上限清理时每条 DELETE 绑定的 id 数，远低于 SQLite 的绑定参数上限。
const DELETE_BATCH: usize = 500;

/// 一条已编译的保留规则：条件全部满足才算命中；命中后最后使用时间早于 `cutoff` 即过期，
/// `cutoff = None` 表示命中的记录不按时间清理。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RulePlan {
    /// 空 = 不限内容类别。
    pub categories: Vec<ContentCategory>,
    /// 内容大小下限（不含）；文件记录没有大小，设了下限就不会命中。
    pub min_size_bytes: Option<u64>,
    /// 空 = 不限来源应用。
    pub source_app_ids: Vec<String>,
    pub sensitive_only: bool,
    /// 只命中采集后再没用过的记录：最后使用时间仍等于采集时间。
    pub unused_only: bool,
    pub cutoff: Option<DateTime<Utc>>,
}

/// 按时间清理的完整计划：规则自上而下匹配，记录按第一条命中的规则过期，都没命中时按 `fallback_cutoff`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgePlan {
    pub rules: Vec<RulePlan>,
    pub fallback_cutoff: Option<DateTime<Utc>>,
}

impl AgePlan {
    /// 所有截止时间里最晚的一个：过期记录的最后使用时间必然早于它，删除前先用它走索引缩小范围。
    fn latest_cutoff(&self) -> Option<DateTime<Utc>> {
        self.rules
            .iter()
            .map(|rule| rule.cutoff)
            .chain([self.fallback_cutoff])
            .flatten()
            .max()
    }
}

/// 一条规则（或兜底）当前匹配的普通记录数，以及其中已经过期的条数。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuleMatchStats {
    pub matched: u64,
    pub expired: u64,
}

/// 删除按保留规则已过期的普通记录。
pub async fn delete_expired(conn: &mut SqliteConnection, plan: &AgePlan) -> Result<CleanupOutcome> {
    let mut outcome = CleanupOutcome::default();
    let Some(latest) = plan.latest_cutoff() else {
        return Ok(outcome);
    };

    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(concat!(
        "DELETE FROM clipboard_items WHERE ",
        removable!(),
        " AND last_used_at < "
    ));
    qb.push_bind(latest).push(" AND last_used_at < ");
    push_cutoff(&mut qb, plan);
    qb.push(" RETURNING kind, content, size");

    let rows = qb
        .build_query_as::<DeletedRow>()
        .fetch_all(&mut *conn)
        .await
        .context("failed to delete expired clipboard items")?;
    absorb_deleted(&mut outcome, rows);
    Ok(outcome)
}

/// 普通记录超过 `max_count` 条时，按最后使用时间保留最近的 `max_count` 条，其余删除；`0` 表示不限。
pub async fn delete_over_count(
    conn: &mut SqliteConnection,
    max_count: u32,
) -> Result<CleanupOutcome> {
    let mut outcome = CleanupOutcome::default();
    if max_count == 0 {
        return Ok(outcome);
    }

    // SQLite 中 `LIMIT -1 OFFSET n` 表示「跳过前 n 条，剩下全要」。
    let rows = sqlx::query_as::<_, DeletedRow>(concat!(
        "DELETE FROM clipboard_items WHERE id IN ( \
             SELECT id FROM clipboard_items WHERE ",
        removable!(),
        " ORDER BY last_used_at DESC, created_at DESC LIMIT -1 OFFSET ? \
         ) RETURNING kind, content, size"
    ))
    .bind(i64::from(max_count))
    .fetch_all(&mut *conn)
    .await
    .context("failed to delete clipboard items over max count")?;
    absorb_deleted(&mut outcome, rows);
    Ok(outcome)
}

/// 按存储上限清理：从最久未用的普通记录开始删，直到预估释放量达到 `bytes_to_free`。
///
/// 文本按 `size` 估算，图片由 `image_bytes` 按落盘文件估算。估算只决定删到哪一条为止：
/// 调用方下一轮会重新统计实际占用，估少了多删几条最久未用的记录，估多了下一轮再补删。
/// 删光全部普通记录也达不到目标时（占用主要来自受保护记录或缓存）不删任何记录，避免白白清空历史。
pub async fn delete_least_recent_until(
    conn: &mut SqliteConnection,
    bytes_to_free: u64,
    image_bytes: impl Fn(&str) -> u64,
) -> Result<CleanupOutcome> {
    let mut outcome = CleanupOutcome::default();
    if bytes_to_free == 0 {
        return Ok(outcome);
    }

    // 只为图片取 content（落盘文件名），避免把大段文本整批读进内存。
    let candidates =
        sqlx::query_as::<_, (String, ClipboardKind, Option<String>, Option<i64>)>(concat!(
            "SELECT id, kind, CASE WHEN kind = 'image' THEN content END, size \
             FROM clipboard_items WHERE ",
            removable!(),
            " ORDER BY last_used_at ASC, created_at ASC"
        ))
        .fetch_all(&mut *conn)
        .await
        .context("failed to list clipboard items for storage cleanup")?;

    let mut freed = 0_u64;
    let mut ids = Vec::new();
    for (id, kind, image_file, size) in candidates {
        if freed >= bytes_to_free {
            break;
        }

        freed += match (kind, image_file) {
            (ClipboardKind::Image, Some(file_name)) => image_bytes(&file_name),
            _ => size.map_or(0, |size| size.max(0) as u64),
        };
        ids.push(id);
    }

    if freed < bytes_to_free {
        return Ok(outcome);
    }

    for batch in ids.chunks(DELETE_BATCH) {
        // 选取与删除之间用户可能刚收藏、置顶或整理了某条，删除时再校验一次保护条件。
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(concat!(
            "DELETE FROM clipboard_items WHERE ",
            removable!(),
            " AND id IN ("
        ));
        let mut separated = qb.separated(", ");
        for id in batch {
            separated.push_bind(id);
        }
        separated.push_unseparated(") RETURNING kind, content, size");

        let rows = qb
            .build_query_as::<DeletedRow>()
            .fetch_all(&mut *conn)
            .await
            .context("failed to delete clipboard items over storage limit")?;
        absorb_deleted(&mut outcome, rows);
    }

    Ok(outcome)
}

/// 统计每条规则当前匹配与已过期的普通记录；返回按规则顺序的结果和兜底（未命中任何规则）的结果。
pub async fn rule_match_stats(
    conn: &mut SqliteConnection,
    plan: &AgePlan,
) -> Result<(Vec<RuleMatchStats>, RuleMatchStats)> {
    let mut qb: QueryBuilder<Sqlite> =
        QueryBuilder::new("SELECT rule_index, COUNT(*), COALESCE(SUM(expired), 0) FROM (SELECT ");
    push_rule_index(&mut qb, plan);
    qb.push(" AS rule_index, CASE WHEN last_used_at < ");
    push_cutoff(&mut qb, plan);
    qb.push(concat!(
        " THEN 1 ELSE 0 END AS expired FROM clipboard_items WHERE ",
        removable!(),
        ") GROUP BY rule_index"
    ));

    let rows = qb
        .build_query_as::<(i64, i64, i64)>()
        .fetch_all(&mut *conn)
        .await
        .context("failed to count clipboard items per retention rule")?;

    let mut per_rule = vec![RuleMatchStats::default(); plan.rules.len()];
    let mut fallback = RuleMatchStats::default();
    for (index, matched, expired) in rows {
        let stats = RuleMatchStats {
            matched: matched.max(0) as u64,
            expired: expired.max(0) as u64,
        };
        match usize::try_from(index)
            .ok()
            .and_then(|index| per_rule.get_mut(index))
        {
            Some(slot) => *slot = stats,
            None => fallback = stats,
        }
    }

    Ok((per_rule, fallback))
}

/// 受保护、自动清理不会删除的记录条数。
pub async fn count_protected(conn: &mut SqliteConnection) -> Result<u64> {
    let count: i64 = sqlx::query_scalar(concat!(
        "SELECT COUNT(*) FROM clipboard_items WHERE NOT (",
        removable!(),
        ")"
    ))
    .fetch_one(&mut *conn)
    .await
    .context("failed to count protected clipboard items")?;

    Ok(count.max(0) as u64)
}

/// 数据库处于增量 auto_vacuum 模式时，把删行留下的空闲页还给文件系统；其它模式下什么也不做。
pub async fn release_free_pages(pool: &SqlitePool) -> Result<()> {
    sqlx::query("PRAGMA incremental_vacuum")
        .execute(pool)
        .await
        .context("failed to release sqlite free pages")?;
    Ok(())
}

/// 压缩数据库文件。早期版本建库时没有开启 auto_vacuum，删行后文件不会变小；
/// 这里整库 VACUUM 一次并切到增量模式，之后每次自动清理都能直接收缩文件。
pub async fn compact(pool: &SqlitePool) -> Result<()> {
    let mut conn = pool
        .acquire()
        .await
        .context("failed to acquire connection for compaction")?;
    let mode: i64 = sqlx::query_scalar("PRAGMA auto_vacuum")
        .fetch_one(&mut *conn)
        .await
        .context("failed to read sqlite auto_vacuum mode")?;

    if mode == AUTO_VACUUM_INCREMENTAL {
        sqlx::query("PRAGMA incremental_vacuum")
            .execute(&mut *conn)
            .await
            .context("failed to release sqlite free pages")?;
    } else {
        sqlx::query("PRAGMA auto_vacuum = INCREMENTAL")
            .execute(&mut *conn)
            .await
            .context("failed to switch sqlite auto_vacuum mode")?;
        sqlx::query("VACUUM")
            .execute(&mut *conn)
            .await
            .context("failed to vacuum sqlite database")?;
    }

    // WAL 模式下收缩要等检查点写回主文件；有读者占用时 SQLite 只做部分检查点，不报错。
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(&mut *conn)
        .await
        .context("failed to checkpoint sqlite wal")?;
    Ok(())
}

/// `PRAGMA auto_vacuum` 返回的增量模式编号。
const AUTO_VACUUM_INCREMENTAL: i64 = 2;

/// 拼出一条规则的匹配条件（带括号）；没有任何条件时匹配全部记录。
fn push_rule_match(qb: &mut QueryBuilder<Sqlite>, rule: &RulePlan) {
    qb.push("(1");

    if !rule.categories.is_empty() {
        qb.push(" AND (");
        for (index, category) in rule.categories.iter().enumerate() {
            if index > 0 {
                qb.push(" OR ");
            }
            qb.push("(").push(category.sql_condition()).push(")");
        }
        qb.push(")");
    }

    if let Some(min_size) = rule.min_size_bytes {
        qb.push(" AND size > ")
            .push_bind(i64::try_from(min_size).unwrap_or(i64::MAX));
    }

    if !rule.source_app_ids.is_empty() {
        qb.push(" AND source_app_id IN (");
        let mut separated = qb.separated(", ");
        for app_id in &rule.source_app_ids {
            separated.push_bind(app_id.clone());
        }
        separated.push_unseparated(")");
    }

    if rule.sensitive_only {
        qb.push(" AND is_sensitive = 1");
    }

    if rule.unused_only {
        qb.push(" AND last_used_at = created_at");
    }

    qb.push(")");
}

/// 拼出「记录按第一条命中规则的截止时间」表达式；没有规则时直接是兜底截止时间。
fn push_cutoff(qb: &mut QueryBuilder<Sqlite>, plan: &AgePlan) {
    if plan.rules.is_empty() {
        qb.push("(").push_bind(plan.fallback_cutoff).push(")");
        return;
    }

    qb.push("(CASE");
    for rule in &plan.rules {
        qb.push(" WHEN ");
        push_rule_match(qb, rule);
        qb.push(" THEN ").push_bind(rule.cutoff);
    }
    qb.push(" ELSE ")
        .push_bind(plan.fallback_cutoff)
        .push(" END)");
}

/// 拼出「记录命中的第一条规则序号」表达式，兜底为 `-1`。
fn push_rule_index(qb: &mut QueryBuilder<Sqlite>, plan: &AgePlan) {
    if plan.rules.is_empty() {
        qb.push("-1");
        return;
    }

    qb.push("(CASE");
    for (index, rule) in plan.rules.iter().enumerate() {
        qb.push(" WHEN ");
        push_rule_match(qb, rule);
        qb.push(" THEN ").push(index.to_string());
    }
    qb.push(" ELSE -1 END)");
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;
    use crate::db::test_support::memory_pool;

    /// 测试用记录；时间都以 `NOW` 为基准往前推，单位小时。
    struct Row<'a> {
        id: &'a str,
        kind: &'a str,
        sub_kind: Option<&'a str>,
        size: Option<i64>,
        source_app_id: Option<&'a str>,
        is_favorite: bool,
        is_pinned: bool,
        is_sensitive: bool,
        group_id: Option<&'a str>,
        note: Option<&'a str>,
        created_hours_ago: i64,
        used_hours_ago: i64,
    }

    impl<'a> Row<'a> {
        fn text(id: &'a str, used_hours_ago: i64) -> Self {
            Self {
                id,
                kind: "text",
                sub_kind: None,
                size: Some(10),
                source_app_id: None,
                is_favorite: false,
                is_pinned: false,
                is_sensitive: false,
                group_id: None,
                note: None,
                created_hours_ago: used_hours_ago,
                used_hours_ago,
            }
        }

        fn image(id: &'a str, size: i64, used_hours_ago: i64) -> Self {
            Self {
                kind: "image",
                size: Some(size),
                ..Self::text(id, used_hours_ago)
            }
        }
    }

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    fn hours_ago(hours: i64) -> DateTime<Utc> {
        now() - Duration::hours(hours)
    }

    async fn insert(pool: &SqlitePool, row: Row<'_>) {
        if let Some(app_id) = row.source_app_id {
            sqlx::query(
                "INSERT OR IGNORE INTO clipboard_apps \
                 (id, name, icon_file, platform, created_at, updated_at) \
                 VALUES (?, ?, NULL, 'windows', ?, ?)",
            )
            .bind(app_id)
            .bind(app_id)
            .bind(now())
            .bind(now())
            .execute(pool)
            .await
            .unwrap();
        }
        if let Some(group_id) = row.group_id {
            sqlx::query(
                "INSERT OR IGNORE INTO clipboard_groups \
                 (id, name, icon, is_hidden, sort_order, created_at, updated_at) \
                 VALUES (?, ?, 'i-lets-icons:folder', 0, 0, ?, ?)",
            )
            .bind(group_id)
            .bind(group_id)
            .bind(now())
            .bind(now())
            .execute(pool)
            .await
            .unwrap();
        }

        let created_at = hours_ago(row.created_hours_ago);
        let last_used_at = hours_ago(row.used_hours_ago);
        sqlx::query(
            "INSERT INTO clipboard_items \
             (id, kind, sub_kind, group_id, source_app_id, content, content_hash, size, \
              use_count, is_favorite, is_pinned, is_sensitive, platform, note, \
              created_at, updated_at, last_used_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?, 'windows', ?, ?, ?, ?)",
        )
        .bind(row.id)
        .bind(row.kind)
        .bind(row.sub_kind)
        .bind(row.group_id)
        .bind(row.source_app_id)
        .bind(format!("{}.png", row.id))
        .bind(row.id)
        .bind(row.size)
        .bind(row.is_favorite)
        .bind(row.is_pinned)
        .bind(row.is_sensitive)
        .bind(row.note)
        .bind(created_at)
        .bind(created_at)
        .bind(last_used_at)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn remaining(pool: &SqlitePool) -> Vec<String> {
        sqlx::query_scalar("SELECT id FROM clipboard_items ORDER BY id")
            .fetch_all(pool)
            .await
            .unwrap()
    }

    fn fallback(hours: i64) -> AgePlan {
        AgePlan {
            rules: Vec::new(),
            fallback_cutoff: Some(hours_ago(hours)),
        }
    }

    #[tokio::test]
    async fn expiry_follows_last_use_instead_of_capture_time() {
        let pool = memory_pool().await;
        insert(&pool, Row::text("stale", 48)).await;
        // 两天前采集、刚刚还在用的记录不算过期。
        insert(
            &pool,
            Row {
                created_hours_ago: 48,
                ..Row::text("reused", 1)
            },
        )
        .await;

        let outcome = delete_expired(&mut pool.acquire().await.unwrap(), &fallback(24))
            .await
            .unwrap();

        assert_eq!(outcome.removed, 1);
        assert_eq!(outcome.content_bytes, 10);
        assert_eq!(remaining(&pool).await, ["reused"]);
    }

    #[tokio::test]
    async fn curated_records_are_never_cleaned() {
        let pool = memory_pool().await;
        insert(&pool, Row::text("plain", 100)).await;
        insert(
            &pool,
            Row {
                is_favorite: true,
                ..Row::text("favorite", 100)
            },
        )
        .await;
        insert(
            &pool,
            Row {
                is_pinned: true,
                ..Row::text("pinned", 100)
            },
        )
        .await;
        insert(
            &pool,
            Row {
                group_id: Some("work"),
                ..Row::text("grouped", 100)
            },
        )
        .await;
        insert(
            &pool,
            Row {
                note: Some("keep"),
                ..Row::text("noted", 100)
            },
        )
        .await;
        insert(
            &pool,
            Row {
                note: Some(""),
                ..Row::text("empty-note", 100)
            },
        )
        .await;

        let mut conn = pool.acquire().await.unwrap();
        assert_eq!(count_protected(&mut conn).await.unwrap(), 4);
        let expired = delete_expired(&mut conn, &fallback(1)).await.unwrap();
        let over_count = delete_over_count(&mut conn, 1).await.unwrap();
        drop(conn);

        assert_eq!(expired.removed, 2);
        assert_eq!(over_count.removed, 0);
        assert_eq!(
            remaining(&pool).await,
            ["favorite", "grouped", "noted", "pinned"]
        );
    }

    #[tokio::test]
    async fn first_matching_rule_decides_and_fallback_covers_the_rest() {
        let pool = memory_pool().await;
        // 大图 3 小时前用过：命中「大于 1 MB 的图片 2 小时」过期。
        insert(&pool, Row::image("big-image", 2 * 1024 * 1024, 3)).await;
        // 小图同样 3 小时：跳过第一条，命中「图片 10 小时」保留。
        insert(&pool, Row::image("small-image", 1024, 3)).await;
        // 微信文本 100 小时：命中「不过期」规则保留，尽管兜底只留 24 小时。
        insert(
            &pool,
            Row {
                source_app_id: Some("wechat"),
                ..Row::text("wechat-text", 100)
            },
        )
        .await;
        // 其它文本 30 小时：没有规则命中，按兜底 24 小时过期。
        insert(&pool, Row::text("other-text", 30)).await;
        insert(
            &pool,
            Row {
                sub_kind: Some("url"),
                ..Row::text("fresh-url", 2)
            },
        )
        .await;

        let plan = AgePlan {
            rules: vec![
                RulePlan {
                    categories: vec![ContentCategory::Image],
                    min_size_bytes: Some(1024 * 1024),
                    cutoff: Some(hours_ago(2)),
                    ..RulePlan::default()
                },
                RulePlan {
                    categories: vec![ContentCategory::Image],
                    cutoff: Some(hours_ago(10)),
                    ..RulePlan::default()
                },
                RulePlan {
                    source_app_ids: vec!["wechat".to_owned()],
                    cutoff: None,
                    ..RulePlan::default()
                },
            ],
            fallback_cutoff: Some(hours_ago(24)),
        };

        let (per_rule, rest) = rule_match_stats(&mut pool.acquire().await.unwrap(), &plan)
            .await
            .unwrap();
        assert_eq!(
            per_rule,
            [
                RuleMatchStats {
                    matched: 1,
                    expired: 1
                },
                RuleMatchStats {
                    matched: 1,
                    expired: 0
                },
                RuleMatchStats {
                    matched: 1,
                    expired: 0
                },
            ]
        );
        assert_eq!(
            rest,
            RuleMatchStats {
                matched: 2,
                expired: 1
            }
        );

        let outcome = delete_expired(&mut pool.acquire().await.unwrap(), &plan)
            .await
            .unwrap();
        assert_eq!(outcome.removed, 2);
        assert_eq!(outcome.image_files, ["big-image.png"]);
        assert_eq!(
            remaining(&pool).await,
            ["fresh-url", "small-image", "wechat-text"]
        );
    }

    #[tokio::test]
    async fn rules_match_text_types_sensitive_and_unused_records() {
        let pool = memory_pool().await;
        insert(
            &pool,
            Row {
                is_sensitive: true,
                ..Row::text("secret", 2)
            },
        )
        .await;
        insert(
            &pool,
            Row {
                sub_kind: Some("color"),
                ..Row::text("color", 2)
            },
        )
        .await;
        // 采集后再次用过：最后使用时间晚于采集时间，不算「从未再用」。
        insert(
            &pool,
            Row {
                created_hours_ago: 5,
                ..Row::text("reused", 2)
            },
        )
        .await;
        insert(&pool, Row::text("once", 2)).await;

        let plan = AgePlan {
            rules: vec![
                RulePlan {
                    sensitive_only: true,
                    cutoff: Some(hours_ago(1)),
                    ..RulePlan::default()
                },
                RulePlan {
                    categories: vec![ContentCategory::Color, ContentCategory::Url],
                    cutoff: Some(hours_ago(1)),
                    ..RulePlan::default()
                },
                RulePlan {
                    unused_only: true,
                    cutoff: Some(hours_ago(1)),
                    ..RulePlan::default()
                },
            ],
            fallback_cutoff: None,
        };

        let outcome = delete_expired(&mut pool.acquire().await.unwrap(), &plan)
            .await
            .unwrap();

        assert_eq!(outcome.removed, 3);
        assert_eq!(remaining(&pool).await, ["reused"]);
    }

    #[tokio::test]
    async fn plan_without_any_cutoff_deletes_nothing() {
        let pool = memory_pool().await;
        insert(&pool, Row::text("a", 10_000)).await;

        let plan = AgePlan {
            rules: vec![RulePlan::default()],
            fallback_cutoff: None,
        };
        let outcome = delete_expired(&mut pool.acquire().await.unwrap(), &plan)
            .await
            .unwrap();

        assert_eq!(outcome.removed, 0);
        assert_eq!(remaining(&pool).await, ["a"]);
    }

    #[tokio::test]
    async fn count_limit_keeps_the_most_recently_used() {
        let pool = memory_pool().await;
        for (id, used) in [("a", 5), ("b", 4), ("c", 3)] {
            insert(&pool, Row::text(id, used)).await;
        }
        // 很早采集但刚用过，按最后使用时间排在最前。
        insert(
            &pool,
            Row {
                created_hours_ago: 500,
                ..Row::text("old-but-used", 0)
            },
        )
        .await;

        let outcome = delete_over_count(&mut pool.acquire().await.unwrap(), 2)
            .await
            .unwrap();

        assert_eq!(outcome.removed, 2);
        assert_eq!(remaining(&pool).await, ["c", "old-but-used"]);
        assert_eq!(
            delete_over_count(&mut pool.acquire().await.unwrap(), 0)
                .await
                .unwrap()
                .removed,
            0
        );
    }

    #[tokio::test]
    async fn storage_cleanup_frees_least_recently_used_first() {
        let pool = memory_pool().await;
        for (id, used) in [("t0", 4), ("t1", 3), ("t2", 2), ("t3", 1)] {
            insert(
                &pool,
                Row {
                    size: Some(100),
                    ..Row::text(id, used)
                },
            )
            .await;
        }
        insert(
            &pool,
            Row {
                size: Some(10_000),
                is_favorite: true,
                ..Row::text("fav", 50)
            },
        )
        .await;

        let outcome = delete_least_recent_until(&mut pool.acquire().await.unwrap(), 150, |_| 0)
            .await
            .unwrap();

        assert_eq!(outcome.removed, 2);
        assert_eq!(remaining(&pool).await, ["fav", "t2", "t3"]);
    }

    #[tokio::test]
    async fn storage_cleanup_sizes_images_by_stored_files() {
        let pool = memory_pool().await;
        insert(&pool, Row::image("img", 1, 2)).await;
        insert(
            &pool,
            Row {
                size: Some(1),
                ..Row::text("txt", 1)
            },
        )
        .await;

        let outcome =
            delete_least_recent_until(&mut pool.acquire().await.unwrap(), 4_000, |file_name| {
                assert_eq!(file_name, "img.png");
                5_000
            })
            .await
            .unwrap();

        assert_eq!(outcome.removed, 1);
        assert_eq!(outcome.image_files, ["img.png"]);
        assert_eq!(remaining(&pool).await, ["txt"]);
    }

    #[tokio::test]
    async fn storage_cleanup_keeps_history_when_target_is_unreachable() {
        let pool = memory_pool().await;
        for id in ["a", "b", "c"] {
            insert(
                &pool,
                Row {
                    size: Some(100),
                    ..Row::text(id, 1)
                },
            )
            .await;
        }

        let mut conn = pool.acquire().await.unwrap();
        let unreachable = delete_least_recent_until(&mut conn, 1_000, |_| 0)
            .await
            .unwrap();
        let nothing = delete_least_recent_until(&mut conn, 0, |_| 0)
            .await
            .unwrap();
        drop(conn);

        assert_eq!(unreachable.removed, 0);
        assert_eq!(nothing.removed, 0);
        assert_eq!(remaining(&pool).await.len(), 3);
    }

    #[tokio::test]
    async fn rolled_back_preview_leaves_history_untouched() {
        let pool = memory_pool().await;
        insert(&pool, Row::text("old", 100)).await;

        let mut tx = pool.begin().await.unwrap();
        let outcome = delete_expired(&mut tx, &fallback(1)).await.unwrap();
        tx.rollback().await.unwrap();

        assert_eq!(outcome.removed, 1);
        assert_eq!(remaining(&pool).await, ["old"]);
    }

    #[tokio::test]
    async fn compact_switches_to_incremental_auto_vacuum() {
        let dir = tempfile::tempdir().unwrap();
        let options = sqlx::sqlite::SqliteConnectOptions::new()
            .filename(dir.path().join("compact.db"))
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        for index in 0..50 {
            insert(
                &pool,
                Row {
                    id: &format!("big{index}"),
                    ..Row::text("", 1)
                },
            )
            .await;
        }
        sqlx::query("UPDATE clipboard_items SET content = hex(zeroblob(10000)) || id")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM clipboard_items")
            .execute(&pool)
            .await
            .unwrap();
        let free_before = crate::db::items::reusable_page_bytes(&pool).await.unwrap();

        compact(&pool).await.unwrap();

        let mode: i64 = sqlx::query_scalar("PRAGMA auto_vacuum")
            .fetch_one(&pool)
            .await
            .unwrap();
        let free_after = crate::db::items::reusable_page_bytes(&pool).await.unwrap();
        assert_eq!(mode, AUTO_VACUUM_INCREMENTAL);
        assert!(
            free_before > 0 && free_after == 0,
            "before={free_before} after={free_after}"
        );
    }
}
