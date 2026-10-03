//! 主窗口的视图层（附录 D §3）：头部与搜索框、分组栏、列表（含卡片快捷动作、多选、页脚）。
// render、prepaint、paint 和事件回调里 panic 会让进程以 0xC0000409 中止（附录 D §2.4 第 4 条）。
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)
)]

pub mod bench;
mod card;
mod chrome;
mod editing;
mod frame;
mod group_bar;
mod header;
pub mod host;
mod image_cache;
mod list;
mod panel;
pub mod pin;
pub mod selftest;

use gpui::{Action, App, KeyBinding, actions};
use kwikpaste_ui::INPUT_KEY_CONTEXT;

pub use editing::request_panel;
pub use panel::ClipboardPanel;

/// 主窗口的 key context。Windows 上面板收不到键盘消息，平台层的钩子用 `Window::dispatch_keystroke`
/// 把按键注入面板窗口，按焦点所在的路径匹配这里的绑定；列表和搜索框都在这个 context 之下。
pub const KEY_CONTEXT: &str = "ClipboardPanel";
/// 搜索框外层的 key context：输入框聚焦时 ↑/↓/Enter/Esc/Tab 交给列表（1.x `data-allow-global-keyboard`）。
pub const SEARCH_CONTEXT: &str = "ClipboardSearch";

actions!(
    clipboard_panel,
    [
        /// ↑：当前项上移。
        SelectPrevious,
        /// ↓：当前项下移。
        SelectNext,
        /// Enter：粘贴当前项；多选时勾选 / 取消当前项。
        PasteSelected,
        /// Mod+Enter：粘贴为纯文本 / 路径。
        PasteSelectedPlain,
        /// Esc：按多选、分组、分类、窗口的顺序逐层退出。
        Dismiss,
        /// 搜索框里的 Esc：退出编辑态，焦点回到列表。
        EndSearch,
        /// Mod+F：聚焦搜索框（进入编辑态）。
        FocusSearch,
        /// Mod+Q：切换全部 / 收藏。
        ToggleRange,
        /// ←：上一个分类。
        PreviousCategory,
        /// →：下一个分类。
        NextCategory,
        /// Tab：下一个自定义分组。
        NextGroup,
        /// Shift+Tab：上一个自定义分组。
        PreviousGroup,
        /// Mod+C：复制当前项。
        CopySelected,
        /// Mod+D：收藏 / 取消收藏当前项。
        ToggleFavorite,
        /// Mod+T：置顶 / 取消置顶当前项。
        TogglePinned,
        /// Mod+M：编辑当前项的备注。
        EditNote,
        /// Mod+Backspace / Mod+Delete：删除当前项；多选时删除已勾选的记录。
        DeleteSelected,
        /// Mod+O：打开链接 / 邮箱 / 文件位置。
        OpenSelected,
        /// Mod+S：拆词。
        SplitSelected,
        /// Mod+A：进入多选并全选 / 取消全选。
        SelectAll,
        /// Mod+P：固定 / 取消固定窗口。
        PinWindow,
        /// Mod+,：打开偏好设置。
        OpenPreferences,
        /// Mod+K：快捷键列表。
        ShowShortcuts,
        /// Mod+N：新增分组。
        NewGroup,
    ]
);

/// Mod+数字：粘贴第 N 个可见的非置顶项（`slot` 0–9 对应数字键 1–9、0）。
#[derive(Clone, Debug, PartialEq, Eq, Action)]
#[action(namespace = clipboard_panel, no_json)]
pub struct QuickPaste {
    pub key: char,
}

