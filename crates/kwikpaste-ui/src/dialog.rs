//! antd `Modal` 式的表单对话框：标题、任意内容、右下角的取消与确定。
//!
//! 外壳与 [`crate::confirm()`] 相同，用 gpui-component 的 Dialog（遮罩、Esc、焦点陷阱）。
//! 内容里的多行输入框自己处理 Enter（换行），确定只能点按钮。

use std::{cell::RefCell, rc::Rc};

use futures::channel::oneshot;
use gpui::{
    AnyElement, App, ParentElement as _, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, rems,
};
use gpui_base::{h_flex, v_flex};
use gpui_component::WindowExt as _;

use crate::{
    button::Button,
    strings::ui_strings,
    styled::KpStyled as _,
    theme::{self, TextSize, radius, space},
};

type DialogCheck = Rc<dyn Fn(&mut Window, &mut App) -> bool>;
type FooterExtra = Rc<dyn Fn(&mut Window, &mut App) -> AnyElement>;

/// 对话框的标题、按钮文字，以及可选的确定前校验和页脚额外按钮。
#[derive(Clone)]
pub struct DialogSpec {
    title: SharedString,
    ok_text: Option<SharedString>,
    cancel_text: Option<SharedString>,
    validate: Option<DialogCheck>,
    footer_extra: Option<FooterExtra>,
    show_cancel: bool,
    danger: bool,
}

impl DialogSpec {
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            ok_text: None,
            cancel_text: None,
            validate: None,
            footer_extra: None,
            show_cancel: true,
            danger: false,
        }
    }

    /// 只读的说明弹框（如快捷键列表）只要一个确定按钮；Esc 仍然关闭。
    pub fn without_cancel(mut self) -> Self {
        self.show_cancel = false;
        self
    }

    /// 点确定时先校验（antd `form.validateFields`）：返回假时对话框留着，由表单自己显示错误。
    pub fn validate(mut self, check: impl Fn(&mut Window, &mut App) -> bool + 'static) -> Self {
        self.validate = Some(Rc::new(check));
        self
    }

    /// 页脚左侧的额外按钮（1.x `SortableTreeModal` 的 `footerExtra`），与右侧的取消、确定分开。
    pub fn footer_extra(
        mut self,
        extra: impl Fn(&mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.footer_extra = Some(Rc::new(extra));
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
        let tokens = theme::semantic(cx);
        let strings = ui_strings(cx);
        let ok_text = spec.ok_text.clone().unwrap_or(strings.ok);
        let cancel_text = spec.cancel_text.clone().unwrap_or(strings.cancel);
        let ok_answer = answer.clone();
        let cancel_answer = answer.clone();
        let on_cancel = answer.clone();
        let title = spec.title.clone();
        let content = content.clone();
        let validate = spec.validate.clone();
        let footer_extra = spec.footer_extra.clone();
        let show_cancel = spec.show_cancel;
        let danger = spec.danger;
        // antd Modal 默认宽 520 px；面板只有 360 px 宽，左右各留 16 px。
        let width = rems(32.5)
            .to_pixels(window.rem_size())
            .min(window.viewport_size().width - px(32.));

        dialog
            .width(width)
            .close_button(false)
            .overlay_closable(false)
            .bg(tokens.surface.raised)
            .border_0()
            .rounded(radius::LG)
            .px(rems(1.5))
            .py(rems(1.25))
            .content(move |body, window, cx| {
                let ok_answer = ok_answer.clone();
                let cancel_answer = cancel_answer.clone();
                let validate = validate.clone();
                let extra = footer_extra.as_ref().map(|extra| extra(window, cx));
                body.child(
                    v_flex()
                        .gap(space(3.))
                        .child(
                            div()
                                .kp_text(TextSize::Base)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(theme::semantic(cx).text.primary)
                                .child(title.clone()),
                        )
                        .child(content(window, cx))
                        // 额外按钮（如“新增”）靠左，与取消、确定分开；没有时取消、确定照常靠右。
                        .child(
                            h_flex()
                                .gap(space(2.))
                                .child(div().flex().flex_1().children(extra))
                                .when(show_cancel, |footer| {
                                    footer.child(
                                        Button::new("kp-dialog-cancel", cancel_text.clone())
                                            .on_click(move |_, window, cx| {
                                                cancel_answer.send(false);
                                                window.close_dialog(cx);
                                            }),
                                    )
                                })
                                .child(
                                    Button::new("kp-dialog-ok", ok_text.clone())
                                        .map(|button| {
                                            if danger {
                                                button.danger()
                                            } else {
                                                button.primary()
                                            }
                                        })
                                        .on_click(move |_, window, cx| {
                                            if let Some(validate) = &validate
                                                && !validate(window, cx)
                                            {
                                                return;
                                            }
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
