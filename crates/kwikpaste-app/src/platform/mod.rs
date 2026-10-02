//! 平台层的 GPUI 接线：创建平台、单实例、core、剪贴板面板、键盘与鼠标钩子、编辑态、
//! 粘贴链路、全局热键、托盘和系统设置信号。
//!
//! 启动顺序（附录 C §4.3 的子集）：[`launch`] 在创建 GPU 设备之前判重（第二实例把参数转交给
//! 主实例后直接退出）并启动 core；[`create`] 创建 GPUI 平台；[`start`] 在 `Application::run`
//! 回调里设全局行为、预创建隐藏的面板、注册热键和托盘、接上 core 的设置事件。
//!
//! 只需要原生句柄的部分在 `kwikpaste-os`，这里只放要碰 `gpui::*` 的胶水。所有原生窗口调用
//! 都在 `cx.spawn` 的任务体里、GPUI 借用之外进行。
//!
//! # UI 怎么接
//! - 面板显示 / 隐藏、编辑态进出：订阅 [`Panel::events`] 发出的 [`PanelEvent`]（面板窗口打开之前
//!   就已挂上，UI 在 `build_panel` 里构造视图时即可订阅）。收到 `Shown` 时把 GPUI 焦点放到列表
//!   （带 key context 的元素）上，钩子转来的按键才会命中列表的绑定。
//! - 非编辑态的键盘（Windows）：表 `kwikpaste_os::hook_keys::HOOK_KEYS` 里的键由钩子截下，
//!   以普通 GPUI 按键派发给面板（`window.dispatch_keystroke`），UI 照常 `bind_keys` 即可；
//!   空格另有 `KeyUp`，Ctrl 的按下松开以 `ModifiersChanged` 送达。[`hook_keystrokes`] 列出全部组合。
//! - 编辑态：输入框外层 `capture_any_mouse_down` 里 [`request`] `BeginEditing(EditTrigger::Mouse)`，
//!   Ctrl+F 之类键盘触发用 `EditTrigger::Keyboard`；收到 `EditingStarted` 后再聚焦输入框，
//!   `EditingRefused` 表示没拿到前台（留在列表）。退出时 [`request`] `EndEditing`，
//!   收到 `EditingEnded` 把焦点还给列表。面板隐藏会自动结束编辑态。
//! - 系统信号：[`SystemSignals`] 全局（文本大小、高对比度、减少动画），`cx.observe_global` 订阅；
//!   文本大小已经同步给 `kwikpaste_ui::theme::set_text_scale`，减少动画已写进 `cx.reduce_motion()`。
//! - 粘贴、复制：列表的意图交给 [`paste`] 模块（[`paste::paste`]、[`paste::paste_fragment`]、
//!   [`paste::copy`]），流程与时序见该模块文档；全局快速粘贴由热键直接走 [`paste::quick_paste`]。
//! - core：`crate::core_host::core(cx)` 取 `Core`；[`CoreEvents`] 转发 core 的全部事件。

mod editing;
mod hotkey;
mod instance;
mod keyboard;
mod mouse;
mod panel;
pub mod paste;
mod probe;
mod probe_view;
mod settings;
mod system;
mod tray;
mod updater;
mod window_state;

#[cfg(target_os = "macos")]
#[path = "native_macos.rs"]
mod native;
#[cfg(target_os = "windows")]
#[path = "native_windows.rs"]
mod native;

use std::rc::Rc;

use anyhow::Context as _;
use gpui::{App, AppContext as _, CursorHideMode, Entity, Platform, QuitMode, Render, Window};
use kwikpaste_os::single_instance::{self, Claim, Invocation, PrimaryInstance};

use crate::core_host::{self, StartedCore};
use crate::selftest;

#[allow(unused_imports, reason = "UI 接线用的接口，见本模块文档")]
pub use editing::EditTrigger;
pub use panel::{Panel, PanelCommand, PanelEvent, Trigger, TriggerSource, rendered_frames};
#[allow(unused_imports, reason = "UI 接线用的接口，见本模块文档")]
pub use settings::{CoreEvents, core_events};
#[allow(unused_imports, reason = "UI 接线用的接口，见本模块文档")]
pub use system::SystemSignals;
pub use updater::exit_code;

/// 判重通过后带进 GPUI 的启动状态。
pub struct Launch {
    instance: PrimaryInstance,
    invocations: async_channel::Receiver<Invocation>,
    core: StartedCore,
}

