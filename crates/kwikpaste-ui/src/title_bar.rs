//! Windows 上自绘的窗口标题栏。
//!
//! 磨砂（accent 亚克力）只铺客户区，系统标题栏会留下一条不透明的色带。所以带标题栏的窗口在 Windows 上
//! 用 [`titlebar_options`] 隐藏系统标题栏，再由 [`TitleBar`] 画标题和最小化 / 最大化 / 关闭按钮。
//! 按钮只声明 `window_control_area`：点击、悬停时的贴靠布局、双击最大化都交给系统的非客户区处理。
//! macOS 保留系统标题栏，[`TitleBar`] 什么也不画。

use gpui::{
    App, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, TitlebarOptions, Window,
    WindowControlArea, div, prelude::FluentBuilder as _, px, rems,
};

use crate::theme::{self, TextSize, space};
use crate::{Icon, IconName, KpStyled as _};

/// 应用是否自己画标题栏（Windows）；为 false 时窗口用系统标题栏。
pub const DRAWS_TITLE_BAR: bool = cfg!(target_os = "windows");

/// 自绘标题栏占用的客户区高度（同 Windows 11 系统标题栏，不随文字缩放）；系统标题栏时为 0。
pub const TITLE_BAR_HEIGHT: Pixels = px(if DRAWS_TITLE_BAR { 32. } else { 0. });

/// 带标题的窗口的 `TitlebarOptions`：Windows 上隐藏系统标题栏，`title` 仍是任务栏和 Alt+Tab 的名字。
pub fn titlebar_options(title: SharedString) -> TitlebarOptions {
    TitlebarOptions {
        title: Some(title),
        appears_transparent: DRAWS_TITLE_BAR,
        ..Default::default()
    }
}

/// 自绘标题栏：左侧是可拖动的标题区，右侧是窗口按钮。高度同 Windows 11 系统标题栏。
#[derive(IntoElement, Default)]
pub struct TitleBar {
    title: Option<SharedString>,
}

impl TitleBar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }
}

impl RenderOnce for TitleBar {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        if !DRAWS_TITLE_BAR {
            return div();
        }

        let tokens = theme::components(cx).caption_button;
        let active = window.is_window_active();
        let foreground = if active {
            tokens.foreground
        } else {
            tokens.inactive_foreground
        };
        let maximize = if window.is_maximized() {
            IconName::WindowRestore
        } else {
            IconName::WindowMaximize
        };

        div()
            .flex()
            .flex_none()
            .w_full()
            .h(TITLE_BAR_HEIGHT)
            .child(
                div()
                    .id("title-bar-drag")
                    .window_control_area(WindowControlArea::Drag)
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .px(space(3.))
                    .kp_text(TextSize::Xs)
                    .text_color(foreground)
                    .when_some(self.title, |bar, title| {
                        bar.child(div().truncate().child(title))
                    }),
            )
            .child(caption_button(
                "title-bar-minimize",
                IconName::WindowMinimize,
                WindowControlArea::Min,
                foreground,
                cx,
            ))
            .child(caption_button(
                "title-bar-maximize",
                maximize,
                WindowControlArea::Max,
                foreground,
                cx,
            ))
            .child(caption_button(
                "title-bar-close",
                IconName::WindowClose,
                WindowControlArea::Close,
                foreground,
                cx,
            ))
    }
}

fn caption_button(
    id: &'static str,
    icon: IconName,
    area: WindowControlArea,
    foreground: gpui::Hsla,
    cx: &App,
) -> impl IntoElement {
    let tokens = theme::components(cx).caption_button;
    let close = area == WindowControlArea::Close;
    let (hover, pressed, hover_foreground) = if close {
        (
            tokens.close_hover,
            tokens.close_pressed,
            tokens.close_foreground,
        )
    } else {
        (tokens.hover, tokens.pressed, foreground)
    };

    div()
        .id(id)
        .window_control_area(area)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(46.))
        .h_full()
        .text_color(foreground)
        .hover(|style| style.bg(hover).text_color(hover_foreground))
        .active(|style| style.bg(pressed).text_color(hover_foreground))
        .child(Icon::new(icon).size(rems(0.9375)))
}
