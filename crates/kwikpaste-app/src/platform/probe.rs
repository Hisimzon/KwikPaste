//! 平台自测的探针日志。
//!
//! 同时有 `--selftest-platform` 和 `KWIKPASTE_SELFTEST=1` 时，把面板就绪、显示、隐藏、进出编辑态、
//! 钩子按键、输入框内容和输入法状态按 JSON 行追加到 `KWIKPASTE_PROBE_LOG` 指向的文件，由
//! `tools/platform-probes` 的脚本判定。刻度是 [`kwikpaste_os::clock`]，Windows 上与脚本的
//! `Stopwatch.GetTimestamp()` 同源。

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};

use kwikpaste_os::clock;

use super::editing::{EditReport, EditTrigger};
use super::native::NativePanel;
use super::panel::Trigger;
use super::system::SystemSignals;
use crate::selftest;

const LOG_ENV: &str = "KWIKPASTE_PROBE_LOG";

static LOG: OnceLock<Mutex<File>> = OnceLock::new();
/// 面板窗口句柄（Windows），读写输入法状态用。
static PANEL: AtomicIsize = AtomicIsize::new(0);

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

fn json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned())
}

/// 面板装好原生补丁、开始接收命令。
pub fn ready(native: &NativePanel) {
    PANEL.store(native.raw_handle(), Ordering::SeqCst);
    if enabled() {
        let hook_keys = super::hook_keystrokes().len();
        write(
            "ready",
            &format!(
                r#","hook_keystrokes":{hook_keys}{}"#,
                native.probe_fields(None)
            ),
        );
    }
}

/// 系统设置信号（启动和变化时）。
pub fn signals(signals: &SystemSignals) {
    write(
        "signals",
        &format!(
            r#","text_scale":{},"high_contrast":{},"reduce_motion":{}"#,
            signals.text_scale, signals.high_contrast, signals.reduce_motion
        ),
    );
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

/// 一次进入编辑态的结果。
pub fn editing(
    trigger: EditTrigger,
    report: EditReport,
    error: Option<String>,
    native_fields: &str,
) {
    write(
        "editing",
        &format!(
            r#","trigger":"{}","entered":{},"error":{},"previous_foreground":{},"marked_alt_swallowed":{},"elapsed_ms":{:.3}{native_fields}"#,
            trigger.name(),
            error.is_none(),
            error.as_deref().map_or("null".to_owned(), json_string),
            report.previous_foreground,
            report.marked_alt_swallowed,
            report.elapsed_ms,
        ),
    );
}

/// 已退出编辑态。
pub fn editing_ended(native_fields: &str) {
    write("editing_ended", native_fields);
}

/// 一个钩子按键派发完毕。
#[cfg(target_os = "windows")]
pub fn hook_key(event: &kwikpaste_os::win::keyboard::HookEvent, handled: bool) {
    use kwikpaste_os::win::keyboard::HookEvent;

    if !enabled() {
        return;
    }
    let fields = match *event {
        HookEvent::Key {
            key,
            phase,
            ctrl,
            shift,
        } => format!(
            r#","key":{},"phase":"{phase:?}","ctrl":{ctrl},"shift":{shift},"handled":{handled}"#,
            json_string(key)
        ),
        HookEvent::Control { down } => format!(r#","key":"control","down":{down}"#),
    };
    write("hook_key", &fields);
}

/// 自测视图里输入框的内容变了。
pub fn input(value: &str) {
    write("input", &format!(r#","value":{}"#, json_string(value)));
}

/// 面板窗口里匹配到的按键（含 action 名）。
pub fn keystroke(keystroke: &str, action: Option<&str>) {
    write(
        "keystroke",
        &format!(
            r#","keystroke":{},"action":{}"#,
            json_string(keystroke),
            action.map_or("null".to_owned(), json_string)
        ),
    );
}

/// 记录面板输入上下文的状态（Windows）。
pub fn ime_state() {
    #[cfg(target_os = "windows")]
    {
        let state = kwikpaste_os::win::ime::state(PANEL.load(Ordering::SeqCst));
        let candidate = state.candidate.map_or("null".to_owned(), |(style, x, y)| {
            format!("[{style},{x},{y}]")
        });
        write(
            "ime",
            &format!(
                r#","attached":{},"open":{},"conversion":{},"composition":{},"candidate":{candidate},"foreground":{}"#,
                state.attached,
                state.open,
                state.conversion,
                json_string(&state.composition),
                kwikpaste_os::win::foreground_window(),
            ),
        );
    }
}

/// 把面板的输入法切到中文模式（Windows；微软拼音在新的输入上下文里默认是英文模式）。
pub fn set_ime_native_mode() {
    #[cfg(target_os = "windows")]
    {
        let switched = kwikpaste_os::win::ime::set_native_mode(PANEL.load(Ordering::SeqCst));
        write("ime_native", &format!(r#","switched":{switched}"#));
    }
}

/// 注入了一次粘贴键。`kind` 是 item / fragment / quick。
pub fn pasted(
    kind: &str,
    id: &str,
    plain: bool,
    report: super::paste::InjectReport,
    elapsed: std::time::Duration,
) {
    write(
        "pasted",
        &format!(
            r#","kind":{},"id":{},"plain":{plain},"panel_was_visible":{},"foreground":{},"elapsed_ms":{:.3}"#,
            json_string(kind),
            json_string(id),
            report.panel_was_visible,
            report.foreground,
            elapsed.as_secs_f64() * 1000.0,
        ),
    );
}

/// 一条记录写回了剪贴板（不粘贴）。
pub fn copied(id: &str, plain: bool, hide_window: bool) {
    write(
        "copied",
        &format!(
            r#","id":{},"plain":{plain},"hide_window":{hide_window}"#,
            json_string(id)
        ),
    );
}

/// 进程即将退出。
pub fn quitting() {
    write("quit", "");
}
