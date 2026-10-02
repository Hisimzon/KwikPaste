//! 全局热键：global-hotkey（1.x 的 tauri-plugin-global-shortcut 用的同一个 crate）。
//!
//! 切换面板的热键读设置 `shortcuts.openClipboard`（Tauri accelerator 字面量，如 `Alt+C`，
//! 用 `HotKey::try_from` 解析，与 1.x 一致），设置变更时重新注册。管理器在主线程创建，
//! `WM_HOTKEY` / Carbon 事件由 GPUI 的消息循环顺带派发；事件经「专用线程阻塞 `recv()` →
//! `async_channel` → 面板命令循环」送回，不轮询，线程里只转发、不碰 GPUI。
//!
//! TODO：`shortcuts.openPreference` 等偏好窗建好后再注册。

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use async_channel::Sender;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui::{App, Global};
use kwikpaste_core::settings::Shortcuts;

use super::panel::{PanelCommand, Trigger, TriggerSource};
use crate::core_host;

/// 开发构建在设置仍是出厂默认值时改用的热键。出厂默认的 Alt+C 属于本机正在运行的 1.x，
/// 开发版抢先注册会让 1.x 重启后注册失败；Ctrl+Alt+Shift+F9 两边都不用。
const DEVELOPMENT_TOGGLE: &str = "Control+Alt+Shift+F9";

/// 持有管理器（丢弃时注销全部热键）和当前注册的热键。
struct Hotkeys {
    manager: GlobalHotKeyManager,
    registered: Option<HotKey>,
    /// 事件桥线程据此认出切换面板的热键；0 表示没有注册。
    toggle_id: Arc<AtomicU32>,
}

impl Global for Hotkeys {}

/// 创建管理器、启动事件桥，按当前设置注册热键。
pub fn register(cx: &mut App, commands: Sender<PanelCommand>) -> anyhow::Result<()> {
    let manager = GlobalHotKeyManager::new()?;
    let toggle_id = Arc::new(AtomicU32::new(0));

    let bridge_id = toggle_id.clone();
    std::thread::Builder::new()
        .name("hotkey-bridge".to_owned())
        .spawn(move || {
            let receiver = GlobalHotKeyEvent::receiver();
            while let Ok(event) = receiver.recv() {
                let toggle = bridge_id.load(Ordering::SeqCst);
                if toggle == 0 || event.id != toggle || event.state != HotKeyState::Pressed {
                    continue;
                }
                let trigger = Trigger::now(TriggerSource::Hotkey);
                if commands
                    .send_blocking(PanelCommand::Toggle(trigger))
                    .is_err()
                {
                    break;
                }
            }
        })?;

    cx.set_global(Hotkeys {
        manager,
        registered: None,
        toggle_id,
    });
    if let Some(core) = core_host::core(cx) {
        let shortcuts = core.settings().shortcuts;
        apply(&shortcuts, cx);
    }

    Ok(())
}

/// 按设置重新注册切换面板的热键；与当前相同时什么也不做。
pub fn apply(shortcuts: &Shortcuts, cx: &mut App) {
    let Some(hotkeys) = cx.try_global::<Hotkeys>() else {
        return;
    };
    let accelerator = effective_accelerator(shortcuts);
    let wanted = match parse(&accelerator) {
        Ok(wanted) => wanted,
        Err(err) => {
            log::error!("panel hotkey {accelerator:?} is invalid: {err}");
            None
        }
    };
    if hotkeys.registered == wanted {
        return;
    }

    let hotkeys = cx.global_mut::<Hotkeys>();
    if let Some(previous) = hotkeys.registered.take()
        && let Err(err) = hotkeys.manager.unregister(previous)
    {
        log::warn!("panel hotkey {previous} could not be unregistered: {err}");
    }
    hotkeys.toggle_id.store(0, Ordering::SeqCst);

    let Some(hotkey) = wanted else {
        log::info!("panel hotkey cleared");
        return;
    };
    match hotkeys.manager.register(hotkey) {
        Ok(()) => {
            hotkeys.registered = Some(hotkey);
            hotkeys.toggle_id.store(hotkey.id(), Ordering::SeqCst);
            log::info!("panel hotkey registered: {hotkey}");
        }
        Err(err) => log::error!("panel hotkey {hotkey} could not be registered: {err}"),
    }
}

/// 空字符串表示用户清空了快捷键。
fn parse(accelerator: &str) -> Result<Option<HotKey>, global_hotkey::hotkey::HotKeyParseError> {
    if accelerator.trim().is_empty() {
        return Ok(None);
    }
    HotKey::try_from(accelerator).map(Some)
}

/// 实际注册的快捷键：正式版就是设置值；开发版在设置仍是出厂默认值时换成 [`DEVELOPMENT_TOGGLE`]。
fn effective_accelerator(shortcuts: &Shortcuts) -> String {
    let default = Shortcuts::default().open_clipboard;
    if cfg!(feature = "production-identity") || shortcuts.open_clipboard != default {
        return shortcuts.open_clipboard.clone();
    }

    DEVELOPMENT_TOGGLE.to_owned()
}

#[cfg(test)]
mod tests {
    use global_hotkey::hotkey::{Code, Modifiers};

    use super::*;

    #[test]
    fn tauri_accelerators_parse_like_1x() {
        let parsed = |text: &str| parse(text).expect("valid").expect("not empty");

        assert_eq!(
            parsed("Alt+C"),
            HotKey::new(Some(Modifiers::ALT), Code::KeyC)
        );
        assert_eq!(
            parsed(DEVELOPMENT_TOGGLE),
            HotKey::new(
                Some(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SHIFT),
                Code::F9
            )
        );
        assert_eq!(parse("  ").expect("empty is allowed"), None);
        assert!(parse("Alt+").is_err());
    }

    #[cfg(not(feature = "production-identity"))]
    #[test]
    fn development_builds_never_take_the_shipped_default() {
        let mut shortcuts = Shortcuts::default();
        assert_eq!(effective_accelerator(&shortcuts), DEVELOPMENT_TOGGLE);

        shortcuts.open_clipboard = "Control+Shift+F8".to_owned();
        assert_eq!(effective_accelerator(&shortcuts), "Control+Shift+F8");
    }
}
