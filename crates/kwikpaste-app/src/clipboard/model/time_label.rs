//! 卡片右上角的时间标签。
//!
//! 1.x 由命令层按查询时刻格式化成 `displayCreatedAt`，面板开着跨过午夜也不更新（frontend-main §9-4）。
//! 原生版改由 UI 按 `createdAt` 和当前本地日期格式化，面板每次显示时重算（附录 D §8 第 4 条），
//! 格式不变：今天 `HH:mm`，今年 `MM-DD HH:mm`，更早 `YYYY-MM-DD HH:mm`。

use std::fmt::Display;

use chrono::{DateTime, TimeZone, Utc};

/// 按 `now` 所在时区格式化 `created_at`。
pub fn time_label<Tz>(created_at: DateTime<Utc>, now: &DateTime<Tz>) -> String
where
    Tz: TimeZone,
    Tz::Offset: Display,
{
    kwikpaste_core::presenter::display_created_at(&created_at, now)
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;

    use super::*;

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    #[test]
    fn three_formats_by_local_date() {
        let shanghai = FixedOffset::east_opt(8 * 3600).expect("valid offset");
        let now = utc("2026-10-02T04:00:00Z").with_timezone(&shanghai);

        assert_eq!(time_label(utc("2026-10-02T01:05:00Z"), &now), "09:05");
        assert_eq!(time_label(utc("2026-09-30T23:59:00Z"), &now), "10-01 07:59");
        assert_eq!(
            time_label(utc("2025-12-31T15:30:00Z"), &now),
            "2025-12-31 23:30"
        );
    }

    #[test]
    fn midnight_is_judged_in_local_time() {
        let shanghai = FixedOffset::east_opt(8 * 3600).expect("valid offset");
        // 本地 10-02 00:10，记录是本地 10-01 23:50：已经是“昨天”。
        let now = utc("2026-10-01T16:10:00Z").with_timezone(&shanghai);

        assert_eq!(time_label(utc("2026-10-01T15:50:00Z"), &now), "10-01 23:50");
        assert_eq!(time_label(utc("2026-10-01T16:05:00Z"), &now), "00:05");
    }
    #[test]
    fn extreme_timestamps_survive_both_offset_directions() {
        for seconds in [-8 * 3600, 8 * 3600] {
            let offset = FixedOffset::east_opt(seconds).expect("valid offset");
            let now = utc("2026-10-02T04:00:00Z").with_timezone(&offset);
            for created_at in [DateTime::<Utc>::MIN_UTC, DateTime::<Utc>::MAX_UTC] {
                assert!(!time_label(created_at, &now).is_empty());
            }
        }
    }
}
