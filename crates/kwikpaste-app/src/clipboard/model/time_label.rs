//! 卡片右上角的时间标签。
//!
//! 1.x 由命令层按查询时刻格式化成 `displayCreatedAt`，面板开着跨过午夜也不更新（frontend-main §9-4）。
//! 原生版改由 UI 按 `createdAt` 和当前本地日期格式化，面板每次显示时重算（附录 D §8 第 4 条），
//! 格式不变：今天 `HH:mm`，今年 `MM-DD HH:mm`，更早 `YYYY-MM-DD HH:mm`。

use std::fmt::Display;

use chrono::{DateTime, Datelike, TimeZone, Utc};

/// 按 `now` 所在时区格式化 `created_at`。
pub fn time_label<Tz>(created_at: DateTime<Utc>, now: &DateTime<Tz>) -> String
where
    Tz: TimeZone,
    Tz::Offset: Display,
{
    let local = created_at.with_timezone(&now.timezone());

    if local.date_naive() == now.date_naive() {
        local.format("%H:%M").to_string()
    } else if local.year() == now.year() {
        local.format("%m-%d %H:%M").to_string()
    } else {
        local.format("%Y-%m-%d %H:%M").to_string()
    }
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
}
