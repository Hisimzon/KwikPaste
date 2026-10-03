//! 主面板快捷键说明。

use gpui::{
    AnyElement, App, Context, IntoElement as _, ParentElement as _, Styled as _, Window, div, px,
};
use kwikpaste_ui::theme::TextSize;
use kwikpaste_ui::{DialogSpec, KpStyled as _, Shortcut, form_dialog, theme};

use super::panel::ClipboardPanel;
use crate::i18n::t;

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
        &["CmdOrCtrl", "Backspace"],
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

/// 在当前面板窗口打开快捷键列表。
pub fn show(window: &mut Window, cx: &mut Context<ClipboardPanel>) {
    drop(form_dialog(
        DialogSpec::new(t("clipboard:shortcuts.title")).ok_text(t("common:ui.ok")),
        |_, cx| content(cx),
        window,
        cx,
    ));
}

fn content(cx: &mut App) -> AnyElement {
    let tokens = theme::tokens(cx);
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .max_h(px(480.))
        .children(SHORTCUTS.iter().map(|(label, keys)| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.))
                .py(px(3.))
                .child(
                    div()
                        .flex_1()
                        .text_color(tokens.text)
                        .kp_text(TextSize::Sm)
                        .child(t(label)),
                )
                .child(Shortcut::new(keys.iter().copied()))
        }))
        .into_any_element()
}
