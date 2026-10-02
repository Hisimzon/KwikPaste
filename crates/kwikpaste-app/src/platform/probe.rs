//! 平台自测的探针日志。
//!
//! 同时有 `--selftest-platform` 和 `KWIKPASTE_SELFTEST=1` 时，把面板每次就绪、显示、隐藏的刻度、
//! 矩形和子类计数按 JSON 行追加到 `KWIKPASTE_PROBE_LOG` 指向的文件，由 `tools/platform-probes`
//! 的脚本判定。刻度是 [`kwikpaste_os::clock`]，Windows 上与脚本的 `Stopwatch.GetTimestamp()` 同源。

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::sync::{Mutex, OnceLock};

use kwikpaste_os::clock;

use super::native::NativePanel;
use super::panel::Trigger;
use crate::selftest;

const LOG_ENV: &str = "KWIKPASTE_PROBE_LOG";

static LOG: OnceLock<Mutex<File>> = OnceLock::new();

/// 自测开关打开且给了日志路径时打开探针日志。
pub fn init() {
    if !selftest::enabled(selftest::PLATFORM) {
        return;
    }
    let Some(path) = std::env::var_os(LOG_ENV) else {
        log::warn!(
            "{} is set but {LOG_ENV} is not; no probe log",
            selftest::PLATFORM
        );
        return;
    };

    match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(file) => {
            let _ = LOG.set(Mutex::new(file));
        }
        Err(err) => log::error!("probe log {} could not be opened: {err}", path.display()),
    }
}

pub fn enabled() -> bool {
    LOG.get().is_some()
}

fn write(event: &str, fields: &str) {
    let Some(log) = LOG.get() else {
        return;
    };
    let line = format!(
        r#"{{"event":"{event}","pid":{},"ticks":{},"ticks_per_second":{}{fields}}}"#,
        std::process::id(),
        clock::now_ticks(),
        clock::ticks_per_second(),
    );
    if let Ok(mut file) = log.lock() {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

/// 面板装好原生补丁、开始接收命令。
pub fn ready(native: &NativePanel) {
    if enabled() {
        write("ready", &native.probe_fields(None));
    }
}

/// 面板出了首帧。`ticks` 依次是开始显示、显示调用返回、首帧渲染、判定的首帧时刻。
pub fn shown(
    trigger: Trigger,
    ticks: [i64; 4],
    latency_ms: f64,
    frame_inside_show: bool,
    native_fields: &str,
) {
    let [show_started, show_returned, rendered, frame] = ticks;
    write(
        "shown",
        &format!(
            r#","source":"{}","trigger_ticks":{},"show_started":{show_started},"show_returned":{show_returned},"rendered":{rendered},"frame":{frame},"latency_ms":{latency_ms:.3},"frame_inside_show":{frame_inside_show}{native_fields}"#,
            trigger.source.name(),
            trigger.ticks,
        ),
    );
}

/// 面板已隐藏。
pub fn hidden(trigger: Trigger, native_fields: &str) {
    write(
        "hidden",
        &format!(
            r#","source":"{}","trigger_ticks":{}{native_fields}"#,
            trigger.source.name(),
            trigger.ticks,
        ),
    );
}

/// 进程即将退出。
pub fn quitting() {
    write("quit", "");
}
