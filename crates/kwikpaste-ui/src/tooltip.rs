//! antd 风格的 Tooltip：`colorBgSpotlight` 深底白字，最宽 250 px，允许多行，默认显示在上方。
//!
//! gpui-component 自带的 Tooltip 用 popover 底色和描边，而且挂载接口是 crate 私有的，所以这里
//! 直接用 gpui-base 的 `TooltipOverlay`（定位、延迟、切换宽限期都由它负责），每个窗口一份，
//! 放在 [`crate::overlay`] 插件里。

use std::{cell::Cell, rc::Rc};

use gpui::{
    Animation, AnimationExt as _, AnyElement, AnyView, App, AppContext as _, Bounds, Context,
    ElementId, IntoElement, MouseButton, ParentElement, Pixels, Render, RenderOnce, SharedString,
    StatefulInteractiveElement, Styled as _, Window, div, rems,
};
use gpui_base::{ElementExt as _, Placement, TooltipRequest, TooltipTransition};

use crate::{
    overlay,
    styled::KpStyled as _,
    theme::{self, TextSize, control_height, motion, radius, space},
};

/// Tooltip 的气泡本体（浮层与组件展示共用）。
#[derive(IntoElement)]
pub struct TooltipBubble {
    text: SharedString,
}

impl TooltipBubble {
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self { text: text.into() }
    }
}

impl RenderOnce for TooltipBubble {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = theme::tokens(cx);

        // antd：内边距 6 px 8 px，最小高 controlHeight，最宽 250 px，圆角 borderRadius，阴影 boxShadow。
        // 不用 flex：flex 子项不会收缩到内容宽度以下，长文字就不换行了。
        div()
            .max_w(rems(15.625))
            .min_h(control_height::MD)
            .px(space(2.))
            .py(rems(0.375))
            .rounded(radius::MD)
            .bg(tokens.bg_spotlight)
            .text_color(tokens.light_solid)
            .shadow(tokens.shadow_elevated.to_vec())
            .kp_text(TextSize::Sm)
            .line_height(rems(1.375))
            .child(self.text)
    }
}

/// 浮层里的 Tooltip 视图。
struct TooltipView {
    text: SharedString,
}

impl Render for TooltipView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        // 外边距就是与触发元素的间隙（antd 由 8 px 的箭头撑开，这里不画箭头）。
        div()
            .m(space(2.))
            .child(TooltipBubble::new(self.text.clone()))
    }
}

/// gpui-base 浮层的呈现：首次出现淡入（antd 的 zoom-big-fast 为 100 ms），在相邻控件间切换不再淡入。
pub(crate) fn render_tooltip(
    view: AnyView,
    transition: TooltipTransition,
    _: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let content = div().child(view);
    let TooltipTransition::Enter { epoch } = transition else {
        return content.into_any_element();
    };
    if cx.reduce_motion() {
        return content.into_any_element();
    }

    content
        .with_animation(
            ElementId::NamedInteger("kp-tooltip".into(), epoch as u64),
            Animation::new(motion::FAST),
            |content, delta| content.opacity(delta),
        )
        .into_any_element()
}

/// 给可交互元素挂 Tooltip。图标按钮必须挂：它的文字同时是读屏名称（见 [`crate::Button::icon`]）。
pub trait TooltipExt: StatefulInteractiveElement + ParentElement + Sized {
    fn kp_tooltip(self, text: impl Into<SharedString>) -> Self {
        let text = text.into();
        let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::default();
        let writer = bounds.clone();

        self.on_prepaint(move |element_bounds, _, _| writer.set(element_bounds))
            .on_hover(move |hovered, window, cx| {
                let Some(overlays) = overlay::overlays(window, cx) else {
                    return;
                };
                let tooltip = overlays.read(cx).tooltip.clone();
                if !*hovered {
                    tooltip.update(cx, |tooltip, cx| tooltip.request_hide(window, cx));
                    return;
                }

                let text = text.clone();
                let request = TooltipRequest::new(bounds.get(), move |_, cx| {
                    let text = text.clone();
                    cx.new(|_| TooltipView { text }).into()
                })
                .placement(Placement::Top);
                tooltip.update(cx, |tooltip, cx| tooltip.request_show(request, window, cx));
            })
            .on_mouse_down(MouseButton::Left, |_, window, cx| {
                let Some(overlays) = overlay::overlays(window, cx) else {
                    return;
                };
                let tooltip = overlays.read(cx).tooltip.clone();
                tooltip.update(cx, |tooltip, cx| tooltip.hide(cx));
            })
    }
}

impl<E: StatefulInteractiveElement + ParentElement + Sized> TooltipExt for E {}
