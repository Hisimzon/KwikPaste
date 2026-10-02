//! 自测开关：必须同时传 `--selftest-*` 参数并设置 `KWIKPASTE_SELFTEST=1`，普通启动不会误触。
//!
//! 自测进程用带 `.selftest` 后缀的 identifier（见 [`crate::identity`]），把日志打到 stderr。

use std::cell::Cell;
use std::io::Write as _;
use std::rc::Rc;
use std::time::Duration;

use gpui::App;

use crate::platform::{self, Panel, PanelCommand, PanelEvent, Trigger, TriggerSource};

const ENV: &str = "KWIKPASTE_SELFTEST";
const PREFIX: &str = "--selftest-";

/// 冒烟：显示面板，几秒后检查确实出过帧并正常退出。CI 看退出码。
pub const SMOKE: &str = "--selftest-smoke";
/// 平台探针：写探针日志（见 `platform::probe`），并接受后启动的自测实例转交的下面几条命令。
pub const PLATFORM: &str = "--selftest-platform";
pub const SHOW: &str = "--selftest-show";
pub const HIDE: &str = "--selftest-hide";
pub const TOGGLE: &str = "--selftest-toggle";
pub const QUIT: &str = "--selftest-quit";
/// 以键盘触发的方式进入编辑态（走吞 Alt 取前台）、退出编辑态。
pub const EDIT: &str = "--selftest-edit";
pub const END_EDIT: &str = "--selftest-end-edit";
/// 把面板输入上下文的状态写进探针日志；把面板的输入法切到中文模式。
pub const IME_STATE: &str = "--selftest-ime-state";
pub const IME_NATIVE: &str = "--selftest-ime-native";
/// `--selftest-settings=<JSON patch>`：经 core 更新设置。
pub const SETTINGS: &str = "--selftest-settings=";
/// 组件展示窗：代替面板打开 gallery，不启动托盘、热键和面板。
pub const GALLERY: &str = "--selftest-gallery";

const SMOKE_DURATION: Duration = Duration::from_secs(3);
/// 平台探针最长运行时间：测量脚本中途出错时不留下进程。
const PLATFORM_WATCHDOG: Duration = Duration::from_secs(15 * 60);

/// 本进程是否处于任一自测模式。
pub fn active() -> bool {
    env_enabled() && std::env::args().any(|arg| arg.starts_with(PREFIX))
}

/// 本进程是否打开了某个自测开关。
pub fn enabled(flag: &str) -> bool {
    env_enabled() && std::env::args().any(|arg| arg == flag)
}

/// 是否打开组件展示窗（`--selftest-gallery`）。
pub fn gallery_requested() -> bool {
    enabled(GALLERY)
}

fn env_enabled() -> bool {
    std::env::var_os(ENV).is_some_and(|value| value == "1")
}

/// 自测进程把日志打到 stderr：本 workspace 的 crate 到 debug，其余到 warn。
pub fn init_logging() {
    if !active() {
        return;
    }
    if log::set_boxed_logger(Box::new(StderrLogger)).is_ok() {
        log::set_max_level(log::LevelFilter::Debug);
    }
}

struct StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // 二进制 crate 的日志 target 以 `KwikPaste::` 开头，库 crate 以 `kwikpaste_` 开头。
        let ours = metadata
            .target()
            .get(..9)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("kwikpaste"));
        let max = if ours {
            log::Level::Debug
        } else {
            log::Level::Warn
        };
        metadata.level() <= max
    }

    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            let _ = writeln!(
                std::io::stderr().lock(),
                "[{} {}] {}",
                record.level(),
                record.target(),
                record.args()
            );
        }
    }

    fn flush(&self) {}
}

/// 按开关安排自测流程。在 `platform::start` 之后调用。
pub fn schedule(cx: &mut App) {
    if enabled(SMOKE) {
        smoke(cx);
    }
    if enabled(PLATFORM) {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(PLATFORM_WATCHDOG).await;
            log::warn!("platform selftest ran for {PLATFORM_WATCHDOG:?}; quitting");
            cx.update(|cx| cx.quit());
        })
        .detach();
    }
}

/// 显示面板，等几秒后检查收到了 `PanelEvent::Shown` 且渲染过帧，再正常退出。
///
/// Windows 上没出帧就以退出码 1 结束；macOS 只能靠 CI 验证，暂时只记日志。
fn smoke(cx: &mut App) {
    let Some(panel) = cx.try_global::<Panel>() else {
        log::error!("smoke selftest: the panel was not created");
        std::process::exit(1);
    };
    let events = panel.events().clone();
    panel.request(PanelCommand::Show(Trigger::now(TriggerSource::Selftest)));

    let shown = Rc::new(Cell::new(false));
    let subscription = cx.subscribe(&events, {
        let shown = shown.clone();
        move |_, event: &PanelEvent, _| {
            if *event == PanelEvent::Shown {
                shown.set(true);
            }
        }
    });

    cx.spawn(async move |cx| {
        cx.background_executor().timer(SMOKE_DURATION).await;
        drop(subscription);

        let frames = platform::rendered_frames();
        let passed = shown.get() && frames > 0;
        log::info!(
            "smoke selftest: panel shown={} frames={frames}",
            shown.get()
        );
        if !passed && cfg!(target_os = "windows") {
            log::error!("smoke selftest failed: the panel never showed a frame");
            std::process::exit(1);
        }
        cx.update(|cx| cx.quit());
    })
    .detach();
}
