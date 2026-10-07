//! 自动检查的时间安排（与 1.x 相同）：启动 8 秒后开始，按设置的频率检查；关掉自动检查时每小时重读一次设置；
//! 每次检查之后（不论成败）至少隔一小时再看，失败也就是每小时重试一次。

use std::time::Duration;

use chrono::{DateTime, Utc};
use kwikpaste_core::settings::{Update as UpdateSettings, UpdateFrequency};

pub(crate) const INITIAL_DELAY: Duration = Duration::from_secs(8);
pub(crate) const SETTINGS_REFRESH: Duration = Duration::from_secs(60 * 60);
pub(crate) const RETRY_AFTER_CHECK: Duration = Duration::from_secs(60 * 60);

/// 距离下一次自动检查还要等多久；为 0 表示现在就该检查。
pub(crate) fn next_auto_check_delay(settings: &UpdateSettings, now: DateTime<Utc>) -> Duration {
    if !settings.auto_check {
        return SETTINGS_REFRESH;
    }

    let Some(last_checked_at) = settings.last_checked_at.as_deref() else {
        return Duration::ZERO;
    };
    let Ok(last_checked_at) = DateTime::parse_from_rfc3339(last_checked_at) else {
        return Duration::ZERO;
    };

    let elapsed = now
        .signed_duration_since(last_checked_at.with_timezone(&Utc))
        .num_seconds();
    let remaining = frequency_seconds(settings.frequency).saturating_sub(elapsed);
    if remaining <= 0 {
        return Duration::ZERO;
    }

    Duration::from_secs(remaining as u64).min(SETTINGS_REFRESH)
}

fn frequency_seconds(frequency: UpdateFrequency) -> i64 {
    match frequency {
        UpdateFrequency::Daily => 24 * 60 * 60,
        UpdateFrequency::Weekly => 7 * 24 * 60 * 60,
        UpdateFrequency::Monthly => 30 * 24 * 60 * 60,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update_settings(
        auto_check: bool,
        frequency: UpdateFrequency,
        last_checked_at: Option<String>,
    ) -> UpdateSettings {
        UpdateSettings {
            auto_check,
            frequency,
            last_checked_at,
            skipped_version: None,
        }
    }

    fn fixed_now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-30T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    #[test]
    fn auto_check_delay_uses_settings_refresh_when_disabled() {
        let settings = update_settings(false, UpdateFrequency::Daily, None);

        assert_eq!(
            next_auto_check_delay(&settings, fixed_now()),
            SETTINGS_REFRESH
        );
    }

    #[test]
    fn auto_check_delay_is_due_without_previous_check() {
        let settings = update_settings(true, UpdateFrequency::Daily, None);

        assert_eq!(
            next_auto_check_delay(&settings, fixed_now()),
            Duration::ZERO
        );
    }

    #[test]
    fn auto_check_delay_uses_configured_frequency() {
        let now = fixed_now();
        let last_checked_at = (now - chrono::Duration::hours(24)).to_rfc3339();
        let daily = update_settings(true, UpdateFrequency::Daily, Some(last_checked_at.clone()));
        let weekly = update_settings(true, UpdateFrequency::Weekly, Some(last_checked_at));

        assert_eq!(next_auto_check_delay(&daily, now), Duration::ZERO);
        assert_eq!(next_auto_check_delay(&weekly, now), SETTINGS_REFRESH);
    }

    #[test]
    fn auto_check_delay_sleeps_until_due_when_within_refresh_window() {
        let now = fixed_now();
        let last_checked_at = (now - chrono::Duration::minutes(23 * 60 + 30)).to_rfc3339();
        let settings = update_settings(true, UpdateFrequency::Daily, Some(last_checked_at));

        assert_eq!(
            next_auto_check_delay(&settings, now),
            Duration::from_secs(30 * 60)
        );
    }

    #[test]
    fn unreadable_timestamps_mean_check_now() {
        let settings =
            update_settings(true, UpdateFrequency::Monthly, Some("yesterday".to_owned()));

        assert_eq!(
            next_auto_check_delay(&settings, fixed_now()),
            Duration::ZERO
        );
    }
}
