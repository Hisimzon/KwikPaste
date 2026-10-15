//! 关于页的「上报问题」：弹窗里写描述和可选的联系方式，附带内容可以逐项展开预览；
//! 发送成功后显示反馈编号，失败时可以把同一份内容保存为文件。

use std::{ops::Range, rc::Rc, time::Duration};

use gpui::{
    AppContext as _, AsyncWindowContext, Context, InteractiveElement as _, IntoElement,
    ListHorizontalSizingBehavior, ParentElement as _, Render, SharedString,
    StatefulInteractiveElement as _, Styled as _, UniformListScrollHandle, Window, div,
    prelude::FluentBuilder as _, radians, rems, uniform_list,
};
use kwikpaste_ui::{
    ConfirmSpec, DialogSpec, Icon, IconName, Input, KpStyled as _, TextArea, TextAreaInput,
    TextInput, confirm, form_dialog,
    theme::{self, TextSize, space},
    toast::{self, Toast},
};

use super::{Preferences, form_field, list_tile};
use crate::{
    clipboard, i18n,
    platform::feedback::{self, FeedbackDraft, FeedbackError},
    preferences::text,
};

/// 「正在发送」提示的 key：发送结束时原地换成结果。
const SEND_TOAST: &str = "feedback-send";
/// 上传最长 30 s，提示多留一些，结束时会被结果替换。
const SEND_TOAST_DURATION: Duration = Duration::from_secs(40);
const SAVE_NAME: &str = "KwikPaste-feedback.json.gz";

/// 失败原因对应的界面文案。
fn error_message(error: FeedbackError) -> SharedString {
    let key = match error {
        FeedbackError::DescriptionLength => "descriptionLength",
        FeedbackError::ContactLength => "contactLength",
        FeedbackError::TooLarge => "tooLarge",
        FeedbackError::NoEndpoint => "noEndpoint",
        FeedbackError::Timeout => "timeout",
        FeedbackError::Offline => "offline",
        FeedbackError::Rejected => "rejected",
        FeedbackError::RateLimited => "rateLimited",
        FeedbackError::Unavailable => "unavailable",
        FeedbackError::BadResponse => "badResponse",
        FeedbackError::Collect => "collect",
        FeedbackError::Save => "save",
    };
    i18n::t(&format!("preferences:feedback.errors.{key}"))
}

impl Preferences {
    /// 立即打开弹窗并在后台收集附带内容；点发送且校验通过后上传。
    pub(super) fn report_problem(&self, window: &mut Window, cx: &mut Context<Self>) {
        let dialog = cx.new(|cx| FeedbackDialog::new(window, cx));
        let content = dialog.clone();
        let check = dialog.downgrade();
        let answer = form_dialog(
            DialogSpec::new(i18n::t("preferences:feedback.title"))
                .ok_text(i18n::t("preferences:feedback.send"))
                .cancel_text(i18n::t("common:actions.cancel"))
                .validate(move |window, cx| {
                    let Some(dialog) = check.upgrade() else {
                        return false;
                    };
                    let Some(error) = dialog.read(cx).validation_error(cx) else {
                        return true;
                    };
                    toast::show(Toast::error(error), window, cx);
                    false
                }),
            move |_, _| content.clone().into_any_element(),
            window,
            cx,
        );
        let description = dialog.read(cx).description.clone();
        description.focus(window, cx);
        window
            .spawn(cx, async move |cx| {
                if !answer.await.unwrap_or(false) {
                    return;
                }
                let Ok(Some(form)) = cx.update(|_, cx| dialog.read(cx).form(cx)) else {
                    return;
                };
                send(form, cx).await;
            })
            .detach();
    }
}

/// 校验通过后要发送的内容。
#[derive(Clone)]
struct Form {
    draft: FeedbackDraft,
    description: String,
    contact: String,
}

/// 展开预览的附件：按行切好，最宽的一行用来撑开横向滚动宽度。
struct Expanded {
    index: usize,
    text: SharedString,
    lines: Rc<Vec<Range<usize>>>,
    widest: usize,
}

struct FeedbackDialog {
    description: TextAreaInput,
    contact: TextInput,
    /// `None` 表示还在收集。
    draft: Option<Result<FeedbackDraft, FeedbackError>>,
    expanded: Option<Expanded>,
    preview_scroll: UniformListScrollHandle,
}

impl FeedbackDialog {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prepare = feedback::prepare(cx);
        cx.spawn(async move |this, cx| {
            let draft = prepare.await;
            let _ = this.update(cx, |dialog, cx| {
                dialog.draft = Some(draft);
                cx.notify();
            });
        })
        .detach();

