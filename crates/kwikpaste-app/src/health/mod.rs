//! 7×24 稳健性（附录 C §8）：日志文件、panic hook、崩溃重启与重启上限。
//!
//! - [`install`] 是 `main` 的第一步：开日志（普通启动写 `<日志目录>/KwikPaste.log`，与 1.x 同一个
//!   文件；自测写 stderr），装 panic hook。
//! - panic hook 把线程名、位置、payload 和调用栈写进日志并 flush，把这次崩溃记进
//!   `<bootstrap>/state/last-crash.json`，然后交给原来的 hook：崩溃照常发生，不会被吞掉。
//! - 重启：10 分钟内第 1、2 次原样重启，第 3 次降级重启（`GPUI_DISABLE_DIRECT_COMPOSITION=1`、
//!   强制减少动画），再崩就不重启、在记录里标 `gaveUp`。子进程带 `--relaunched-after-crash <n>`
//!   和原来的参数，不弹面板。
//!   - 主线程 panic（窗口过程里的 panic 返回后进程即 abort）：hook 里直接拉起子进程；子进程
//!     等互斥量（≤5 s，父进程死时得到 `WAIT_ABANDONED`）接管单实例。
//!   - 其它线程 panic（如 `VSyncProvider`）：进程和消息循环还活着，请求经 channel 送给主线程做
//!     有序重启（`platform::watchdog`：释放单实例、拉起子进程、退出码 70）；10 s 内主线程没接手就
//!     由兜底线程拉起子进程并退出。
//! - 看门狗（`platform::watchdog`）在显示面板前和每 30 s 检查 vsync 线程（补丁 0001 的
//!   `vsync_thread_alive`），死了就走同一条有序重启。
//!
//! 自测进程默认只记录不重启，`--selftest-crash-restart` 才打开（`tools/platform-probes/crash-restart.ps1`）。

mod crash;
mod logger;
mod panic;

use std::path::PathBuf;
use std::sync::OnceLock;

pub use crash::{Restart, exit_code, request_restart, restart_requests, spawn_relaunch};

use crate::selftest;

/// 崩溃重启的子进程带的参数：`--relaunched-after-crash <n>`，`n` 是 10 分钟内的第几次。
pub const RELAUNCHED: &str = "--relaunched-after-crash";
/// 第几次重启起进入降级模式。
const DEGRADED_FROM: u32 = 3;

/// 本进程的启动状态，[`install`] 时确定。
struct State {
    /// 崩溃记录 `<bootstrap>/state/last-crash.json`；路径解析失败时为 `None`（只写日志）。
    crash_file: Option<PathBuf>,
    /// 这是第几次崩溃重启（普通启动为 0）。
    relaunch: u32,
    /// 崩溃时是否重启：普通启动是，自测只在 `--selftest-crash-restart` 时是。
    restart_enabled: bool,
}

static STATE: OnceLock<State> = OnceLock::new();

/// `main` 的第一步：开日志、装 panic hook。
pub fn install() {
    let identity = crate::identity::current();
    let paths = kwikpaste_core::CorePaths::for_native(identity.identifier, identity.env)
        .inspect_err(|err| eprintln!("app paths could not be resolved: {err:#}"))
        .ok();

    if selftest::active() {
        selftest::init_logging();
    } else if let Some(paths) = &paths {
        logger::init(paths.log_dir());
    }

    let relaunch = parse_relaunch(std::env::args());
    let restart_enabled = !selftest::active() || selftest::enabled(selftest::CRASH_RESTART);
    let _ = STATE.set(State {
        crash_file: paths.map(|paths| paths.bootstrap_dir().join("state").join(crash::CRASH_FILE)),
        relaunch,
        restart_enabled,
    });
    panic::install();

    if relaunch > 0 {
        log::warn!(
            "relaunched after crash #{relaunch} within {} minutes{}",
            crash::WINDOW.as_secs() / 60,
            if degraded() { " (degraded mode)" } else { "" }
        );
    }
}

/// 主实例确认之后调用：上次崩溃次数超限、没有再重启时，记一条日志并清掉标记。
pub fn after_claim() {
    if relaunch_count() == 0 {
        crash::acknowledge_give_up();
    }
}

/// 这是第几次崩溃重启（普通启动为 0）。
pub fn relaunch_count() -> u32 {
    STATE.get().map_or(0, |state| state.relaunch)
}

/// 降级模式：DirectComposition 关闭（子进程的环境变量），动画关闭。
pub fn degraded() -> bool {
    relaunch_count() >= DEGRADED_FROM
}

fn state() -> Option<&'static State> {
    STATE.get()
}

/// 从命令行取 `--relaunched-after-crash <n>`。
fn parse_relaunch(args: impl IntoIterator<Item = String>) -> u32 {
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == RELAUNCHED {
            return args.next().and_then(|n| n.parse().ok()).unwrap_or(1);
        }
    }

    0
}

/// 重启子进程的参数：原来的参数去掉旧的 `--relaunched-after-crash <n>`，再带上新的。
fn relaunch_args(args: impl IntoIterator<Item = String>, relaunch: u32) -> Vec<String> {
    let mut kept = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg == RELAUNCHED {
            args.next();
            continue;
        }
        kept.push(arg);
    }
    kept.push(RELAUNCHED.to_owned());
    kept.push(relaunch.to_string());

    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|arg| (*arg).to_owned()).collect()
    }

    #[test]
    fn relaunch_count_comes_from_the_flag() {
        assert_eq!(parse_relaunch(args(&["--auto-launch"])), 0);
        assert_eq!(parse_relaunch(args(&[RELAUNCHED, "2"])), 2);
        assert_eq!(parse_relaunch(args(&["--auto-launch", RELAUNCHED])), 1);
    }

    #[test]
    fn relaunch_keeps_the_arguments_and_replaces_the_count() {
        assert_eq!(
            relaunch_args(args(&["--auto-launch", RELAUNCHED, "1"]), 2),
            args(&["--auto-launch", RELAUNCHED, "2"])
        );
        assert_eq!(relaunch_args(args(&[]), 1), args(&[RELAUNCHED, "1"]));
    }
}
