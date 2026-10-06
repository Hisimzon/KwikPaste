//! 头部下方的分组栏（1.x `Group.tsx`）：范围（全部 / 收藏）、分类（文本 / 图片 / 文件）和自定义分组。
//!
//! - 范围始终有一个选中；分类、自定义分组再点一次取消；
//! - 隐藏的分组不出现在栏上；放不下的自定义分组收进“更多”菜单；
//! - 自定义分组右键可以编辑、隐藏、删除；新增、编辑、管理分组的弹框由主窗口打开（`group_dialogs`）。

use std::sync::Arc;

use gpui::{
    AnyElement, Context, EventEmitter, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Role, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    WindowControlArea, div, prelude::FluentBuilder as _,
};
use kwikpaste_ui::{
    IconName, MenuEntry, MenuItem, MenuTrigger, context_menu, group_icon_path, theme,
};

use super::{
    card::dp,
    chrome::{Glyph, Look, icon_button, separator},
};
use crate::{
    clipboard::{
        model::{
            filter::{CATEGORIES, ListFilter, Range},
            item::ItemKind,
        },
        source::Group,
    },
    i18n::t,
};

/// 1.x 分组按钮的尺寸与间距（设计 px），用来算一行放得下几个自定义分组。
const BUTTON: f32 = 24.;
const GAP: f32 = 4.;
/// 第二条分隔线右边缘到分组栏左边缘的距离（左内边距 + 范围 + 分类 + 两条分隔线，见 1.x
/// `computeCustomGroupCapacity`），再加分隔线外边距和一个间距。
const CUSTOM_START: f32 = 170. + 4. + GAP;
/// 末尾留给“更多”或“新增”按钮的位置。
const ACTION_SLOT: f32 = GAP + BUTTON;

/// 分组栏发给主窗口的事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GroupBarEvent {
    SelectRange(Range),
    ToggleCategory(ItemKind),
    ToggleGroup(Arc<str>),
    EditGroup(Group),
    HideGroup(Group),
    DeleteGroup(Group),
    NewGroup,
    ManageGroups,
}

pub struct GroupBar {
    groups: Vec<Group>,
    range: Range,
    category: Option<ItemKind>,
    group_id: Option<Arc<str>>,
    hints: bool,
}

impl EventEmitter<GroupBarEvent> for GroupBar {}

/// 一行能放下几个自定义分组（宽度为设计 px）。
pub fn capacity(width: f32) -> usize {
    let available = (width - CUSTOM_START - ACTION_SLOT).max(0.);
    ((available + GAP) / (BUTTON + GAP)).floor().max(0.) as usize
}

impl GroupBar {
    pub fn new() -> Self {
        Self {
            groups: Vec::new(),
            range: Range::All,
            category: None,
            group_id: None,
            hints: false,
        }
    }

    pub fn set_groups(&mut self, groups: Vec<Group>, cx: &mut Context<Self>) {
        self.groups = groups;
        cx.notify();
    }

    /// 分组栏上可见的自定义分组的 id（Tab 循环用）。
    pub fn visible_ids(&self) -> Vec<Arc<str>> {
        self.groups
            .iter()
            .filter(|group| !group.is_hidden)
            .map(|group| group.id.clone())
            .collect()
    }

    pub fn set_filter(&mut self, filter: &ListFilter, cx: &mut Context<Self>) {
        self.range = filter.range;
        self.category = filter.category;
        self.group_id = filter.group_id.clone();
        cx.notify();
    }

    pub fn set_hints(&mut self, hints: bool, cx: &mut Context<Self>) {
        if self.hints != hints {
            self.hints = hints;
            cx.notify();
        }
    }