        Self {
            description: TextAreaInput::new(
                i18n::t("preferences:feedback.descriptionPlaceholder"),
                4,
                8,
                window,
                cx,
            ),
            contact: TextInput::new(
                i18n::t("preferences:feedback.contactPlaceholder"),
                window,
                cx,
            ),
            draft: None,
            expanded: None,
            preview_scroll: UniformListScrollHandle::new(),
        }
    }

    /// 点发送前的校验：返回要提示的错误，`None` 表示可以发送。
    fn validation_error(&self, cx: &gpui::App) -> Option<SharedString> {
        if let Err(error) =
            feedback::validate(&self.description.value(cx), self.contact.value(cx).trim())
        {
            return Some(error_message(error));
        }
        match &self.draft {
            None => Some(i18n::t("preferences:feedback.notReady")),
            Some(Err(error)) => Some(error_message(*error)),
            Some(Ok(_)) => None,
        }
    }

    fn form(&self, cx: &gpui::App) -> Option<Form> {
        let draft = self.draft.as_ref()?.as_ref().ok()?.clone();
        Some(Form {
            draft,
            description: self.description.value(cx).to_string(),
            contact: self.contact.value(cx).trim().to_owned(),
        })
    }

    fn toggle(&mut self, index: usize, cx: &mut Context<Self>) {
        if self
            .expanded
            .as_ref()
            .is_some_and(|expanded| expanded.index == index)
        {
            self.expanded = None;
        } else if let Some(Ok(draft)) = &self.draft
            && let Some(attachment) = draft.attachments.get(index)
        {
            let mut lines = Vec::new();
            let mut start = 0;
            for line in attachment.text.split('\n') {
                lines.push(start..start + line.trim_end_matches('\r').len());
                start += line.len() + 1;
            }
            let widest = lines
                .iter()
                .enumerate()
                .max_by_key(|(_, range)| range.len())
                .map_or(0, |(index, _)| index);
            self.expanded = Some(Expanded {
                index,
                text: attachment.text.clone().into(),
                lines: Rc::new(lines),
                widest,
            });
            self.preview_scroll = UniformListScrollHandle::new();
        }
        cx.notify();
    }

    fn render_attachments(&self, cx: &Context<Self>) -> gpui::AnyElement {
        let tokens = theme::semantic(cx);
        let note = |message: SharedString, color| {
            list_tile(tokens).child(
                div()
                    .px(space(3.))
                    .py(space(2.5))
                    .kp_text(TextSize::Sm)
                    .text_color(color)
                    .child(message),
            )
        };
        let draft = match &self.draft {
            None => {
                return note(
                    i18n::t("preferences:feedback.loading"),
                    tokens.text.secondary,
                )
                .into_any_element();
            }
            Some(Err(error)) => {
                return note(error_message(*error), tokens.status.danger.solid).into_any_element();
            }
            Some(Ok(draft)) => draft,
        };

        let mut tile = list_tile(tokens);
        for (index, attachment) in draft.attachments.iter().enumerate() {
            let expanded = self
                .expanded
                .as_ref()
                .filter(|expanded| expanded.index == index);
            tile = tile.child(
                div()
                    .id(("feedback-attachment", index))
                    .flex()
                    .items_center()
                    .gap(space(2.))
                    .px(space(3.))
                    .py(space(2.))
                    .when(index > 0, |row| {
                        row.border_t_1().border_color(tokens.border.divider)
                    })
                    .cursor_pointer()
                    .hover(|row| row.bg(tokens.fill.hover))
                    .on_click(cx.listener(move |dialog, _, _, cx| dialog.toggle(index, cx)))
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(rems(1.))
                            .color(tokens.text.muted)
                            .when(expanded.is_none(), |icon| {
                                icon.rotate(radians(-std::f32::consts::FRAC_PI_2))
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .kp_text(TextSize::Sm)
                            .truncate()
                            .child(attachment.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .kp_text(TextSize::Xs)
                            .text_color(tokens.text.secondary)
                            .child(text::format_bytes(attachment.text.len() as u64)),
                    ),
            );
            if let Some(expanded) = expanded {
                tile = tile.child(self.render_preview(expanded, cx));
            }
        }
        tile.into_any_element()
    }

    /// 附件全文按行虚拟滚动（日志可能有 1 MB），不折行，横向可滚。
    fn render_preview(&self, expanded: &Expanded, cx: &Context<Self>) -> gpui::AnyElement {
        let tokens = theme::semantic(cx);
        let text = expanded.text.clone();
        let lines = expanded.lines.clone();
        let list = uniform_list(
            "feedback-preview",
            lines.len(),
            move |range: Range<usize>, _, _| {
                range
                    .map(|index| {
                        let line = lines
                            .get(index)
                            .and_then(|range| text.get(range.clone()))
                            .filter(|line| !line.is_empty())
                            .unwrap_or(" ");
                        div()
                            .px(space(3.))
                            .whitespace_nowrap()
                            .child(SharedString::from(line.to_owned()))
                    })
                    .collect()
            },
        )
        .with_horizontal_sizing_behavior(ListHorizontalSizingBehavior::Unconstrained)
        .with_width_from_item(Some(expanded.widest))
        .track_scroll(&self.preview_scroll)
        .h(space(40.))
        .py(space(2.))
        .kp_mono()
        .kp_text(TextSize::Xs)
        .text_color(tokens.text.secondary);

        div()
            .border_t_1()
            .border_color(tokens.border.divider)
            .bg(tokens.fill.faint)
            .child(list)
            .into_any_element()
    }
}

impl Render for FeedbackDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::semantic(cx);
        let attachments = div()
            .flex()
            .flex_col()
            .gap(space(1.5))
            .child(self.render_attachments(cx))
            .child(
                div()
                    .kp_text(TextSize::Xs)
                    .text_color(tokens.text.secondary)
                    .child(i18n::t("preferences:feedback.noClipboard")),
            );

        div()
            .flex()
            .flex_col()
            .gap(space(4.))
            .child(form_field(
                i18n::t("preferences:feedback.description"),
                TextArea::new(&self.description).into_any_element(),
                tokens,
            ))
            .child(form_field(
                i18n::t("preferences:feedback.contact"),
                Input::new(&self.contact).into_any_element(),
                tokens,
            ))
            .child(form_field(
                i18n::t("preferences:feedback.attachments"),
                attachments.into_any_element(),
                tokens,
            ))
    }
}

