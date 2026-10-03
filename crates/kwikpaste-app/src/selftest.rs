//! 自测开关：必须同时传 `--selftest-*` 参数并设置 `KWIKPASTE_SELFTEST=1`，普通启动不会误触。
//!
//! 自测进程按种类用 `.selftest-<kind>` 后缀的 identifier（见 [`kind`] 与 [`crate::identity`]），
//! 把日志打到 stderr。

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
/// 平台探针改用本机系统剪贴板并开始监听（默认是内存剪贴板、不监听）。只给真机剪贴板验证用：
/// 跑之前要先退出本机的 1.x，免得测试内容进了用户的历史。
pub const REAL_CLIPBOARD: &str = "--selftest-real-clipboard";
/// `--selftest-copy-item=<id>`：把一条记录写回剪贴板（不粘贴），走 `platform::paste::copy`。
pub const COPY_ITEM: &str = "--selftest-copy-item=";
/// 手动读取一次当前剪贴板并入库（`Core::read_clipboard_now`）。
pub const READ_NOW: &str = "--selftest-read-now";
/// `--selftest-handoff=<code>`：演练更新交接的宿主步骤后以 `code` 退出（见 `platform::updater`）。
pub const HANDOFF: &str = "--selftest-handoff=";
/// 把历史记录总数写进探针日志。
pub const COUNT: &str = "--selftest-count";
/// 平台探针的面板放正式的列表（UI 的 `build_panel`），不放平台自测视图；验证列表的粘贴意图用。
pub const UI_PANEL: &str = "--selftest-ui-panel";
/// 崩溃重启自测：自测进程崩溃时默认只记录不重启，带这个开关才按正式策略重启
/// （`tools/platform-probes/crash-restart.ps1`）。
pub const CRASH_RESTART: &str = "--selftest-crash-restart";
/// `--selftest-panic=main|thread`：在主线程（前台任务里）或一个新线程上 panic。
pub const PANIC: &str = "--selftest-panic=";
/// `--selftest-drag-payload=<JSON>`：平台自测视图按住拖动时拖出的内容，
/// `{"plain": …, "html": …, "rtf": …}` 或 `{"files": […]}`。
pub const DRAG_PAYLOAD: &str = "--selftest-drag-payload=";
/// `--selftest-seed=<n>`：往平台自测进程的 core 灌 n 条合成记录（含真实尺寸和 4K 图片），内存验收用。
pub const SEED: &str = "--selftest-seed=";
/// 让看门狗认为 vsync 线程已死，下一次显示面板时有序重启。
pub const VSYNC_DEAD: &str = "--selftest-vsync-dead";
/// `--selftest-device-lost=<n>`：模拟一次 GPU 设备丢失，前 n 次重建全局设备失败（补丁 0003 的注入点）。
pub const DEVICE_LOST: &str = "--selftest-device-lost=";
/// `--selftest-async-frame=<ms>`：过 ms 毫秒后不经输入把面板标脏，记录到下一次渲染隔了多久（探针事件 `async_frame`）。
pub const ASYNC_FRAME: &str = "--selftest-async-frame=";
/// 组件展示窗：代替面板打开 gallery，不启动托盘、热键和面板。
pub const GALLERY: &str = "--selftest-gallery";
/// 列表跑分：1 万行合成数据，附录 D §3.7 的门槛与锚定场景（见 `clipboard::view::bench`）。
pub const LIST_BENCH: &str = "--selftest-list-bench";
/// 列表演示：显示面板并保持打开，供截图核对（示例夹具；`KP_GALLERY_THEME` 等环境变量同展示窗）。
pub const LIST_DEMO: &str = "--selftest-list-demo";
/// 列表数据改由临时目录里的真 core 提供（灌入合成记录），可与 `--selftest-list-demo` 合用。
pub const CORE_LIST: &str = "--selftest-core-list";
/// 主窗口交互自测：示例夹具上按脚本派发按键、检查状态（见 `clipboard::view::selftest`），退出码表示结果。
pub const PANEL_UI: &str = "--selftest-panel-ui";

const SMOKE_DURATION: Duration = Duration::from_secs(3);
/// 平台探针最长运行时间：测量脚本中途出错时不留下进程。
const PLATFORM_WATCHDOG: Duration = Duration::from_secs(15 * 60);
/// 列表跑分和演示的最长运行时间。
const LIST_WATCHDOG: Duration = Duration::from_secs(5 * 60);

/// 本进程是否处于任一自测模式。
pub fn active() -> bool {
    env_enabled() && std::env::args().any(|arg| arg.starts_with(PREFIX))
}

/// 这次自测的种类，用作 identifier 的后缀（`….selftest-<kind>`）：每种自测各有自己的单实例名字和
/// 数据目录，同时跑的冒烟、展示窗、列表跑分和平台探针互不转交参数、互不共用数据。
/// 平台探针本身和给它转交命令的后启动实例是同一种（`platform`）。不在自测模式时为 `None`。
pub fn kind() -> Option<&'static str> {
    if !active() {
        return None;
    }
    if platform_probe() {
        return Some("platform");
    }
    let kinds = [
        (SMOKE, "smoke"),
        (GALLERY, "gallery"),
        (LIST_BENCH, "list-bench"),
        (LIST_DEMO, "list-demo"),
        (CORE_LIST, "core-list"),
        (PANEL_UI, "panel-ui"),
    ];

    Some(
        kinds
            .into_iter()
            .find(|(flag, _)| enabled(flag))
            .map_or("other", |(_, kind)| kind),
    )
}

/// 本进程是平台探针本身（`--selftest-platform`）或者给它转交命令的后启动实例。
fn platform_probe() -> bool {
    const COMMANDS: [&str; 13] = [
        COUNT,
        VSYNC_DEAD,
        PLATFORM,
        SHOW,
        HIDE,
        TOGGLE,
        QUIT,
        EDIT,
        END_EDIT,
        IME_STATE,
        IME_NATIVE,
        REAL_CLIPBOARD,
        READ_NOW,
    ];

    env_enabled()
        && std::env::args().any(|arg| {
            COMMANDS.contains(&arg.as_str())
                || arg.starts_with(SETTINGS)
                || arg.starts_with(COPY_ITEM)
                || arg.starts_with(HANDOFF)
                || arg.starts_with(PANIC)
                || arg.starts_with(DRAG_PAYLOAD)
                || arg.starts_with(SEED)
                || arg.starts_with(DEVICE_LOST)
                || arg.starts_with(ASYNC_FRAME)
        })
}

/// 本进程是否打开了某个自测开关。
pub fn enabled(flag: &str) -> bool {
    env_enabled() && std::env::args().any(|arg| arg == flag)
}

/// 是否打开组件展示窗（`--selftest-gallery`）。
pub fn gallery_requested() -> bool {
    enabled(GALLERY)
}

/// 是否是列表自测（跑分、演示或主窗口交互自测）。
pub fn list_selftest() -> bool {
    enabled(LIST_BENCH) || enabled(LIST_DEMO) || enabled(PANEL_UI)
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
    if (enabled(LIST_DEMO) || enabled(PANEL_UI))
        && let Some(panel) = cx.try_global::<Panel>()
    {
        panel.request(PanelCommand::Show(Trigger::now(TriggerSource::Selftest)));
    }
    if list_selftest() {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(LIST_WATCHDOG).await;
            log::warn!("list selftest ran for {LIST_WATCHDOG:?}; quitting");
            std::process::exit(4);
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
