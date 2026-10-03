use std::{
    sync::LazyLock,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use gpui_util::ResultExt;
use windows::Win32::{
    Foundation::HWND,
    Graphics::Dwm::{DWM_TIMING_INFO, DwmFlush, DwmGetCompositionTimingInfo},
    System::Performance::QueryPerformanceFrequency,
};

static QPC_TICKS_PER_SECOND: LazyLock<u64> = LazyLock::new(|| {
    let mut frequency = 0;
    // On systems that run Windows XP or later, the function will always succeed and
    // will thus never return zero.
    unsafe { QueryPerformanceFrequency(&mut frequency).unwrap() };
    frequency as u64
});

const VSYNC_INTERVAL_THRESHOLD: Duration = Duration::from_millis(1);
const DEFAULT_VSYNC_INTERVAL: Duration = Duration::from_micros(16_666); // ~60Hz

// [kwikpaste patch 0001] Park the vsync thread while no GPUI window is visible.
//
// WM_SHOWWINDOW(TRUE) wakes the thread before the window is actually visible, so right after a wake
// the thread must not park for the full timeout again (frames for async updates would then wait up to
// a second for input). For `WAKE_GRACE` after a wake it only naps one ~60 Hz interval per park.
const WAKE_GRACE: Duration = Duration::from_millis(500);

#[derive(Default)]
struct WakeState {
    woken: bool,
    grace_until: Option<Instant>,
}

static VSYNC_WAKE: LazyLock<(std::sync::Mutex<WakeState>, std::sync::Condvar)> =
    LazyLock::new(|| (std::sync::Mutex::new(WakeState::default()), std::sync::Condvar::new()));

/// Wake the vsync thread, e.g. because a window became visible.
pub(crate) fn wake_vsync_thread() {
    let (state, condvar) = &*VSYNC_WAKE;
    let mut state = state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    state.woken = true;
    state.grace_until = Some(Instant::now() + WAKE_GRACE);
    condvar.notify_all();
}

/// Block until [`wake_vsync_thread`] is called or `timeout` elapses; shortly after a wake, only
/// for one ~60 Hz interval.
pub(crate) fn park_vsync_thread(timeout: Duration) {
    let (state, condvar) = &*VSYNC_WAKE;
    let guard = state.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let timeout = match guard.grace_until {
        Some(until) if Instant::now() < until => DEFAULT_VSYNC_INTERVAL,
        _ => timeout,
    };
    let (mut guard, _) = condvar
        .wait_timeout_while(guard, timeout, |state| !state.woken)
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.woken = false;
}

// [kwikpaste patch 0001] Health signal: true while the vsync thread runs its loop. Its guard drops
// when the thread exits or unwinds (e.g. the "Device lost" panic), so a dead thread reads false.
static VSYNC_THREAD_ALIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the vsync thread is running. `false` before the platform starts it and after it exited
/// or panicked; without it no window ever redraws.
pub fn vsync_thread_alive() -> bool {
    VSYNC_THREAD_ALIVE.load(std::sync::atomic::Ordering::Acquire)
}

/// Marks the vsync thread alive until dropped.
pub(crate) struct VSyncThreadAlive;

impl VSyncThreadAlive {
    pub(crate) fn mark() -> Self {
        VSYNC_THREAD_ALIVE.store(true, std::sync::atomic::Ordering::Release);
        Self
    }
}

impl Drop for VSyncThreadAlive {
    fn drop(&mut self) {
        VSYNC_THREAD_ALIVE.store(false, std::sync::atomic::Ordering::Release);
    }
}

pub(crate) struct VSyncProvider {
    interval: Duration,
    f: Box<dyn Fn() -> bool>,
}

impl VSyncProvider {
    pub(crate) fn new() -> Self {
        let interval = get_dwm_interval()
            .context("Failed to get DWM interval")
            .log_err()
            .unwrap_or(DEFAULT_VSYNC_INTERVAL);
        let f = Box::new(|| unsafe { DwmFlush().is_ok() });
        Self { interval, f }
    }

    pub(crate) fn wait_for_vsync(&self) {
        let vsync_start = Instant::now();
        let wait_succeeded = (self.f)();
        let elapsed = vsync_start.elapsed();
        // DwmFlush and DCompositionWaitForCompositorClock returns very early
        // instead of waiting until vblank when the monitor goes to sleep or is
        // unplugged (nothing to present due to desktop occlusion). We use 1ms as
        // a threshold for the duration of the wait functions and fallback to
        // Sleep() if it returns before that. This could happen during normal
        // operation for the first call after the vsync thread becomes non-idle,
        // but it shouldn't happen often.
        if !wait_succeeded || elapsed < VSYNC_INTERVAL_THRESHOLD {
            log::trace!("VSyncProvider::wait_for_vsync() took less time than expected");
            std::thread::sleep(self.interval);
        }
    }
}

fn get_dwm_interval() -> Result<Duration> {
    let mut timing_info = DWM_TIMING_INFO {
        cbSize: std::mem::size_of::<DWM_TIMING_INFO>() as u32,
        ..Default::default()
    };
    unsafe { DwmGetCompositionTimingInfo(HWND::default(), &mut timing_info) }?;
    let interval = retrieve_duration(timing_info.qpcRefreshPeriod, *QPC_TICKS_PER_SECOND);
    // Check for interval values that are impossibly low. A 29 microsecond
    // interval was seen (from a qpcRefreshPeriod of 60).
    if interval < VSYNC_INTERVAL_THRESHOLD {
        Ok(retrieve_duration(
            timing_info.rateRefresh.uiDenominator as u64,
            timing_info.rateRefresh.uiNumerator as u64,
        ))
    } else {
        Ok(interval)
    }
}

#[inline]
fn retrieve_duration(counts: u64, ticks_per_second: u64) -> Duration {
    let ticks_per_microsecond = ticks_per_second / 1_000_000;
    Duration::from_micros(counts / ticks_per_microsecond)
}