    fn range_button(&self, range: Range, cx: &mut Context<Self>) -> AnyElement {
        let tokens = theme::tokens(cx);
        let (id, icon, label) = match range {
            Range::All => (
                "group-range-all",
                IconName::GroupAll,
                t("clipboard:groups.all"),
            ),
            Range::Favorite => (
                "group-range-favorite",
                IconName::GroupFavorite,
                t("clipboard:groups.favorite"),
            ),
        };
        let selected = self.range == range;
        // Mod+Q 的角标标在按下去会切到的那个按钮上。
        let hint = (self.hints && self.range.toggled() == range).then(|| SharedString::from("Q"));

        icon_button(tokens, id, Glyph::Icon(icon), label, hint, look(selected))
            .role(Role::Tab)
            .aria_selected(selected)
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(GroupBarEvent::SelectRange(range))))
            .into_any_element()
    }

    fn category_button(&self, kind: ItemKind, cx: &mut Context<Self>) -> AnyElement {
        let tokens = theme::tokens(cx);
        let (id, icon, label) = match kind {
            ItemKind::Text => (
                "group-text",
                IconName::GroupText,
                t("clipboard:groups.text"),
            ),
            ItemKind::Image => (
                "group-image",
                IconName::GroupImage,
                t("clipboard:groups.image"),
            ),
            ItemKind::Files => (
                "group-files",
                IconName::GroupFiles,
                t("clipboard:groups.files"),
            ),
        };

        icon_button(
            tokens,
            id,
            Glyph::Icon(icon),
            label,
            None,
            look(self.category == Some(kind)),
        )
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(GroupBarEvent::ToggleCategory(kind))))
        .role(Role::Tab)
        .aria_selected(self.category == Some(kind))
        .into_any_element()
    }

    fn custom_button(&self, group: &Group, cx: &mut Context<Self>) -> AnyElement {
        let tokens = theme::tokens(cx);
        let selected = self.group_id.as_deref() == Some(&*group.id);
        let id = group.id.clone();
        let entity = cx.entity().downgrade();
        let menu_group = group.clone();

        let button = icon_button(
            tokens,
            SharedString::from(format!("group-custom-{}", group.id)),
            Glyph::Path(group_icon_path(&group.icon)),
            SharedString::from(group.name.to_string()),
            None,
            look(selected),
        )
        .role(Role::Tab)
        .aria_selected(selected)
        .on_click(cx.listener(move |_, _, _, cx| {
            cx.emit(GroupBarEvent::ToggleGroup(id.clone()));
        }));

        context_menu(button, move |_, _| group_menu(&menu_group, entity.clone()))
    }

    fn more_button(&self, overflow: Vec<Group>, cx: &mut Context<Self>) -> AnyElement {
        let tokens = theme::tokens(cx);
        let selected = overflow
            .iter()
            .any(|group| self.group_id.as_deref() == Some(&*group.id));
        let current = self.group_id.clone();
        let entity = cx.entity().downgrade();
        let hint = self.hints.then(|| SharedString::from("N"));

        MenuTrigger::new("group-more")
            .child(icon_button(
                tokens,
                "group-more-button",
                Glyph::Icon(IconName::Ellipsis),
                t("clipboard:groups.more"),
                hint,
                look(selected),
            ))
            .dropdown(move |_, _| {
                let mut entries: Vec<MenuEntry> = overflow
                    .iter()
                    .map(|group| {
                        let id = group.id.clone();
                        let entity = entity.clone();
                        MenuItem::new(SharedString::from(group.name.to_string()), move |_, cx| {
                            entity
                                .update(cx, |_, cx| {
                                    cx.emit(GroupBarEvent::ToggleGroup(id.clone()));
                                })
                                .ok();
                        })
                        .icon_path(group_icon_path(&group.icon))
                        .checked(current.as_deref() == Some(&*group.id))
                        .into()
                    })
                    .collect();
                entries.push(MenuEntry::Separator);
                entries.extend(create_entries(entity.clone()));
                entries
            })
    }

    fn create_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let tokens = theme::tokens(cx);
        let entity = cx.entity().downgrade();
        let hint = self.hints.then(|| SharedString::from("N"));

        let button = icon_button(
            tokens,
            "group-create",
            Glyph::Icon(IconName::Plus),
            t("clipboard:groups.add"),
            hint,
            Look::Text,
        )
        .on_click(cx.listener(|_, _, _, cx| cx.emit(GroupBarEvent::NewGroup)));

        context_menu(button, move |_, _| {
            let entity = entity.clone();
            vec![
                MenuItem::new(t("clipboard:groups.manage"), move |_, cx| {
                    entity
                        .update(cx, |_, cx| cx.emit(GroupBarEvent::ManageGroups))
                        .ok();
                })
                .icon(IconName::Settings2)
                .into(),
            ]
        })
    }
}

