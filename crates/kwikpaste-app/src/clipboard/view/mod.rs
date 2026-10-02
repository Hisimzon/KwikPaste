//! 主窗口列表的视图层（附录 D §3.1）。
// render、prepaint、paint 和事件回调里 panic 会让进程以 0xC0000409 中止（附录 D §2.4 第 4 条）。
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)
)]

pub mod bench;
mod card;
mod frame;
mod image_cache;
mod list;

use gpui::{App, KeyBinding, actions};

pub use list::{ClipboardList, ListIntent};

/// 列表的 key context。Windows 上面板收不到键盘消息，平台层的钩子用 `Window::dispatch_keystroke`
/// 把按键注入面板窗口时，只要列表持有焦点（面板显示时它会自己拿焦点），就会命中这里的绑定。
pub const KEY_CONTEXT: &str = "ClipboardList";

actions!(
    clipboard_list,
    [
        /// ↑：当前项上移。
        SelectPrevious,
        /// ↓：当前项下移。
        SelectNext,
        /// Enter：粘贴当前项。
        PasteSelected,
        /// Mod+Enter：粘贴为纯文本 / 路径。
        PasteSelectedPlain,
        /// Esc：U1 只有最后一层（隐藏窗口）。
        Dismiss,
    ]
);

/// 注册列表的按键绑定。在打开面板之前调用一次。
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("up", SelectPrevious, Some(KEY_CONTEXT)),
        KeyBinding::new("down", SelectNext, Some(KEY_CONTEXT)),
        KeyBinding::new("enter", PasteSelected, Some(KEY_CONTEXT)),
        KeyBinding::new("secondary-enter", PasteSelectedPlain, Some(KEY_CONTEXT)),
        KeyBinding::new("escape", Dismiss, Some(KEY_CONTEXT)),
    ]);
}
