//! 自动检查始终开启：启动 8 秒后首次检查，之后每小时醒一次，距上次成功检查满 6 小时（按墙钟）才再查；
//! 失败的检查不算，下一小时重试。墙钟比较让系统睡眠的时间也计入间隔（单调时钟在 macOS 睡眠时不走）。

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};

pub(crate) const INITIAL_DELAY: Duration = Duration::from_secs(8);
pub(crate) const TICK: Duration = Duration::from_secs(60 * 60);
const CHECK_INTERVAL: TimeDelta = TimeDelta::hours(6);

/// 本次运行还没成功检查过，或距上次成功检查已满间隔（时钟倒退也算到期）。
pub(crate) fn check_due(last_success: Option<DateTime<Utc>>, now: DateTime<Utc>) -> bool {
    last_success.is_none_or(|at| {
        let elapsed = now.signed_duration_since(at);
        elapsed >= CHECK_INTERVAL || elapsed < TimeDelta::zero()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_are_due_six_wall_clock_hours_after_the_last_success() {
        let at = DateTime::parse_from_rfc3339("2026-10-14T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert!(check_due(None, at));
        assert!(!check_due(Some(at), at + TimeDelta::hours(5)));
        assert!(check_due(Some(at), at + TimeDelta::hours(6)));
        assert!(check_due(Some(at), at - TimeDelta::hours(1)));
    }
}
