//! 自动检查始终开启：启动 8 秒后首次检查，之后每 6 小时检查一次，失败也等待下一轮。

use std::time::Duration;

pub(crate) const INITIAL_DELAY: Duration = Duration::from_secs(8);
pub(crate) const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checks_start_after_eight_seconds_and_repeat_every_six_hours() {
        assert_eq!(INITIAL_DELAY, Duration::from_secs(8));
        assert_eq!(CHECK_INTERVAL, Duration::from_secs(21_600));
    }
}