/// 主窗口的按键绑定（GPUI keystroke 字符串，`secondary` 在 macOS 是 ⌘、其余平台是 Ctrl）。
fn bindings() -> Vec<KeyBinding> {
    let panel = Some(KEY_CONTEXT);
    let search = format!("{SEARCH_CONTEXT} > {INPUT_KEY_CONTEXT}");
    let search = Some(search.as_str());

    let mut bindings = vec![
        KeyBinding::new("up", SelectPrevious, panel),
        KeyBinding::new("down", SelectNext, panel),
        KeyBinding::new("enter", PasteSelected, panel),
        KeyBinding::new("secondary-enter", PasteSelectedPlain, panel),
        KeyBinding::new("escape", Dismiss, panel),
        KeyBinding::new("secondary-f", FocusSearch, panel),
        KeyBinding::new("secondary-q", ToggleRange, panel),
        KeyBinding::new("left", PreviousCategory, panel),
        KeyBinding::new("right", NextCategory, panel),
        KeyBinding::new("tab", NextGroup, panel),
        KeyBinding::new("shift-tab", PreviousGroup, panel),
        KeyBinding::new("secondary-c", CopySelected, panel),
        KeyBinding::new("secondary-d", ToggleFavorite, panel),
        KeyBinding::new("secondary-t", TogglePinned, panel),
        KeyBinding::new("secondary-m", EditNote, panel),
        KeyBinding::new("secondary-backspace", DeleteSelected, panel),
        KeyBinding::new("secondary-delete", DeleteSelected, panel),
        KeyBinding::new("secondary-o", OpenSelected, panel),
        KeyBinding::new("secondary-s", SplitSelected, panel),
        KeyBinding::new("secondary-a", SelectAll, panel),
        KeyBinding::new("secondary-p", PinWindow, panel),
        KeyBinding::new("secondary-,", OpenPreferences, panel),
        KeyBinding::new("secondary-k", ShowShortcuts, panel),
        KeyBinding::new("secondary-n", NewGroup, panel),
        // 搜索框聚焦时交给列表的键：与输入框自己的绑定同深度，这里后注册，优先。
        // ←/→ 和 Mod+A / C / Backspace / Delete 留给输入框（1.x `EDITABLE_GLOBAL_HANDOFF_KEYS`）。
        KeyBinding::new("up", SelectPrevious, search),
        KeyBinding::new("down", SelectNext, search),
        KeyBinding::new("enter", PasteSelected, search),
        KeyBinding::new("secondary-enter", PasteSelectedPlain, search),
        KeyBinding::new("escape", EndSearch, search),
        KeyBinding::new("tab", NextGroup, search),
        KeyBinding::new("shift-tab", PreviousGroup, search),
        KeyBinding::new("secondary-f", FocusSearch, search),
    ];
    for key in ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'] {
        bindings.push(KeyBinding::new(
            &format!("secondary-{key}"),
            QuickPaste { key },
            panel,
        ));
    }

    bindings
}

/// 注册主窗口的按键绑定。在打开面板之前调用一次（在 `kwikpaste_ui::init` 之后，搜索框的改写才能
/// 排在输入框自己的绑定后面）。
pub fn init(cx: &mut App) {
    cx.bind_keys(bindings());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 钩子吞下的每个键都有绑定（不然按下去什么也不发生，目标应用也收不到）。空格是按住预览，
    /// 预览窗在 U2 第二部分。
    #[test]
    fn every_hooked_key_is_bound() {
        let bound: Vec<String> = bindings()
            .iter()
            .filter(|binding| {
                binding
                    .predicate()
                    .is_some_and(|predicate| predicate.to_string() == KEY_CONTEXT)
            })
            .map(|binding| {
                binding
                    .keystrokes()
                    .iter()
                    .map(|keystroke| keystroke.unparse())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect();

        for entry in kwikpaste_os::hook_keys::HOOK_KEYS {
            if entry.key == "space" {
                continue;
            }
            let wanted = match entry.kind {
                kwikpaste_os::hook_keys::HookKeyKind::Ctrl => {
                    gpui::Keystroke::parse(&format!("secondary-{}", entry.key))
                }
                _ => gpui::Keystroke::parse(entry.key),
            }
            .map(|keystroke| keystroke.unparse())
            .unwrap_or_default();
            assert!(bound.contains(&wanted), "{wanted} is hooked but not bound");
        }
    }
}
