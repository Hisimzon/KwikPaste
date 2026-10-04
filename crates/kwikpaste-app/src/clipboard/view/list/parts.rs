//! 列表里带回调的小部件：卡片的悬停快捷动作与复选框、页脚、多选栏、空态。

use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, ElementId, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, div,
    prelude::FluentBuilder as _, radians,
};
use kwikpaste_ui::{
    Button, Checkbox, Icon, IconName, KpStyled as _, TooltipExt as _,
    theme::{self, KpTokens, TextSize},
};

use super::ClipboardList;
use crate::{
    clipboard::{
        model::{
            actions::{QuickAction, is_copy},
            empty_state::{category_key, empty_text},
            item::ListItem,
            shortcut,
        },
        view::{
            card::dp,
            chrome::{Glyph, Look, icon_button},
        },
    },
    i18n::{t, t_args, t_count},
};

/// 一个快捷动作按钮的图标、文案和颜色（1.x `resolveItemActionPresentation`）。
fn presentation(
    tokens: &KpTokens,
    action: QuickAction,
    item: &ListItem,
    copied: bool,
) -> (IconName, SharedString, gpui::Hsla) {
    let secondary = tokens.secondary;
    match action {
        QuickAction::Paste => (
            IconName::ClipboardPaste,
            t("clipboard:quickActions.paste"),
            secondary,
        ),
        QuickAction::PastePlain => (
            IconName::ClipboardType,
            t("clipboard:quickActions.pastePlain"),
            secondary,
        ),
        QuickAction::PastePath => (
            IconName::FileSymlink,
            t("clipboard:quickActions.pastePath"),
            secondary,
        ),
        QuickAction::Copy | QuickAction::CopyPlain if copied => (
            IconName::CircleCheck,
            t("clipboard:quickActions.copySuccess"),
            tokens.success,
        ),
        QuickAction::Copy => (IconName::Copy, t("clipboard:quickActions.copy"), secondary),
        QuickAction::CopyPlain => (
            IconName::CopyCheck,
            t("clipboard:quickActions.copyPlain"),
            secondary,
        ),
        QuickAction::SplitWords => (
            IconName::TextSelect,
            t("clipboard:quickActions.splitWords"),
            secondary,
        ),
        QuickAction::OpenLink => (
            IconName::SquareArrowOutUpRight,
            t("clipboard:quickActions.openLink"),
            secondary,
        ),
        QuickAction::SendEmail => (
            IconName::Mail,
            t("clipboard:quickActions.sendEmail"),
            secondary,
        ),
        QuickAction::Reveal => (
            IconName::FolderOpen,
            t("clipboard:quickActions.reveal"),
            secondary,
        ),
        QuickAction::Note => (
            IconName::NotebookPen,
            t("clipboard:quickActions.note"),
            if item.note.is_some() {
                tokens.primary
            } else {
                secondary
            },
        ),
        QuickAction::Star if item.is_favorite => (
            IconName::Star,
            t("clipboard:quickActions.starActive"),
            tokens.warning,
        ),
        QuickAction::Star => (IconName::Star, t("clipboard:quickActions.star"), secondary),
        QuickAction::PinItem if item.is_pinned => (
            IconName::PushPin,
            t("clipboard:quickActions.pinItemActive"),
            tokens.primary,
        ),
        QuickAction::PinItem => (
            IconName::PushPin,
            t("clipboard:quickActions.pinItem"),
            secondary,
        ),
        QuickAction::Delete => (
            IconName::Trash,
            t("clipboard:quickActions.delete"),
            tokens.error,
        ),
    }
}

fn action_name(action: QuickAction) -> &'static str {
    match action {
        QuickAction::Paste => "paste",
        QuickAction::PastePlain => "paste-plain",
        QuickAction::PastePath => "paste-path",
        QuickAction::Copy => "copy",
        QuickAction::CopyPlain => "copy-plain",
        QuickAction::SplitWords => "split-words",
        QuickAction::OpenLink => "open-link",
        QuickAction::SendEmail => "send-email",
        QuickAction::Reveal => "reveal",
        QuickAction::Note => "note",
        QuickAction::Star => "star",
        QuickAction::PinItem => "pin-item",
        QuickAction::Delete => "delete",
    }
}

impl ClipboardList {
    /// 卡片悬停时显示的一排快捷动作（20 px 图标按钮）。按下与点击都不再冒泡给卡片，
    /// 免得触发选中、单击粘贴或双击粘贴。
    pub(super) fn quick_actions(
        &self,
        item: &Arc<ListItem>,
        actions: Vec<QuickAction>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let tokens = theme::tokens(cx);
        let buttons = actions.into_iter().map(|action| {
            let copied = is_copy(action)
                && self
                    .copied
                    .as_ref()
                    .is_some_and(|(id, copied)| *id == item.id && *copied == action);
            let (icon, label, color) = presentation(tokens, action, item, copied);
            let hover_color = if color == tokens.secondary {
                tokens.text
            } else {
                color
            };
            let item = item.clone();

            div()
                .id(ElementId::Name(action_name(action).into()))
                .flex()
                .flex_none()
                .size(dp(20.))
                .items_center()
                .justify_center()
                .rounded(dp(6.))
                .cursor_pointer()
                .text_color(color)
                .hover(move |style| style.bg(tokens.fill_tertiary).text_color(hover_color))
                .child(
                    Icon::new(icon)
                        .size(TextSize::Sm.font_size())
                        .when(action == QuickAction::PinItem, |icon| {
                            icon.rotate(radians(-std::f32::consts::FRAC_PI_4))
                        }),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |list, _, window, cx| {
                    cx.stop_propagation();
                    list.run_quick_action(item.clone(), action, window, cx);
                }))
                .kp_tooltip(label)
        });

