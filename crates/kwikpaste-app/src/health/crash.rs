//! 崩溃记录（`last-crash.json`）、重启策略和拉起子进程。

use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::time::Duration;

use async_channel::{Receiver, Sender};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::logger;

pub const CRASH_FILE: &str = "last-crash.json";
/// 统计重启次数的时间窗。
pub const WINDOW: Duration = Duration::from_secs(10 * 60);
/// 时间窗内最多重启几次（最后一次是降级重启）。
const MAX_RESTARTS: u32 = 3;
/// 记录里最多保留几条。
const KEPT: usize = 10;
/// 其它线程 panic 后，主线程这么久还没完成有序重启，就由兜底线程拉起子进程并退出。
const FALLBACK_AFTER: Duration = Duration::from_secs(10);
/// 有序重启的退出码。
const RESTART_EXIT_CODE: i32 = 70;
/// GPUI 读的环境变量：降级模式关掉 DirectComposition。
const DISABLE_DIRECT_COMPOSITION: &str = "GPUI_DISABLE_DIRECT_COMPOSITION";

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrashLog {
    #[serde(default)]
    crashes: Vec<CrashRecord>,
    /// 时间窗内崩溃次数超限、没有再重启。下次手动启动时读到并清掉。
    #[serde(default)]
    gave_up: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CrashRecord {
    at: DateTime<Utc>,
    pid: u32,
    version: String,
    thread: String,
    location: String,
    message: String,
    /// `restart`、`restart-degraded`、`give-up`，或自测不重启时的 `none`。
    action: String,
}

/// 一次崩溃（panic 或看门狗判定）的描述。
pub struct CrashInfo {
    pub thread: String,
    pub location: String,
    pub message: String,
}

/// 一次重启：子进程带 `--relaunched-after-crash <relaunch>`，`degraded` 时进降级模式。
#[derive(Debug, Clone)]
pub struct Restart {
    pub relaunch: u32,
    pub degraded: bool,
    pub reason: String,
}

enum Action {
    None,
    Restart(Restart),
    GiveUp,
}

static REQUESTS: LazyLock<(Sender<Restart>, Receiver<Restart>)> =
    LazyLock::new(async_channel::unbounded);
/// 只处理第一次崩溃：之后的 panic 只记日志。
static HANDLING: AtomicBool = AtomicBool::new(false);
static SPAWNED: AtomicBool = AtomicBool::new(false);
static EXIT_CODE: AtomicI32 = AtomicI32::new(0);

/// 主线程上要处理的有序重启请求。
pub fn restart_requests() -> Receiver<Restart> {
    REQUESTS.1.clone()
}

/// 有序重启拉起子进程之后要求的退出码（没有重启时为 0）。
pub fn exit_code() -> i32 {
    EXIT_CODE.load(Ordering::SeqCst)
}

/// panic hook 调用。主线程上的 panic 之后进程就会退出，这里直接拉起子进程；其它线程的交给主线程
/// 有序重启，并开兜底线程。
pub(super) fn on_panic(info: CrashInfo, on_main_thread: bool) {
    let Some(restart) = begin(info) else {
        return;
    };
    if on_main_thread {
        if let Err(err) = spawn_relaunch(&restart) {
            logger::write_crash(&format!("the restart could not be started: {err}"));
        }
        return;
    }
    let _ = REQUESTS.0.try_send(restart.clone());
    start_fallback(restart);
}

/// 不是 panic、但应用已经不能正常工作（看门狗发现 vsync 线程死了）时要求有序重启。
pub fn request_restart(source: &str, reason: String) {
    log::error!("{source}: {reason}; restarting");
    let info = CrashInfo {
        thread: source.to_owned(),
        location: String::new(),
        message: reason,
    };
    let Some(restart) = begin(info) else {
        return;
    };
    let _ = REQUESTS.0.try_send(restart.clone());
    start_fallback(restart);
}

/// 记一次崩溃并按时间窗内的次数决定怎么办；需要重启时返回重启参数。
fn begin(info: CrashInfo) -> Option<Restart> {
    if HANDLING.swap(true, Ordering::SeqCst) {
        logger::write_crash("another crash is already being handled; not restarting again");
        return None;
    }
    let state = super::state();
    let file = state.and_then(|state| state.crash_file.as_deref());
    let mut log = file.map(load).unwrap_or_default();
    let now = Utc::now();
    let recent = match file {
        Some(_) => recent_count(&log, now) + 1,
        None => super::relaunch_count() + 1,
    };
    let action = if !state.is_some_and(|state| state.restart_enabled) {
        Action::None
    } else if recent <= MAX_RESTARTS {
        Action::Restart(Restart {
            relaunch: recent,
            degraded: recent >= super::DEGRADED_FROM,
            reason: format!("thread '{}': {}", info.thread, info.message),
        })
    } else {
        Action::GiveUp
    };
    let action_name = match &action {
        Action::None => "none",
        Action::Restart(restart) if restart.degraded => "restart-degraded",
        Action::Restart(_) => "restart",
        Action::GiveUp => "give-up",
    };

    log.crashes.push(CrashRecord {
        at: now,
        pid: std::process::id(),
        version: env!("CARGO_PKG_VERSION").to_owned(),
        thread: info.thread,
        location: info.location,
        message: info.message,
        action: action_name.to_owned(),
    });
    let excess = log.crashes.len().saturating_sub(KEPT);
    log.crashes.drain(..excess);
    log.gave_up = matches!(action, Action::GiveUp);
    if let Some(file) = file {
        save(file, &log);
    }
    logger::write_crash(&format!(
        "crash #{recent} within {} minutes: {action_name}",
        WINDOW.as_secs() / 60
    ));

    match action {
        Action::Restart(restart) => Some(restart),
        Action::None | Action::GiveUp => None,
    }
}

fn recent_count(log: &CrashLog, now: DateTime<Utc>) -> u32 {
    let window = chrono::Duration::from_std(WINDOW).unwrap_or(chrono::Duration::MAX);
    let recent = log
        .crashes
        .iter()
        .filter(|crash| now.signed_duration_since(crash.at) < window)
        .count();

    u32::try_from(recent).unwrap_or(u32::MAX)
}

fn load(path: &Path) -> CrashLog {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save(path: &Path, log: &CrashLog) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_vec_pretty(log) {
        Ok(bytes) => {
            if let Err(err) = std::fs::write(path, bytes) {
                logger::write_crash(&format!("{} could not be written: {err}", path.display()));
            }
        }
        Err(err) => logger::write_crash(&format!("crash record could not be encoded: {err}")),
    }
}

/// 拉起重启后的子进程（只拉一次）：同一个可执行文件、原来的参数加 `--relaunched-after-crash <n>`，
/// 降级时带 `GPUI_DISABLE_DIRECT_COMPOSITION=1`。成功后退出码记为 70。
pub fn spawn_relaunch(restart: &Restart) -> std::io::Result<u32> {
    if SPAWNED.swap(true, Ordering::SeqCst) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "the restart was already started",
        ));
    }
    let spawned = std::env::current_exe().and_then(|exe| {
        let mut command = Command::new(exe);
        command.args(super::relaunch_args(
            std::env::args().skip(1),
            restart.relaunch,
        ));
        if restart.degraded {
            command.env(DISABLE_DIRECT_COMPOSITION, "1");
        } else if super::degraded() {
            command.env_remove(DISABLE_DIRECT_COMPOSITION);
        }
        command.spawn()
    });

    match spawned {
        Ok(child) => {
            EXIT_CODE.store(RESTART_EXIT_CODE, Ordering::SeqCst);
            logger::write_crash(&format!(
                "restarted as pid {} (relaunch #{}{})",
                child.id(),
                restart.relaunch,
                if restart.degraded { ", degraded" } else { "" }
            ));
            Ok(child.id())
        }
        Err(err) => {
            SPAWNED.store(false, Ordering::SeqCst);
            Err(err)
        }
    }
}

