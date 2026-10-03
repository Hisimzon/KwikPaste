//! 看门狗与有序重启的主线程一侧（附录 C §8 第 4、6 条，策略见 `crate::health`）。
//!
//! - 有序重启：其它线程 panic 或看门狗判定后，请求经 channel 送到这里：释放单实例、拉起带
//!   `--relaunched-after-crash <n>` 的子进程、退出（退出码 70）。
//! - 看门狗：显示面板前、以及每 30 s 检查 vsync 线程（补丁 0001 的 `vsync_thread_alive`）；
//!   它死了窗口就再也不会重绘，按崩溃处理。macOS 没有这个线程，恒为正常。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui::{App, AsyncApp};

use super::{instance, probe};
use crate::health::{self, Restart};

const CHECK_INTERVAL: Duration = Duration::from_secs(30);

/// 自测模拟 vsync 线程已死（`--selftest-vsync-dead`）。
static SIMULATED_DEAD: AtomicBool = AtomicBool::new(false);

/// 在 `Application::run` 回调里调用：开始处理有序重启请求并定时检查渲染线程。
pub fn serve(cx: &mut App) {
    probe::health(
        health::relaunch_count(),
        health::degraded(),
        std::env::var_os("GPUI_DISABLE_DIRECT_COMPOSITION").is_some(),
    );

    let requests = health::restart_requests();
    cx.spawn(async move |cx: &mut AsyncApp| {
        if let Ok(restart) = requests.recv().await {
            cx.update(|cx| restart_now(cx, &restart));
        }
    })
    .detach();

    cx.spawn(async move |cx: &mut AsyncApp| {
        loop {
            cx.background_executor().timer(CHECK_INTERVAL).await;
            if !render_thread_ok("periodic check") {
                break;
            }
        }
    })
    .detach();
}

/// 渲染线程是否正常；不正常时要求有序重启并返回 `false`（调用方不要再显示面板）。
pub fn render_thread_ok(source: &str) -> bool {
    if render_thread_alive() {
        return true;
    }
    health::request_restart(
        "watchdog",
        format!("{source}: the vsync thread is not running"),
    );

    false
}

#[cfg(target_os = "windows")]
fn render_thread_alive() -> bool {
    gpui_windows::vsync_thread_alive() && !SIMULATED_DEAD.load(Ordering::SeqCst)
}

#[cfg(target_os = "macos")]
fn render_thread_alive() -> bool {
    !SIMULATED_DEAD.load(Ordering::SeqCst)
}

/// 自测：让看门狗认为渲染线程已死。
pub fn simulate_dead_render_thread() {
    log::warn!("selftest: the watchdog now treats the vsync thread as dead");
    SIMULATED_DEAD.store(true, Ordering::SeqCst);
}

fn restart_now(cx: &mut App, restart: &Restart) {
    log::error!(
        "orderly restart #{}{}: {}",
        restart.relaunch,
        if restart.degraded { " (degraded)" } else { "" },
        restart.reason
    );
    probe::quitting();
    // 先放掉单实例，子进程启动时直接成为主实例。
    instance::release(cx);
    if let Err(err) = health::spawn_relaunch(restart) {
        log::error!("the restarted instance could not be started: {err}");
    }
    cx.quit();
}
