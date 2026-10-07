//! 标签（antd Tag）、按键徽标（1.x 快捷键列表的 Kbd）与修饰键提示角标（1.x KeyHint）。

use gpui::{
    App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window, div, rems,
};
use gpui_base::h_flex;
use gpui_component::tag::Tag as KitTag;

use crate::{
    styled::KpStyled as _,
    theme::{self, TextSize, radius, space},
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

impl RenderOnce for Tag {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = &theme::components(cx).tag;
        let index = match self.color {
            TagColor::Default => 0,
            TagColor::Primary => 1,
            TagColor::Success => 2,
            TagColor::Warning => 3,
            TagColor::Error => 4,
        };
        let state = tokens.variants.get(index).copied().unwrap_or_default();

        KitTag::custom(state.background, state.foreground, state.border)
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
        let tokens = &theme::components(cx).tag;

        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .h(space(6.))
            .min_w(space(6.))
            .px(space(1.5))
            .rounded(radius::MD)
            .bg(tokens.background)
            .text_color(tokens.foreground)
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
    /// 每个键一个键帽；单独的 `/` 表示“或”，画成键帽之间的灰色分隔字。
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = &theme::components(cx).tag;

        h_flex()
            .gap(space(1.))
            .items_center()
            .children(self.keys.into_iter().map(|key| {
                if key.as_ref() == "/" {
                    return div()
                        .px(space(0.5))
                        .kp_text(TextSize::Xs)
                        .text_color(tokens.separator)
                        .child(key)
                        .into_any_element();
                }

                Kbd::new(key).into_any_element()
            }))
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
        let tokens = &theme::components(cx).tag;

        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(space(4.))
            .rounded(radius::SM)
            .bg(tokens.key_background)
            .text_color(tokens.key_foreground)
            .kp_mono()
            .kp_text(TextSize::Xs)
            .font_weight(gpui::FontWeight::BOLD)
            .child(self.key.to_uppercase())
    }
}