/// 上传期间顶部提示「正在发送」；成功弹出反馈编号，失败说明原因并提供保存为文件。
async fn send(form: Form, cx: &mut AsyncWindowContext) {
    let Ok(task) = cx.update(|window, cx| {
        toast::show(
            Toast::info(i18n::t("preferences:feedback.sending"))
                .key(SEND_TOAST)
                .duration(SEND_TOAST_DURATION),
            window,
            cx,
        );
        feedback::submit(
            form.draft.clone(),
            form.description.clone(),
            form.contact.clone(),
            cx,
        )
    }) else {
        return;
    };
    let result = task.await;
    let _ = cx.update(|window, cx| match result {
        Ok(receipt) => {
            toast::show(
                Toast::success(i18n::t("preferences:feedback.sent")).key(SEND_TOAST),
                window,
                cx,
            );
            let content = i18n::t_args(
                "preferences:feedback.sentContent",
                &[("code", &receipt.code)],
            );
            drop(form_dialog(
                DialogSpec::new(i18n::t("preferences:feedback.sentTitle")).without_cancel(),
                move |_, _| {
                    div()
                        .kp_text(TextSize::Sm)
                        .child(content.clone())
                        .into_any_element()
                },
                window,
                cx,
            ));
        }
        Err(error) => offer_save(form, error, window, cx),
    });
}

/// 发送失败：说明原因，确认后选位置把同一份内容写成 `.json.gz`。
fn offer_save(form: Form, error: FeedbackError, window: &mut Window, cx: &mut gpui::App) {
    toast::show(
        Toast::error(i18n::t("preferences:feedback.sendFailed")).key(SEND_TOAST),
        window,
        cx,
    );
    let answer = confirm(
        ConfirmSpec::new(i18n::t("preferences:feedback.failedTitle"))
            .content(i18n::t_args(
                "preferences:feedback.failedContent",
                &[("reason", &error_message(error))],
            ))
            .ok_text(i18n::t("preferences:feedback.saveAsFile"))
            .cancel_text(i18n::t("preferences:feedback.close")),
        window,
        cx,
    );
    window
        .spawn(cx, async move |cx| {
            if !answer.await.unwrap_or(false) {
                return;
            }
            let Ok(prompt) = cx.update(|_, cx| {
                let directory = dirs::download_dir().unwrap_or_else(std::env::temp_dir);
                clipboard::view::pin::prompt_for_new_path(&directory, SAVE_NAME, cx)
            }) else {
                return;
            };
            let Some(path) = prompt.await else {
                return;
            };
            let Ok(task) = cx.update(|_, cx| {
                feedback::save(form.draft, form.description, form.contact, path, cx)
            }) else {
                return;
            };
            let message = match task.await {
                Ok(()) => Toast::success(i18n::t("preferences:feedback.saved")),
                Err(error) => Toast::error(error_message(error)),
            };
            let _ = cx.update(|window, cx| toast::show(message, window, cx));
        })
        .detach();
}
