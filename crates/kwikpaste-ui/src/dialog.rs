//! antd `Modal` 式的表单对话框：标题、任意内容、右下角的取消与确定。
//!
//! 外壳与 [`crate::confirm()`] 相同，用 gpui-component 的 Dialog（遮罩、Esc、焦点陷阱）。
//! 内容里的多行输入框自己处理 Enter（换行），确定只能点按钮。

use std::{cell::RefCell, rc::Rc};

use futures::channel::oneshot;
use gpui::{AnyElement, App, ParentElement as _, SharedString, Styled as _, Window, div, px, rems};
use gpui_base::{h_flex, v_flex};
use gpui_component::WindowExt as _;

use crate::{
    button::Button,
    strings::ui_strings,
    styled::KpStyled as _,
    theme::{self, TextSize, radius, space},
};

/// 对话框的标题与按钮文字。
#[derive(Clone, Debug)]
pub struct DialogSpec {
    title: SharedString,
    ok_text: Option<SharedString>,
    cancel_text: Option<SharedString>,
}

impl DialogSpec {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            ok_text: None,
            cancel_text: None,
        }
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
}

/// 只回答一次：先到的那个（按钮、Esc）生效。
#[derive(Clone)]
struct Answer(Rc<RefCell<Option<oneshot::Sender<bool>>>>);

impl Answer {
    fn send(&self, confirmed: bool) {
        if let Some(sender) = self.0.borrow_mut().take() {
            let _ = sender.send(confirmed);
        }
    }
}

/// 打开表单对话框。点确定得到 `true`；取消、Esc 得到 `false`；窗口关闭时接收端收到取消错误。
/// `content` 每帧调用一次，返回对话框的主体（输入框等）。
pub fn form_dialog(
    spec: DialogSpec,
    content: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    window: &mut Window,
    cx: &mut App,
) -> oneshot::Receiver<bool> {
    let (sender, receiver) = oneshot::channel();
    let answer = Answer(Rc::new(RefCell::new(Some(sender))));
    let content = Rc::new(content);

    window.open_dialog(cx, move |dialog, window, cx| {
        let tokens = theme::tokens(cx);
        let strings = ui_strings(cx);
        let ok_text = spec.ok_text.clone().unwrap_or(strings.ok);
        let cancel_text = spec.cancel_text.clone().unwrap_or(strings.cancel);
        let ok_answer = answer.clone();
        let cancel_answer = answer.clone();
        let on_cancel = answer.clone();
        let title = spec.title.clone();
        let content = content.clone();
        // antd Modal 默认宽 520 px；面板只有 360 px 宽，左右各留 16 px。
        let width = rems(32.5)
            .to_pixels(window.rem_size())
            .min(window.viewport_size().width - px(32.));

        dialog
            .width(width)
            .close_button(false)
            .overlay_closable(false)
            .bg(tokens.bg_elevated)
            .border_0()
            .rounded(radius::LG)
            .px(rems(1.5))
            .py(rems(1.25))
            .content(move |body, window, cx| {
                let ok_answer = ok_answer.clone();
                let cancel_answer = cancel_answer.clone();
                body.child(
                    v_flex()
                        .gap(space(3.))
                        .child(
                            div()
                                .kp_text(TextSize::Base)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme::tokens(cx).text)
                                .child(title.clone()),
                        )
                        .child(content(window, cx))
                        .child(
                            h_flex()
                                .justify_end()
                                .gap(space(2.))
                                .child(
                                    Button::new("kp-dialog-cancel", cancel_text.clone()).on_click(
                                        move |_, window, cx| {
                                            cancel_answer.send(false);
                                            window.close_dialog(cx);
                                        },
                                    ),
                                )
                                .child(
                                    Button::new("kp-dialog-ok", ok_text.clone())
                                        .primary()
                                        .on_click(move |_, window, cx| {
                                            ok_answer.send(true);
                                            window.close_dialog(cx);
                                        }),
                                ),
                        ),
                )
            })
            .on_cancel(move |_, _, _| {
                on_cancel.send(false);
                true
            })
    });

    receiver
}

/// 关掉窗口里最上层的对话框（例如面板隐藏时收起备注框）。
pub fn close_dialog(window: &mut Window, cx: &mut App) {
    window.close_dialog(cx);
}

/// 窗口里有没有打开的对话框。
pub fn has_dialog(window: &mut Window, cx: &mut App) -> bool {
    window.has_active_dialog(cx)
}