        div()
            .flex()
            .items_center()
            .gap(dp(2.))
            .children(buttons)
            .into_any_element()
    }

    /// 多选时卡片上的复选框；受删除保护的记录不可选（1.x `disabled={!canDelete}`）。
    pub(super) fn checkbox(&self, item: &ListItem, can_delete: bool) -> AnyElement {
        Checkbox::new(ElementId::Name(format!("check-{}", item.id).into()))
            .checked(self.selection.is_checked(&item.id))
            .disabled(!can_delete)
            .accessibility_label(t("clipboard:footer.select"))
            .into_any_element()
    }

    /// 空列表：按筛选条件给出 16 种提示之一。
    pub(super) fn render_empty(&self, cx: &App) -> AnyElement {
        let tokens = theme::tokens(cx);
        let filter = self.filter();
        let text = empty_text(filter);
        let category = text.category.map(|kind| t(category_key(kind)));
        let group = filter.group_id.as_ref().map(|id| {
            self.groups
                .iter()
                .find(|group| group.id == *id)
                .map(|group| SharedString::from(group.name.to_string()))
                .unwrap_or_else(|| t("clipboard:empty.groupFallback"))
        });
        let mut args: Vec<(&str, &str)> = Vec::new();
        if text.searching {
            args.push(("keyword", &filter.keyword));
        }
        if let Some(category) = &category {
            args.push(("category", category));
        }
        if let Some(group) = &group {
            args.push(("group", group));
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(dp(8.))
            .px(dp(24.))
            .child(
                Icon::new(IconName::Inbox)
                    .size(dp(40.))
                    .color(tokens.quaternary),
            )
            .child(
                div()
                    .kp_text(TextSize::Sm)
                    .text_color(tokens.description)
                    .text_center()
                    .child(t_args(text.key, &args)),
            )
            .into_any_element()
    }

    /// 页脚：左侧当前视图的总条数，右侧多选入口与快捷键列表（1.x `Footer.tsx`）；多选时换成多选栏。
    pub(super) fn render_footer(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.selection.active() {
            return self.render_selection_bar(cx);
        }

        let tokens = theme::tokens(cx);
        let total = i64::try_from(self.model.total()).unwrap_or(i64::MAX);
        let hint = |key: &str| self.key_hints.then(|| SharedString::from(key.to_owned()));
        let empty = total == 0;

        div()
            .flex()
            .flex_none()
            .h(dp(32.))
            .items_center()
            .justify_between()
            .px(dp(12.))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .items_center()
                    .window_control_area(gpui::WindowControlArea::Drag)
                    .kp_text(TextSize::Xs)
                    .text_color(tokens.tertiary)
                    .child(t_count("clipboard:footer.total", total, &[])),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(dp(4.))
                    .child(
                        icon_button(
                            tokens,
                            "footer-select",
                            Glyph::Icon(IconName::ListChecks),
                            t("clipboard:footer.select"),
                            hint("A"),
                            Look::Text,
                        )
                        .when(empty, |button| button.opacity(0.4).cursor_default())
                        .on_click(cx.listener(move |list, _, _, cx| {
                            if !empty {
                                list.enter_selection(cx);
                            }
                        })),
                    )
                    .child(
                        icon_button(
                            tokens,
                            "footer-shortcuts",
                            Glyph::Icon(IconName::Keyboard),
                            t("clipboard:footer.shortcuts"),
                            hint("K"),
                            Look::Text,
                        )
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(super::ListIntent::ShowShortcuts);
                        })),
                    ),
            )
            .into_any_element()
    }

    /// 多选栏（1.x `SelectionBar.tsx`）：左侧已选条数或操作提示，右侧全选、删除、取消。
    fn render_selection_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let tokens = theme::tokens(cx);
        let count = self.selection.count();
        let busy = self.selection.busy();
        let status = if count > 0 {
            t_count(
                "clipboard:selection.selected",
                i64::try_from(count).unwrap_or(i64::MAX),
                &[],
            )
        } else {
            t("clipboard:selection.hint")
        };
        let toggle_label = if self.selection.all_checked() {
            t("clipboard:selection.clear")
        } else {
            t("clipboard:selection.selectAll")
        };

        div()
            .flex()
            .flex_none()
            .h(dp(32.))
            .items_center()
            .justify_between()
            .gap(dp(8.))
            .px(dp(12.))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .kp_text(TextSize::Xs)
                    .text_color(tokens.secondary)
                    .child(status),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(dp(4.))
                    .child(
                        Button::new("selection-toggle-all", toggle_label)
                            .small()
                            .ghost()
                            .disabled(busy)
                            .tooltip(shortcut::display("CmdOrCtrl+A"))
                            .on_click(cx.listener(|list, _, window, cx| {
                                list.toggle_all(window, cx);
                            })),
                    )
                    .child(
                        Button::new("selection-delete", t("clipboard:selection.delete"))
                            .small()
                            .danger_outline()
                            .disabled(busy || count == 0)
                            .tooltip(shortcut::display("CmdOrCtrl+Backspace"))
                            .on_click(cx.listener(|list, _, window, cx| {
                                list.delete_checked(window, cx);
                            })),
                    )
                    .child(
                        Button::new("selection-exit", t("clipboard:selection.exit"))
                            .small()
                            .ghost()
                            .tooltip(shortcut::display("Escape"))
                            .on_click(cx.listener(|list, _, _, cx| list.exit_selection(cx))),
                    ),
            )
            .into_any_element()
    }
}
