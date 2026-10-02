//! 标签（antd Tag）、按键徽标（1.x 快捷键列表的 Kbd）与修饰键提示角标（1.x KeyHint）。

use gpui::{
    App, Hsla, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window, div,
    rems,
};
use gpui_base::h_flex;
use gpui_component::tag::Tag as KitTag;

use crate::{
    styled::KpStyled as _,
    theme::{self, TextSize, antd, radius, space},
};

/// 标签颜色，对应 antd Tag 的默认与状态色。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TagColor {
    #[default]
    Default,
    Primary,
    Success,
    Warning,
    Error,
}

impl TagColor {
    pub const ALL: [Self; 5] = [
        Self::Default,
        Self::Primary,
        Self::Success,
        Self::Warning,
        Self::Error,
    ];
}

/// antd Tag：12 px 字、行高 20 px、左右 7 px、圆角 borderRadiusSM。
#[derive(IntoElement)]
pub struct Tag {
    label: SharedString,
    color: TagColor,
}

impl Tag {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            color: TagColor::Default,
        }
    }

    pub fn color(mut self, color: TagColor) -> Self {
        self.color = color;
        self
    }
}

/// `(底色, 字色, 描边)`。
fn tag_colors(color: TagColor, colors: &antd::AntdColors) -> (Hsla, Hsla, Hsla) {
    let (bg, fg, border) = match color {
        TagColor::Default => (
            colors.color_fill_quaternary,
            colors.color_text,
            colors.color_border,
        ),
        TagColor::Primary => (
            colors.color_primary_bg,
            colors.color_primary,
            colors.color_primary_border,
        ),
        TagColor::Success => (
            colors.color_success_bg,
            colors.color_success,
            colors.color_success_border,
        ),
        TagColor::Warning => (
            colors.color_warning_bg,
            colors.color_warning,
            colors.color_warning_border,
        ),
        TagColor::Error => (
            colors.color_error_bg,
            colors.color_error,
            colors.color_error_border,
        ),
    };

    (bg.to_hsla(), fg.to_hsla(), border.to_hsla())
}

impl RenderOnce for Tag {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = match theme::appearance(cx) {
            theme::Appearance::Light => &antd::LIGHT,
            theme::Appearance::Dark => &antd::DARK,
        };
        let (bg, fg, border) = tag_colors(self.color, colors);

        KitTag::custom(bg, fg, border)
            .rounded(radius::SM)
            .px(rems(0.4375))
            .py_0()
            .kp_text(TextSize::Xs)
            .line_height(rems(1.25))
            .child(self.label)
    }
}

/// 一个按键徽标（1.x 快捷键列表：24 px 高、最小 24 px 宽、`rounded-1.5`、`fill-secondary` 底、等宽 12 px）。
#[derive(IntoElement)]
pub struct Kbd {
    label: SharedString,
}

impl Kbd {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

impl RenderOnce for Kbd {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = theme::tokens(cx);

        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .h(space(6.))
            .min_w(space(6.))
            .px(space(1.5))
            .rounded(radius::MD)
            .bg(tokens.fill_secondary)
            .text_color(tokens.secondary)
            .kp_mono()
            .kp_text(TextSize::Xs)
            .child(self.label)
    }
}

/// 一组按键徽标，按按下顺序排列（如 `["Ctrl", "F"]`）。
#[derive(IntoElement)]
pub struct Shortcut {
    keys: Vec<SharedString>,
}

impl Shortcut {
    pub fn new<K: Into<SharedString>>(keys: impl IntoIterator<Item = K>) -> Self {
        Self {
            keys: keys.into_iter().map(Into::into).collect(),
        }
    }
}

impl RenderOnce for Shortcut {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        h_flex()
            .gap(space(1.))
            .items_center()
            .children(self.keys.into_iter().map(Kbd::new))
    }
}

/// 按住修饰键时显示的角标（1.x KeyHint：16 px、`rounded-1`、`bg-spotlight` 底、12 px 粗体等宽大写、
/// `light-solid` 字）。
#[derive(IntoElement)]
pub struct KeyHint {
    key: SharedString,
}

impl KeyHint {
    pub fn new(key: impl Into<SharedString>) -> Self {
        Self { key: key.into() }
    }
}

impl RenderOnce for KeyHint {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = theme::tokens(cx);

        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(space(4.))
            .rounded(radius::SM)
            .bg(tokens.bg_spotlight)
            .text_color(tokens.light_solid)
            .kp_mono()
            .kp_text(TextSize::Xs)
            .font_weight(gpui::FontWeight::BOLD)
            .child(self.key.to_uppercase())
    }
}
