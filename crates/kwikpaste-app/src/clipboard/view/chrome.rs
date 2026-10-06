//! 头部、分组栏、页脚共用的小按钮：24 px 图标按钮，按住修饰键时图标换成快捷键角标（1.x
//! `CustomIconButton` + `KeyHint`）。

use gpui::{
    AnyElement, Div, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Rems,
    Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, div,
    prelude::FluentBuilder as _, svg,
};
use kwikpaste_ui::{
    Icon, IconName, KeyHint, TooltipExt as _,
    theme::{KpTokens, TextSize},
};

use super::card::dp;

/// 按钮里的图标：内置图标或资源路径（自定义分组图标）。
#[derive(Clone)]
pub enum Glyph {
    Icon(IconName),
    Path(SharedString),
}

impl Glyph {
    pub fn render(&self, size: Rems, color: gpui::Hsla) -> AnyElement {
        match self {
            Self::Icon(name) => Icon::new(*name).size(size).color(color).into_any_element(),
            Self::Path(path) => svg()
                .path(path.clone())
                .flex_none()
                .size(size)
                .text_color(color)
                .into_any_element(),
        }
    }
}

/// 按钮的外观。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// 无底，悬停出填充色、图标加深。
    Text,
    /// 选中：淡主色底、主色图标（分组栏选中项、固定窗口按钮）。
    Selected,
}

/// 图标按钮：`hint` 是按住修饰键时显示的角标（`None` 时不显示）。调用方挂点击。
pub fn icon_button(
    tokens: &KpTokens,
    id: impl Into<ElementId>,
    glyph: Glyph,
    label: SharedString,
    hint: Option<SharedString>,
    look: Look,
) -> Stateful<Div> {
    let (color, background, hover) = match look {
        Look::Text => (tokens.secondary, None, tokens.fill_secondary),
        Look::Selected => (
            tokens.primary,
            Some(tokens.primary.opacity(0.13)),
            tokens.primary.opacity(0.2),
        ),
    };
    let hinted = hint.is_some();
    let keyshortcuts = hint.clone();

    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .when_some(keyshortcuts, |button, key| button.aria_keyshortcuts(key))
        .relative()
        .flex()
        .flex_none()
        .size(dp(24.))
        .items_center()
        .justify_center()
        .rounded(dp(7.))
        .cursor_pointer()
        .when_some(background, |button, background| button.bg(background))
        .hover(move |style| style.bg(hover))
        .child(
            div()
                .flex()
                .when(hinted, |icon| icon.opacity(0.))
                .child(glyph.render(TextSize::Base.font_size(), color)),
        )
        .when_some(hint, |button, key| {
            button.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(KeyHint::new(key)),
            )
        })
        .kp_tooltip(label)
}

/// 分组栏里分隔范围、分类、自定义分组三段的竖线。
pub fn separator(tokens: &KpTokens) -> Div {
    div()
        .flex_none()
        .mx(dp(4.))
        .h(dp(16.))
        .w(gpui::px(1.))
        .bg(tokens.split)
}
