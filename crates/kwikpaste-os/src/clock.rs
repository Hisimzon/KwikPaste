//! 单调时钟刻度，用来量“触发到首帧”这类跨线程、跨进程的延迟。
//!
//! Windows 取 `QueryPerformanceCounter`：与 .NET `Stopwatch.GetTimestamp()` 同源，测量脚本
//! 记下的注入时刻能直接和应用里的刻度相减。

/// 当前刻度。
#[cfg(target_os = "windows")]
pub fn now_ticks() -> i64 {
    let mut ticks = 0;
    // 文档保证 XP 以后不会失败。
    let _ = unsafe { windows::Win32::System::Performance::QueryPerformanceCounter(&mut ticks) };
    ticks
}

/// 每秒的刻度数。
#[cfg(target_os = "windows")]
pub fn ticks_per_second() -> i64 {
    let mut frequency = 0;
    let _ =
        unsafe { windows::Win32::System::Performance::QueryPerformanceFrequency(&mut frequency) };
    frequency.max(1)
}

/// 当前刻度：进程内第一次调用起的纳秒数。
#[cfg(not(target_os = "windows"))]
pub fn now_ticks() -> i64 {
    use std::sync::OnceLock;
    use std::time::Instant;

    static EPOCH: OnceLock<Instant> = OnceLock::new();
    let nanos = EPOCH.get_or_init(Instant::now).elapsed().as_nanos();
    i64::try_from(nanos).unwrap_or(i64::MAX)
}

/// 每秒的刻度数。
#[cfg(not(target_os = "windows"))]
pub fn ticks_per_second() -> i64 {
    1_000_000_000
}

/// 两个刻度之差换算成毫秒。
pub fn ticks_to_ms(delta: i64) -> f64 {
    delta as f64 * 1000.0 / ticks_per_second() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ticks_are_monotonic_and_convert_to_milliseconds() {
        let start = now_ticks();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let elapsed = ticks_to_ms(now_ticks() - start);

        assert!(elapsed >= 15.0, "elapsed {elapsed} ms");
        assert!(elapsed < 2000.0, "elapsed {elapsed} ms");
    }
}
