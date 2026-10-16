//! 图片文字识别的引擎选择（仅 Windows）：系统识别，或下载一档 PP-OCRv6。
//!
//! 每档是一个独立扩展，同时只装一档；装上即使用，切回系统识别就是卸载它。

use gpui::{AnyElement, Context, IntoElement, ParentElement as _, SharedString, Styled as _, div};
use kwikpaste_ui::{
    Button, KpStyled as _, Tag, TagColor,
    theme::{self, TextSize, space},
};
use kwikpaste_updater::extensions::{ExtensionStatus, OperationState};

use super::extensions::{Action, status_text};
use super::{Preferences, row_frame, row_label, text};
use crate::i18n;

/// 从轻到重；顺序就是界面上的顺序。
pub(super) const TIERS: [&str; 3] = ["ocr-tiny", "ocr-small", "ocr-medium"];

impl Preferences {
    /// 标题行加四个选项行：系统识别在最前，下面是三档 PP-OCRv6。
    pub(super) fn render_ocr_engines(
        &self,
        statuses: &[ExtensionStatus],
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let tokens = theme::semantic(cx);
        let tiers: Vec<&ExtensionStatus> = TIERS
            .iter()
            .filter_map(|id| statuses.iter().find(|status| status.id == *id))
            .collect();
        let active = tiers
            .iter()
            .find(|status| status.installed.is_some())
            .map(|status| status.id.clone());
        let busy = tiers.iter().any(|status| {
            matches!(
                status.operation,
                OperationState::Downloading { .. } | OperationState::Installing
            )
        });
        let entity = cx.entity().downgrade();
        let button = |key: &str, label: SharedString, id: String, action: Action| {
            let entity = entity.clone();
            Button::new(SharedString::from(format!("ocr-engine-{key}")), label)
                .small()
                .disabled(busy)
                .on_click(move |_, window, cx| {
                    if let Some(entity) = entity.upgrade() {
                        entity.update(cx, |this, cx| {
                            this.run_extension_action(id.clone(), action, window, cx);
                        });
                    }
                })
                .into_any_element()
        };
        let in_use = || {
            Tag::new(i18n::t("preferences:ocrEngine.inUse"))
                .color(TagColor::Success)
                .into_any_element()
        };

        let mut rows = vec![
            row_frame(false, tokens)
                .child(row_label(
                    i18n::t("preferences:ocrEngine.title"),
                    i18n::t("preferences:ocrEngine.description"),
                    tokens,
                ))
                .into_any_element(),
        ];
        let system = match &active {
            None => in_use(),
            Some(id) => button(
                "system",
                i18n::t("preferences:ocrEngine.use"),
                id.clone(),
                Action::Remove,
            ),
        };
        rows.push(option_row("system", None, system, cx));
        for status in tiers {
            let name = status.id.trim_start_matches("ocr-");
            let control = if status.installed.is_some() {
                in_use()
            } else if let Some(progress) = status_text(status).filter(|_| busy_card(status)) {
                muted(progress, cx)
            } else if matches!(status.operation, OperationState::Error { .. }) {
                button(
                    name,
                    i18n::t("preferences:extensions.retry"),
                    status.id.clone(),
                    Action::Refresh,
                )
            } else if let Some(catalog) = &status.catalog {
                button(
                    name,
                    i18n::t_args(
                        "preferences:ocrEngine.download",
                        &[("size", &text::format_bytes(catalog.size))],
                    ),
                    status.id.clone(),
                    Action::Install,
                )
            } else {
                muted(i18n::t("preferences:ocrEngine.unavailable"), cx)
            };
            let error = match &status.operation {
                OperationState::Error { message } => Some(SharedString::from(message.clone())),
                _ => None,
            };
            rows.push(option_row(name, error, control, cx));
        }
        rows
    }
}

fn busy_card(status: &ExtensionStatus) -> bool {
    matches!(
        status.operation,
        OperationState::Downloading { .. } | OperationState::Installing
    )
}

/// 一个引擎选项：名称、实测说明，出错时说明换成错误原因。
fn option_row(
    key: &str,
    error: Option<SharedString>,
    control: AnyElement,
    cx: &mut Context<Preferences>,
) -> AnyElement {
    let tokens = theme::semantic(cx);
    let description =
        error.unwrap_or_else(|| i18n::t(&format!("preferences:ocrEngine.{key}.description")));
    row_frame(false, tokens)
        .child(row_label(
            i18n::t(&format!("preferences:ocrEngine.{key}.name")),
            description,
            tokens,
        ))
        .child(div().flex().flex_none().items_center().child(control))
        .into_any_element()
}

fn muted(label: SharedString, cx: &mut Context<Preferences>) -> AnyElement {
    let tokens = theme::semantic(cx);
    div()
        .kp_text(TextSize::Xs)
        .text_color(tokens.text.muted)
        .pr(space(1.))
        .child(label)
        .into_any_element()
}
