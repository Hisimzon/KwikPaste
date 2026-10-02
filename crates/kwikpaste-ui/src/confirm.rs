//! antd `modal.confirm` 式的确认框：图标 + 标题 + 内容，按钮在右下，危险操作的确定按钮为红色。
//!
//! 外壳用 gpui-component 的 Dialog（遮罩、Esc、Enter、焦点陷阱），内容和按钮自己画。

use std::{cell::RefCell, rc::Rc};

use futures::channel::oneshot;
use gpui::{
    App, IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window, rems, svg,
};
use gpui_base::{h_flex, v_flex};
use gpui_component::WindowExt as _;

use crate::{
    button::Button,
    strings::ui_strings,
    styled::KpStyled as _,
    theme::{self, TextSize, radius, space},
    toast::EXCLAMATION_CIRCLE_FILLED,
};

/// 确认框的内容。
#[derive(Clone, Debug)]
pub struct ConfirmSpec {
    title: SharedString,
    content: Option<SharedString>,
    ok_text: Option<SharedString>,
    cancel_text: Option<SharedString>,
    danger: bool,
}

impl ConfirmSpec {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            content: None,
            ok_text: None,
            cancel_text: None,
            danger: false,
        }
    }

    pub fn content(mut self, content: impl Into<SharedString>) -> Self {
        self.content = Some(content.into());
        self
    }

    /// 确定按钮文字；不给时用注入的默认文案（`UiStrings::ok`）。
    pub fn ok_text(mut self, text: impl Into<SharedString>) -> Self {
        self.ok_text = Some(text.into());
        self
    }

    /// 取消按钮文字；不给时用注入的默认文案（`UiStrings::cancel`）。
    pub fn cancel_text(mut self, text: impl Into<SharedString>) -> Self {
        self.cancel_text = Some(text.into());
        self
    }

    /// 危险操作：确定按钮为红色（antd `okButtonProps: { danger: true }`）。
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }
}

/// 只回答一次：先到的那个（按钮、Esc、Enter）生效。
#[derive(Clone)]
struct Answer(Rc<RefCell<Option<oneshot::Sender<bool>>>>);

impl Answer {
    fn send(&self, confirmed: bool) {
        if let Some(sender) = self.0.borrow_mut().take() {
            let _ = sender.send(confirmed);
        }
    }
}

/// 打开确认框。确定得到 `true`；取消、Esc 得到 `false`；窗口关闭时接收端收到取消错误。
///
/// ```ignore
/// let answer = kwikpaste_ui::confirm(spec, window, cx);
/// cx.spawn(async move |_, _| if answer.await.unwrap_or(false) { /* 执行 */ }).detach();
/// ```
pub fn confirm(spec: ConfirmSpec, window: &mut Window, cx: &mut App) -> oneshot::Receiver<bool> {
    let (sender, receiver) = oneshot::channel();
    let answer = Answer(Rc::new(RefCell::new(Some(sender))));

    window.open_dialog(cx, move |dialog, window, cx| {
        let tokens = theme::tokens(cx);
        let strings = ui_strings(cx);
        let body = ConfirmBody {
            spec: spec.clone(),
            ok_text: spec.ok_text.clone().unwrap_or(strings.ok),
            cancel_text: spec.cancel_text.clone().unwrap_or(strings.cancel),
            answer: Some(answer.clone()),
        };
        let on_ok = answer.clone();
        let on_cancel = answer.clone();

        // antd confirm：宽 416 px，内边距 20 px 24 px，圆角 borderRadiusLG，底色 colorBgElevated，无描边。
        // 阴影由 gpui-component 的进场动画每帧写入，覆盖不了，接受它的样子。
        dialog
            .width(rems(26.).to_pixels(window.rem_size()))
            .close_button(false)
            .overlay_closable(false)
            .bg(tokens.bg_elevated)
            .border_0()
            .rounded(radius::LG)
            .px(rems(1.5))
            .py(rems(1.25))
            .content(move |content, _, _| content.child(body.clone()))
            .on_ok(move |_, _, _| {
                on_ok.send(true);
                true
            })
            .on_cancel(move |_, _, _| {
                on_cancel.send(false);
                true
            })
    });

    receiver
}

/// 确认框的内容与按钮。组件展示时 `answer` 为空，按钮不做事。
#[derive(Clone, IntoElement)]
pub struct ConfirmBody {
    spec: ConfirmSpec,
    ok_text: SharedString,
    cancel_text: SharedString,
    answer: Option<Answer>,
}

impl ConfirmBody {
    /// 静态展示（组件展示窗用）。
    pub fn preview(spec: ConfirmSpec, cx: &App) -> Self {
        let strings = ui_strings(cx);

        Self {
            ok_text: spec.ok_text.clone().unwrap_or(strings.ok),
            cancel_text: spec.cancel_text.clone().unwrap_or(strings.cancel),
            spec,
            answer: None,
        }
    }
}

impl RenderOnce for ConfirmBody {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = theme::tokens(cx);
        let ok_answer = self.answer.clone();
        let cancel_answer = self.answer;
        let ok = Button::new("kp-confirm-ok", self.ok_text)
            .kind(if self.spec.danger {
                crate::ButtonKind::Danger
            } else {
                crate::ButtonKind::Primary
            })
            .on_click(move |_, window, cx| {
                if let Some(answer) = &ok_answer {
                    answer.send(true);
                    window.close_dialog(cx);
                }
            });
        let cancel =
            Button::new("kp-confirm-cancel", self.cancel_text).on_click(move |_, window, cx| {
                if let Some(answer) = &cancel_answer {
                    answer.send(false);
                    window.close_dialog(cx);
                }
            });

        // antd：图标 22 px、与文字间隔 12 px；标题 16 / 24 px 加粗；内容 14 / 22 px，距标题 8 px；
        // 按钮区距内容 12 px，按钮间隔 8 px。
        v_flex()
            .gap(space(3.))
            .child(
                h_flex()
                    .items_start()
                    .gap(space(3.))
                    .child(
                        svg()
                            .flex_none()
                            .size(rems(1.375))
                            .text_color(tokens.warning)
                            .data(EXCLAMATION_CIRCLE_FILLED),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(space(2.))
                            .child(
                                gpui::div()
                                    .kp_text(TextSize::Base)
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(tokens.text)
                                    .child(self.spec.title),
                            )
                            .children(self.spec.content.map(|content| {
                                gpui::div()
                                    .kp_text(TextSize::Sm)
                                    .line_height(rems(1.375))
                                    .text_color(tokens.text)
                                    .child(content)
                            })),
                    ),
            )
            .child(
                h_flex()
                    .justify_end()
                    .gap(space(2.))
                    .child(cancel)
                    .child(ok),
            )
    }
}