/// 单实例判重并启动 core。本进程是第二实例时把参数转交给主实例并返回 `None`，调用方应直接退出。
///
/// 必须在 [`create`] 之前调用：第二实例不初始化 GPU，也不碰数据库。
pub fn launch() -> anyhow::Result<Option<Launch>> {
    selftest::init_logging();

    let identifier = crate::identity::identifier();
    let (sender, invocations) = async_channel::unbounded();
    let claim = single_instance::claim(identifier, move |invocation| {
        let _ = sender.try_send(invocation);
    })
    .with_context(|| format!("single instance check for {identifier}"))?;

    let instance = match claim {
        Claim::Primary(instance) => instance,
        Claim::Forwarded => {
            log::info!("{identifier} is already running; handed the arguments over");
            return Ok(None);
        }
    };
    let core = core_host::start()?;

    Ok(Some(Launch {
        instance,
        invocations,
        core,
    }))
}

/// 创建 GPUI 平台。
///
/// Windows 上不走 `gpui_platform::application()`：它对 `WindowsPlatform::new` 的错误直接 panic，
/// 这里自己调用、拿到 `Result`，初始化失败时才有机会告诉用户原因。
#[cfg(target_os = "windows")]
pub fn create() -> anyhow::Result<Rc<dyn Platform>> {
    let platform = gpui_windows::WindowsPlatform::new(false)?;

    Ok(Rc::new(platform))
}

#[cfg(target_os = "macos")]
pub fn create() -> anyhow::Result<Rc<dyn Platform>> {
    Ok(gpui_platform::current_platform(false))
}

/// 在 `Application::run` 回调里调用：设全局行为，接上 core，预创建隐藏的面板，注册热键和托盘。
///
/// `--selftest-platform` 下面板放平台自测视图，不用 `build_panel`。只有面板创建失败才返回错误；
/// 热键、托盘失败只记日志，应用照常运行。
pub fn start<V: Render>(
    cx: &mut App,
    launch: Launch,
    build_panel: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    if let Err(err) = kwikpaste_os::mac::panel::use_accessory_activation_policy() {
        log::warn!("could not switch to the accessory activation policy: {err}");
    }
    cx.set_quit_mode(QuitMode::Explicit);
    // 钩子派发的按键会命中 action，默认模式会因此隐藏停在面板上的鼠标指针。
    cx.set_cursor_hide_mode(CursorHideMode::Never);
    probe::init();

    let StartedCore { host, events } = launch.core;
    cx.set_global(host);
    settings::serve(cx, events);
    settings::apply_language(cx);
    let text_scale = system::init(cx);
    if let Some(core) = core_host::core(cx) {
        window_state::migrate_legacy(core);
    }

    if selftest::enabled(selftest::PLATFORM) {
        panel::open(cx, text_scale, |window, cx| {
            cx.new(|cx| probe_view::ProbeView::new(window, cx))
        })?;
    } else {
        panel::open(cx, text_scale, build_panel)?;
    }
    let commands = cx.global::<Panel>().commands();
    keyboard::serve(cx);
    mouse::serve(cx, commands.clone());
    system::serve(cx, commands.clone());

    // 列表自测（跑分、截图）不碰全局热键和托盘：热键是系统范围独占的，会抢走同时在跑的平台探针
    // 或手动开着的开发实例的热键。
    if !crate::selftest::list_selftest() {
        if let Err(err) = hotkey::register(cx, commands.clone()) {
            log::error!("global hotkey is unavailable: {err:#}");
        }
        if let Err(err) = tray::create(cx, commands.clone()) {
            log::error!("tray icon is unavailable: {err:#}");
        }
    }
    settings::follow(cx);
    probe::follow_clipboard(cx);
    instance::serve(cx, launch.instance, launch.invocations, commands);
    updater::start(cx);

    Ok(())
}

/// 请求显示、隐藏、切换面板或进出编辑态。
pub fn request(cx: &App, command: PanelCommand) {
    if let Some(panel) = cx.try_global::<Panel>() {
        panel.request(command);
    }
}

/// 钩子会派发给面板的全部按键组合（GPUI keystroke 字符串），供 UI 核对绑定。
pub fn hook_keystrokes() -> Vec<String> {
    kwikpaste_os::hook_keys::keystrokes()
}
