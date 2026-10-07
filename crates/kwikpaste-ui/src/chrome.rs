//! 剪贴板面板共用的图标按钮、快捷键角标与分隔线。

use gpui::{
    AnyElement, App, Div, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    Rems, Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _, div,
    prelude::FluentBuilder as _, svg,
};

use crate::{Icon, IconName, KeyHint, KpStyled as _, TooltipExt as _, theme};

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

/// 面板图标按钮的外观。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// 无底，悬停出填充色、图标加深。
    Text,
    /// 选中：淡主色底、主色图标（固定窗口按钮）。
    Selected,
    /// 分组栏选中项：中性填充底的胶囊。
    Chip,
}

/// 图标按钮：`hint` 是按住修饰键时显示的角标（`None` 时不显示）。调用方挂点击。
pub fn icon_button(
    cx: &App,
    id: impl Into<ElementId>,
    glyph: Glyph,
    label: SharedString,
    hint: Option<SharedString>,
    look: Look,
) -> Stateful<Div> {
    let semantic = theme::semantic(cx);
    let button = &theme::components(cx).icon_button;
    let (color, background, hover) = match look {
        Look::Text => (semantic.text.secondary, None, button.hover),
        Look::Selected => (
            semantic.accent.solid,
            Some(button.selected),
            button.selected_hover,
        ),
        Look::Chip => (
            button.chip_foreground,
            Some(button.chip_background),
            button.chip_hover,
        ),
    };
    let chip = look == Look::Chip;
    let chip_label = chip.then(|| label.clone());
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
        .h(theme::space(6.))
        .when(!chip, |button| button.w(theme::space(6.)))
        .when(chip, |button| {
            button.px(theme::space(2.)).gap(theme::space(1.))
        })
        .items_center()
        .justify_center()
        .rounded(theme::space(1.75))
        .cursor_pointer()
        .when_some(background, |button, background| button.bg(background))
        .hover(move |style| style.bg(hover))
        .child(
            div()
                .flex()
                .when(hinted, |icon| icon.opacity(0.))
                .child(glyph.render(theme::TextSize::Base.font_size(), color)),
        )
        .when_some(chip_label, |button, label| {
            button.child(
                div()
                    .kp_text(theme::TextSize::Xs)
                    .text_color(color)
                    .max_w(theme::space(18.))
                    .truncate()
                    .when(hinted, |text| text.opacity(0.))
                    .child(label),
            )
        })
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
pub fn separator(cx: &App) -> Div {
    div()
        .flex_none()
        .mx(theme::space(1.))
        .h(theme::space(4.))
        .w(gpui::px(1.))
        .bg(theme::semantic(cx).border.divider)
}
