//! 全局热键：global-hotkey（1.x 的 tauri-plugin-global-shortcut 用的同一个 crate）。
//!
//! 管理器在主线程创建，`WM_HOTKEY` / Carbon 事件由 GPUI 的消息循环顺带派发。事件经
//! 「专用线程阻塞 `recv()` → `async_channel` → 面板命令循环」送回，不轮询；线程里只转发，不碰 GPUI。

use async_channel::Sender;
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui::{App, Global};

use super::panel::{PanelCommand, Trigger, TriggerSource};

/// 开发版切换面板的热键。避开本机 1.x 正在用的 Alt+C / Alt+X 和快速粘贴的「修饰键 + 数字」。
///
/// TODO：正式版改为注册 settings 里 `shortcuts.openClipboard` 的 Tauri accelerator 字面量（默认 Alt+C）。
const TOGGLE_PANEL: (Modifiers, Code) = (
    Modifiers::CONTROL
        .union(Modifiers::ALT)
        .union(Modifiers::SHIFT),
    Code::F9,
);

/// 持有管理器：丢弃时注销全部热键。
struct Hotkeys {
    _manager: GlobalHotKeyManager,
}

impl Global for Hotkeys {}

/// 注册切换面板的热键并启动事件桥。热键被其它程序占用时返回错误。
pub fn register(cx: &mut App, commands: Sender<PanelCommand>) -> anyhow::Result<()> {
    let manager = GlobalHotKeyManager::new()?;
    let hotkey = HotKey::new(Some(TOGGLE_PANEL.0), TOGGLE_PANEL.1);
    manager.register(hotkey)?;
    cx.set_global(Hotkeys { _manager: manager });

    let id = hotkey.id();
    std::thread::Builder::new()
        .name("hotkey-bridge".to_owned())
        .spawn(move || {
            let receiver = GlobalHotKeyEvent::receiver();
            while let Ok(event) = receiver.recv() {
                if event.id != id || event.state != HotKeyState::Pressed {
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

    log::info!("panel hotkey registered: {hotkey}");
    Ok(())
}
