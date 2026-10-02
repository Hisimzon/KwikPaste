//! 下拉选择器。弹层画在窗口内（gpui-component 的 popover），不会开新窗口、不抢焦点。

use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, Rems, RenderOnce, SharedString,
    Styled as _, Subscription, Window, prelude::FluentBuilder as _,
};
use gpui_component::{
    IndexPath, Sizable as _,
    searchable_list::SearchableListItem,
    select::{Select as KitSelect, SelectEvent, SelectState as KitSelectState},
};

use crate::strings::ui_strings;

/// 一个选项：存储用的值和显示用的文字。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectOption {
    pub value: SharedString,
    pub label: SharedString,
}

impl SelectOption {
    pub fn new(value: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            value: value.into(),
            label: label.into(),
        }
    }
}

impl SearchableListItem for SelectOption {
    type Value = SharedString;

    fn title(&self) -> SharedString {
        self.label.clone()
    }

    fn value(&self) -> &Self::Value {
        &self.value
    }
}

type KitState = KitSelectState<Vec<SelectOption>>;

/// 选择器状态，在视图里长期持有，渲染时交给 [`Select`]。
#[derive(Clone)]
pub struct SelectState {
    state: Entity<KitState>,
}

impl SelectState {
    pub fn new(
        options: Vec<SelectOption>,
        selected: Option<&str>,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let selected = position(&options, selected);
        let state = cx.new(|cx| KitState::new(options, selected, window, cx));

        Self { state }
    }

    pub fn selected_value(&self, cx: &App) -> Option<SharedString> {
        self.state.read(cx).selected_value().cloned()
    }

    /// 替换选项（例如切换语言后文字变了），保留同值的选中项。
    pub fn set_options(&self, options: Vec<SelectOption>, window: &mut Window, cx: &mut App) {
        let selected = self.selected_value(cx);
        let index = position(&options, selected.as_deref());
        self.state.update(cx, |state, cx| {
            state.set_items(options, window, cx);
            state.set_selected_index(index, window, cx);
        });
    }

    /// 用户选定后回调。返回的订阅要由视图保存。
    pub fn on_change<V: 'static>(
        &self,
        cx: &mut Context<V>,
        handler: impl Fn(&mut V, Option<SharedString>, &mut Context<V>) + 'static,
    ) -> Subscription {
        cx.subscribe(
            &self.state,
            move |view, _, event: &SelectEvent<Vec<SelectOption>>, cx| {
                let SelectEvent::Confirm(value) = event;
                handler(view, value.clone(), cx);
            },
        )
    }
}

fn position(options: &[SelectOption], value: Option<&str>) -> Option<IndexPath> {
    let value = value?;
    let row = options.iter().position(|option| option.value == value)?;

    Some(IndexPath::default().row(row))
}

#[derive(IntoElement)]
pub struct Select {
    state: SelectState,
    small: bool,
    width: Option<Rems>,
    placeholder: Option<SharedString>,
    disabled: bool,
    accessibility_label: Option<SharedString>,
}

impl Select {
    pub fn new(state: &SelectState) -> Self {
        Self {
            state: state.clone(),
            small: false,
            width: None,
            placeholder: None,
            disabled: false,
            accessibility_label: None,
        }
    }

    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    pub fn width(mut self, width: Rems) -> Self {
        self.width = Some(width);
        self
    }

    /// 占位文字；不给时用注入的默认文案（`UiStrings::select_placeholder`）。
    pub fn placeholder(mut self, placeholder: impl Into<SharedString>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// 读屏名称（偏好窗里用设置行标题）。
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }
}

impl RenderOnce for Select {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let placeholder = self
            .placeholder
            .unwrap_or_else(|| ui_strings(cx).select_placeholder);

        KitSelect::new(&self.state.state)
            .when(self.small, |select| select.small())
            .when_some(self.width, |select, width| select.w(width).max_w_full())
            .when(!placeholder.is_empty(), |select| {
                select.placeholder(placeholder)
            })
            .disabled(self.disabled)
            .when_some(self.accessibility_label, |select, label| {
                select.accessibility_label(label)
            })
    }
}
