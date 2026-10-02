//! 平台层的 GPUI 接线：创建平台、单实例、剪贴板面板、全局热键和托盘。
//!
//! 启动顺序（附录 C §4.3 的子集）：[`launch`] 在创建 GPU 设备之前判重，第二实例把参数转交给
//! 主实例后直接退出；[`create`] 创建 GPUI 平台；[`start`] 在 `Application::run` 回调里设全局行为、
//! 预创建隐藏的面板、注册热键和托盘。
//!
//! 只需要原生句柄的部分在 `kwikpaste-os`，这里只放要碰 `gpui::*` 的胶水。所有原生窗口调用
//! 都在 `cx.spawn` 的任务体里、GPUI 借用之外进行。

mod hotkey;
mod instance;
mod panel;
mod probe;
mod tray;

#[cfg(target_os = "macos")]
#[path = "native_macos.rs"]
mod native;
#[cfg(target_os = "windows")]
#[path = "native_windows.rs"]
mod native;

use std::rc::Rc;

use anyhow::Context as _;
use gpui::{App, CursorHideMode, Entity, Platform, QuitMode, Render, Window};
use kwikpaste_os::single_instance::{self, Claim, Invocation, PrimaryInstance};

pub use panel::{Panel, PanelCommand, PanelEvent, Trigger, TriggerSource, rendered_frames};

/// 判重通过后带进 GPUI 的启动状态。
pub struct Launch {
    instance: PrimaryInstance,
    invocations: async_channel::Receiver<Invocation>,
}

/// 单实例判重。本进程是第二实例时把参数转交给主实例并返回 `None`，调用方应直接退出。
///
/// 必须在 [`create`] 之前调用：第二实例不初始化 GPU。
pub fn launch() -> anyhow::Result<Option<Launch>> {
    crate::selftest::init_logging();

    let identifier = crate::identity::identifier();
    let (sender, invocations) = async_channel::unbounded();
    let claim = single_instance::claim(&identifier, move |invocation| {
        let _ = sender.try_send(invocation);
    })
    .with_context(|| format!("single instance check for {identifier}"))?;

    match claim {
        Claim::Primary(instance) => Ok(Some(Launch {
            instance,
            invocations,
        })),
        Claim::Forwarded => {
            log::info!("{identifier} is already running; handed the arguments over");
            Ok(None)
        }
    }
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

/// 在 `Application::run` 回调里调用：设全局行为，预创建隐藏的面板，注册热键和托盘。
///
/// 只有面板创建失败才返回错误；热键、托盘失败只记日志，应用照常运行。
pub fn start<V: Render>(
    cx: &mut App,
    launch: Launch,
    build_panel: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> anyhow::Result<Entity<V>> {
    #[cfg(target_os = "macos")]
    if let Err(err) = kwikpaste_os::mac::panel::use_accessory_activation_policy() {
        log::warn!("could not switch to the accessory activation policy: {err}");
    }
    cx.set_quit_mode(QuitMode::Explicit);
    // 钩子派发的按键会命中 action，默认模式会因此隐藏停在面板上的鼠标指针。
    cx.set_cursor_hide_mode(CursorHideMode::Never);
    probe::init();

    let content = panel::open(cx, build_panel)?;
    let commands = cx.global::<Panel>().commands();

    if let Err(err) = hotkey::register(cx, commands.clone()) {
        log::error!("global hotkey is unavailable: {err:#}");
    }
    if let Err(err) = tray::create(cx, commands.clone()) {
        log::error!("tray icon is unavailable: {err:#}");
    }
    instance::serve(cx, launch.instance, launch.invocations, commands);

    Ok(content)
}
