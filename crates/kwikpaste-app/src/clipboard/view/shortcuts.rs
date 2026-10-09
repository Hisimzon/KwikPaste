//! 主面板快捷键说明。

use gpui::{
    AnyElement, App, Context, InteractiveElement as _, IntoElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
    rems,
};
use kwikpaste_ui::theme::{TextSize, space};
use kwikpaste_ui::{DialogSpec, KpStyled as _, Shortcut, form_dialog, theme};

use super::panel::ClipboardPanel;
use crate::{clipboard::model::shortcut, i18n::t};

const SHORTCUTS: &[(&str, &[&str])] = &[
    ("clipboard:shortcuts.pasteSelected", &["Enter"]),
    (
        "clipboard:shortcuts.pasteSelectedPlain",
        &["CmdOrCtrl", "Enter"],
    ),
    ("clipboard:shortcuts.pasteNth", &["CmdOrCtrl", "1-0"]),
    ("clipboard:shortcuts.previewSelected", &["Space"]),
    ("clipboard:shortcuts.copySelected", &["CmdOrCtrl", "C"]),
    ("clipboard:shortcuts.splitSelected", &["CmdOrCtrl", "S"]),
    ("clipboard:shortcuts.openSelected", &["CmdOrCtrl", "O"]),
    ("clipboard:shortcuts.noteSelected", &["CmdOrCtrl", "M"]),
    ("clipboard:shortcuts.favoriteSelected", &["CmdOrCtrl", "D"]),
    ("clipboard:shortcuts.pinSelected", &["CmdOrCtrl", "T"]),
    (
        "clipboard:shortcuts.deleteSelected",
        if cfg!(target_os = "macos") {
            &["CmdOrCtrl", "Backspace"]
        } else {
            &["Delete"]
        },
    ),
    ("clipboard:shortcuts.selectAll", &["CmdOrCtrl", "A"]),
    ("clipboard:shortcuts.navigate", &["↑", "/", "↓"]),
    ("clipboard:shortcuts.focusSearch", &["CmdOrCtrl", "F"]),
    ("clipboard:shortcuts.toggleRange", &["CmdOrCtrl", "Q"]),
    ("clipboard:shortcuts.switchCategory", &["←", "/", "→"]),
    (
        "clipboard:shortcuts.switchCustomGroup",
        &["Tab", "/", "Shift", "Tab"],
    ),
    ("clipboard:shortcuts.createGroup", &["CmdOrCtrl", "N"]),
    ("clipboard:shortcuts.pinWindow", &["CmdOrCtrl", "P"]),
    ("clipboard:shortcuts.showShortcuts", &["CmdOrCtrl", "K"]),
    ("clipboard:shortcuts.openPreference", &["CmdOrCtrl", ","]),
    ("clipboard:shortcuts.closePreviewFilterWindow", &["Escape"]),
];

/// 弹框标题、按钮和上下内边距大约占的高度：列表最高为窗口高减去这些，放不下时在框内滚动。
const DIALOG_CHROME: f32 = 200.;

/// 在当前面板窗口打开快捷键列表。只读说明，只有一个确定按钮。
pub fn show(window: &mut Window, cx: &mut Context<ClipboardPanel>) {
    drop(form_dialog(
        DialogSpec::new(t("clipboard:shortcuts.title"))
            .ok_text(t("common:ui.ok"))
            .without_cancel(),
        content,
        window,
        cx,
    ));
}

fn content(window: &mut Window, cx: &mut App) -> AnyElement {
    let tokens = theme::semantic(cx);
    let max_height = (window.viewport_size().height - px(DIALOG_CHROME))
        .max(px(160.))
        .min(rems(30.).to_pixels(window.rem_size()));

    div()
        .id("clipboard-shortcuts")
        .flex()
        .flex_col()
        .max_h(max_height)
        .overflow_y_scroll()
        .children(SHORTCUTS.iter().enumerate().map(|(index, (label, keys))| {
            // 键名换成平台写法（Windows `Ctrl`、macOS `⌘`），行间细分隔线。
            let keys = keys.iter().map(|key| shortcut::display(key));
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(space(4.))
                .min_h(rems(2.25))
                .py(space(1.))
                .when(index > 0, |row| {
                    row.border_t_1().border_color(tokens.border.divider)
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_color(tokens.text.primary)
                        .kp_text(TextSize::Sm)
                        .child(t(label)),
                )
                .child(Shortcut::new(keys))
        }))
        .into_any_element()
}