fn look(selected: bool) -> Look {
    if selected { Look::Chip } else { Look::Text }
}

/// 自定义分组的右键菜单：编辑、隐藏、删除（1.x `buildGroupActionMenuItems`）。
fn group_menu(group: &Group, entity: gpui::WeakEntity<GroupBar>) -> Vec<MenuEntry> {
    let edit = group.clone();
    let hide = group.clone();
    let delete = group.clone();
    let edit_entity = entity.clone();
    let hide_entity = entity.clone();

    vec![
        MenuItem::new(t("clipboard:groups.edit"), move |_, cx| {
            edit_entity
                .update(cx, |_, cx| cx.emit(GroupBarEvent::EditGroup(edit.clone())))
                .ok();
        })
        .icon(IconName::Pencil)
        .into(),
        MenuItem::new(t("clipboard:groups.hide"), move |_, cx| {
            hide_entity
                .update(cx, |_, cx| cx.emit(GroupBarEvent::HideGroup(hide.clone())))
                .ok();
        })
        .icon(IconName::EyeOff)
        .into(),
        MenuEntry::Separator,
        MenuItem::new(t("clipboard:groups.delete"), move |_, cx| {
            entity
                .update(cx, |_, cx| {
                    cx.emit(GroupBarEvent::DeleteGroup(delete.clone()))
                })
                .ok();
        })
        .icon(IconName::Trash2)
        .danger()
        .into(),
    ]
}

fn create_entries(entity: gpui::WeakEntity<GroupBar>) -> Vec<MenuEntry> {
    let manage = entity.clone();

    vec![
        MenuItem::new(t("clipboard:groups.add"), move |_, cx| {
            entity
                .update(cx, |_, cx| cx.emit(GroupBarEvent::NewGroup))
                .ok();
        })
        .icon(IconName::Plus)
        .into(),
        MenuItem::new(t("clipboard:groups.manage"), move |_, cx| {
            manage
                .update(cx, |_, cx| cx.emit(GroupBarEvent::ManageGroups))
                .ok();
        })
        .icon(IconName::Settings2)
        .into(),
    ]
}

impl Render for GroupBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx);
        let width = window.viewport_size().width.as_f32() / window.rem_size().as_f32() * 16.;
        let visible: Vec<Group> = self
            .groups
            .iter()
            .filter(|group| !group.is_hidden)
            .cloned()
            .collect();
        let inline = visible.len().min(capacity(width));
        let overflow: Vec<Group> = visible.iter().skip(inline).cloned().collect();

        let ranges = [Range::All, Range::Favorite].map(|range| self.range_button(range, cx));
        let categories = CATEGORIES.map(|kind| self.category_button(kind, cx));
        let customs: Vec<AnyElement> = visible
            .iter()
            .take(inline)
            .map(|group| self.custom_button(group, cx))
            .collect();
        let action = if overflow.is_empty() {
            self.create_button(cx)
        } else {
            self.more_button(overflow, cx)
        };

        div()
            .id("clipboard-group-bar")
            .role(Role::TabList)
            .aria_label(t("clipboard:accessibility.groupBar"))
            .flex()
            .flex_none()
            .w_full()
            .items_center()
            .gap(dp(GAP))
            .overflow_hidden()
            .px(dp(12.))
            .pb(dp(8.))
            .children(ranges)
            .child(separator(tokens))
            .children(categories)
            .child(separator(tokens))
            .when(!customs.is_empty(), |bar| bar.children(customs))
            .child(action)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h(dp(32.))
                    .window_control_area(WindowControlArea::Drag),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 360 px 宽的面板放 5 个自定义分组（1.x 同宽时的实测）。
    #[test]
    fn the_default_panel_fits_five_custom_groups() {
        assert_eq!(capacity(360.), 5);
        assert_eq!(capacity(200.), 0);
        assert_eq!(capacity(500.), 10);
    }
}