/// 主线程迟迟不接手时的兜底：拉起子进程（还没拉起的话）并直接退出。
fn start_fallback(restart: Restart) {
    let spawned = std::thread::Builder::new()
        .name("crash-restart".to_owned())
        .spawn(move || {
            std::thread::sleep(FALLBACK_AFTER);
            logger::write_crash(
                "the orderly restart did not finish in time; restarting from the fallback thread",
            );
            if !SPAWNED.load(Ordering::SeqCst)
                && let Err(err) = spawn_relaunch(&restart)
            {
                logger::write_crash(&format!("the restart could not be started: {err}"));
            }
            std::process::exit(RESTART_EXIT_CODE);
        });
    if let Err(err) = spawned {
        logger::write_crash(&format!("the fallback restart thread did not start: {err}"));
    }
}

/// 普通启动时：上一轮崩溃次数超限、没再重启的，记一条日志并清掉标记。
pub(super) fn acknowledge_give_up() {
    let Some(file) = super::state().and_then(|state| state.crash_file.as_deref()) else {
        return;
    };
    let mut log = load(file);
    if !log.gave_up {
        return;
    }
    if let Some(last) = log.crashes.last() {
        log::warn!(
            "the previous run crashed more than {MAX_RESTARTS} times within {} minutes and was not restarted; last crash at {} on thread '{}' at {}: {}",
            WINDOW.as_secs() / 60,
            last.at,
            last.thread,
            last.location,
            last.message
        );
    }
    log.gave_up = false;
    save(file, &log);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(minutes_ago: i64, now: DateTime<Utc>) -> CrashRecord {
        CrashRecord {
            at: now - chrono::Duration::minutes(minutes_ago),
            pid: 1,
            version: String::new(),
            thread: "main".to_owned(),
            location: String::new(),
            message: String::new(),
            action: "restart".to_owned(),
        }
    }

    #[test]
    fn only_crashes_inside_the_window_count() {
        let now = Utc::now();
        let log = CrashLog {
            crashes: vec![record(30, now), record(9, now), record(1, now)],
            gave_up: false,
        };

        assert_eq!(recent_count(&log, now), 2);
    }

    #[test]
    fn crash_log_round_trips_and_tolerates_missing_fields() {
        let now = Utc::now();
        let log = CrashLog {
            crashes: vec![record(1, now)],
            gave_up: true,
        };
        let text = serde_json::to_string(&log).expect("encode");
        assert!(text.contains("\"gaveUp\":true"));
        let back: CrashLog = serde_json::from_str(&text).expect("decode");
        assert_eq!(back.crashes.len(), 1);

        let empty: CrashLog = serde_json::from_str("{}").expect("decode");
        assert!(empty.crashes.is_empty() && !empty.gave_up);
    }
}
