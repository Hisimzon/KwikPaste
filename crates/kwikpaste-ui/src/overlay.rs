//! 每个窗口一份的浮层宿主：挂在 gpui-base `Root` 上的插件，负责 toast 与 antd 风格的 Tooltip，
//! 并给根元素设置正文默认样式（字号、行高、颜色、字体回退）。
//!
//! 注册晚于 gpui-component 自己的插件，所以画在对话框、下拉等浮层之上。

use std::time::Duration;

use gpui::{
    AnyElement, App, AppContext as _, Context, Div, Entity, IntoElement, ParentElement as _,
    Render, Stateful, Styled as _, Window, deferred, div,
};
use gpui_base::{Root, RootPlugin, TooltipOverlay};

use crate::{
    theme::{self, TextSize, fonts},
    toast::{Toast, ToastStack},
    tooltip,
};

/// toast 层的绘制优先级：高于 gpui-base 的 Tooltip（200）。
const TOAST_PRIORITY: usize = 300;

pub(crate) struct Overlays {
    pub(crate) tooltip: Entity<TooltipOverlay>,
    toasts: ToastStack,
}

impl Overlays {
    fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            tooltip: cx.new(|_| TooltipOverlay::new().render_with(tooltip::render_tooltip)),
            toasts: ToastStack::default(),
        }
    }

    pub(crate) fn push_toast(&mut self, toast: Toast, window: &mut Window, cx: &mut Context<Self>) {
        let reduce_motion = cx.reduce_motion();
        let (generation, duration) = self.toasts.push(toast);

        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(duration).await;
            let leaving = this.update(cx, |this, cx| {
                let leaving = this.toasts.begin_leave(generation);
                cx.notify();
                leaving
            });
            if !matches!(leaving, Ok(true)) {
                return;
            }

            let exit = theme::motion::duration(theme::motion::MID, reduce_motion);
            if exit > Duration::ZERO {
                cx.background_executor().timer(exit).await;
            }
            let _ = this.update(cx, |this, cx| {
                this.toasts.remove(generation);
                cx.notify();
            });
        })
        .detach();
    }
}

impl RootPlugin for Overlays {
    /// 1.x 的 `body { text-sm text-ant-text }`，外加显式的中文、Emoji 字体回退。
    fn style(&self, surface: &mut Stateful<Div>, _: &mut Window, cx: &mut App) {
        let text = surface.text_style();
        text.color = Some(theme::semantic(cx).text.primary);
        text.font_size = Some(TextSize::Sm.font_size().into());
        text.line_height = Some(TextSize::Sm.line_height().into());
        text.font_fallbacks = Some(fonts::ui_fallbacks());
    }
}

impl Render for Overlays {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let toasts: Option<AnyElement> = (!self.toasts.is_empty()).then(|| {
            deferred(self.toasts.render(window, cx))
                .with_priority(TOAST_PRIORITY)
                .into_any_element()
        });

        div()
            .absolute()
            .inset_0()
            .child(self.tooltip.clone())
            .children(toasts)
    }
}

/// 注册浮层插件。必须在 `gpui_component::init` 之后、打开任何窗口之前调用。
pub(crate) fn init(cx: &mut App) {
    Root::register_plugin::<Overlays>(cx, Overlays::new);
}

/// 当前窗口的浮层宿主；窗口不是经 [`crate::open_window`] 打开的就没有。
pub(crate) fn overlays(window: &Window, cx: &App) -> Option<Entity<Overlays>> {
    window
        .root::<Root>()
        .flatten()?
        .read(cx)
        .plugin::<Overlays>()
}
