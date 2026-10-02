//! 点击面板外部时隐藏（Windows 鼠标钩子）。面板从不激活，收不到失焦通知，只能全局监听按下。
//!
//! macOS 走 `windowDidResignKey`，TODO(macOS)：面板成为 key 窗口之后再接失焦隐藏。

use async_channel::Sender;
use gpui::App;

use super::panel::PanelCommand;

/// 接上鼠标钩子的出口（Windows）：窗外按下即请求隐藏面板。钩子随面板显示、隐藏起停。
#[cfg(target_os = "windows")]
pub fn serve(_cx: &mut App, commands: Sender<PanelCommand>) {
    use kwikpaste_os::win::mouse::{self, MouseEvent};

    use super::panel::{Trigger, TriggerSource};

    let installed = mouse::set_sink(move |event| match event {
        MouseEvent::OutsideClick(_) => {
            let trigger = Trigger::now(TriggerSource::OutsideClick);
            let _ = commands.try_send(PanelCommand::Hide(trigger));
        }
    });
    if let Err(err) = installed {
        log::error!("outside clicks cannot hide the panel: {err}");
    }
}

#[cfg(target_os = "macos")]
pub fn serve(_cx: &mut App, _commands: Sender<PanelCommand>) {}
