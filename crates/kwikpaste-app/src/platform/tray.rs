//! 托盘：tray-icon + muda（1.x 经 Tauri 用的同一系 crate），在主线程创建，由 GPUI 的消息循环派发。
//!
//! 事件经专用线程阻塞 `recv()` 转进 `async_channel`，线程里只转发、不碰 GPUI。

use async_channel::Sender;
use gpui::{App, AsyncApp, Global};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use super::panel::{PanelCommand, Trigger, TriggerSource};

const TRAY_ID: &str = "app-tray";
const MENU_SHOW: &str = "tray::show";
const MENU_QUIT: &str = "tray::quit";
const ICON: &[u8] = include_bytes!("../../assets/tray.ico");

/// 托盘菜单文案，`[zh-CN, en-US]`。
///
/// TODO：接入 settings 的 `appearance.language` 和 Rust 侧 i18n 模块；之前固定用默认语言 zh-CN。
const SHOW_LABEL: [&str; 2] = ["显示面板", "Show panel"];
const QUIT_LABEL: [&str; 2] = ["退出", "Quit"];
const LANGUAGE: usize = 0;

/// 持有托盘图标：退出前丢弃，任务栏上不留残影。
struct Tray {
    icon: Option<TrayIcon>,
}

impl Global for Tray {}

enum MenuAction {
    Show,
    Quit,
}

/// 创建托盘图标和菜单（显示面板、退出），并启动事件桥。
pub fn create(cx: &mut App, commands: Sender<PanelCommand>) -> anyhow::Result<()> {
    let menu = Menu::new();
    menu.append_items(&[
        &MenuItem::with_id(MENU_SHOW, SHOW_LABEL[LANGUAGE], true, None),
        &MenuItem::with_id(MENU_QUIT, QUIT_LABEL[LANGUAGE], true, None),
    ])?;
    let icon = TrayIconBuilder::new()
        .with_id(TRAY_ID)
        .with_icon(load_icon()?)
        .with_icon_as_template(cfg!(target_os = "macos"))
        .with_menu_on_left_click(cfg!(target_os = "macos"))
        .with_tooltip(crate::identity::display_name())
        .with_menu(Box::new(menu))
        .build()?;

    cx.set_global(Tray { icon: Some(icon) });
    cx.on_app_quit(|cx| {
        if cx.has_global::<Tray>() {
            cx.global_mut::<Tray>().icon = None;
        }
        async {}
    })
    .detach();

    #[cfg(target_os = "windows")]
    bridge_left_click(commands.clone())?;

    let (actions, receiver) = async_channel::unbounded();
    std::thread::Builder::new()
        .name("tray-menu-bridge".to_owned())
        .spawn(move || {
            let events = MenuEvent::receiver();
            while let Ok(event) = events.recv() {
                let action = match event.id.as_ref() {
                    MENU_SHOW => MenuAction::Show,
                    MENU_QUIT => MenuAction::Quit,
                    _ => continue,
                };
                if actions.send_blocking(action).is_err() {
                    break;
                }
            }
        })?;
    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(action) = receiver.recv().await {
            match action {
                MenuAction::Show => {
                    let trigger = Trigger::now(TriggerSource::Tray);
                    let _ = commands.send(PanelCommand::Show(trigger)).await;
                }
                MenuAction::Quit => cx.update(|cx| cx.quit()),
            }
        }
    })
    .detach();

    Ok(())
}

/// Windows 左键单击托盘显示面板（1.x `general.trayClick` 的默认行为）；macOS 左键弹菜单，不走这里。
#[cfg(target_os = "windows")]
fn bridge_left_click(commands: Sender<PanelCommand>) -> std::io::Result<()> {
    use tray_icon::{MouseButton, MouseButtonState, TrayIconEvent};

    std::thread::Builder::new()
        .name("tray-bridge".to_owned())
        .spawn(move || {
            let events = TrayIconEvent::receiver();
            while let Ok(event) = events.recv() {
                let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = event
                else {
                    continue;
                };
                let trigger = Trigger::now(TriggerSource::Tray);
                if commands.send_blocking(PanelCommand::Show(trigger)).is_err() {
                    break;
                }
            }
        })?;

    Ok(())
}

fn load_icon() -> anyhow::Result<Icon> {
    let image = image::load_from_memory_with_format(ICON, image::ImageFormat::Ico)?.into_rgba8();
    let (width, height) = image.dimensions();

    Ok(Icon::from_rgba(image.into_raw(), width, height)?)
}
