//! 单行输入框与搜索框。
//!
//! 右键菜单一律关闭：gpui-component 的输入框菜单在 Windows 上是 Win32 原生菜单
//! （`SetForegroundWindow` + `TrackPopupMenuEx`），会抢前台、破坏不激活面板。

use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, Rems, RenderOnce, SharedString,
    Styled as _, Subscription, Window, prelude::FluentBuilder as _,
};
use gpui_component::{
    Sizable as _,
    input::{Input as KitInput, InputEvent, InputState},
};

use crate::{
    icon::{Icon, IconName},
    theme,
};

/// 输入框的内容状态，在视图里长期持有，渲染时交给 [`Input`]。
#[derive(Clone)]
pub struct TextInput {
    state: Entity<InputState>,
}

impl TextInput {
    pub fn new(placeholder: impl Into<SharedString>, window: &mut Window, cx: &mut App) -> Self {
        let placeholder = placeholder.into();
        let state = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(placeholder)
                .context_menu(false)
        });

        Self { state }
    }

    pub fn value(&self, cx: &App) -> SharedString {
        self.state.read(cx).value()
    }

    /// 改写内容，不触发变化回调。
    pub fn set_value(&self, value: impl Into<SharedString>, window: &mut Window, cx: &mut App) {
        let value: SharedString = value.into();
        self.state.update(cx, |state, cx| {
            state.set_value(value.to_string(), window, cx)
        });
    }

    /// 切换语言时更新占位文字。
    pub fn set_placeholder(
        &self,
        placeholder: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let placeholder = placeholder.into();
        self.state.update(cx, |state, cx| {
            state.set_placeholder(placeholder, window, cx)
        });
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| state.focus(window, cx));
    }

    /// 内容变化时回调（输入、粘贴、清空）。返回的订阅要由视图保存。
    pub fn on_change<V: 'static>(
        &self,
        cx: &mut Context<V>,
        handler: impl Fn(&mut V, SharedString, &mut Context<V>) + 'static,
    ) -> Subscription {
        cx.subscribe(&self.state, move |view, state, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                let value = state.read(cx).value();
                handler(view, value, cx);
            }
        })
    }
}

/// 输入框尺寸。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputSize {
    /// 24 px：antd `size="small"`，主窗口搜索框。
    Small,
    /// 32 px：antd 默认。
    #[default]
    Medium,
}

#[derive(IntoElement)]
pub struct Input {
    input: TextInput,
    size: InputSize,
    width: Option<Rems>,
    cleanable: bool,
    disabled: bool,
    search: bool,
    accessibility_label: Option<SharedString>,
}

impl Input {
    pub fn new(input: &TextInput) -> Self {
        Self {
            input: input.clone(),
            size: InputSize::Medium,
            width: None,
            cleanable: false,
            disabled: false,
            search: false,
            accessibility_label: None,
        }
    }

    /// 搜索框：前缀放搜索图标、可清空。主窗口头部用 `.small()` 加 `.width(rems(10.))`（160 px）。
    pub fn search(input: &TextInput) -> Self {
        Self {
            search: true,
            cleanable: true,
            ..Self::new(input)
        }
    }

    pub fn small(mut self) -> Self {
        self.size = InputSize::Small;
        self
    }

    pub fn width(mut self, width: Rems) -> Self {
        self.width = Some(width);
        self
    }

    pub fn cleanable(mut self, cleanable: bool) -> Self {
        self.cleanable = cleanable;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// 读屏名称；不给时用占位文字。
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }
}

impl RenderOnce for Input {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = theme::tokens(cx);
        let icon_size = match self.size {
            InputSize::Small => theme::TextSize::Sm.font_size(),
            InputSize::Medium => theme::TextSize::Base.font_size(),
        };

        KitInput::new(&self.input.state)
            .when(self.size == InputSize::Small, |input| input.small())
            .when_some(self.width, |input, width| input.w(width).max_w_full())
            .cleanable(self.cleanable)
            .disabled(self.disabled)
            .when(self.search, |input| {
                input.prefix(
                    Icon::new(IconName::Search)
                        .size(icon_size)
                        .color(tokens.quaternary),
                )
            })
            .when_some(self.accessibility_label, |input, label| {
                input.aria_label(label)
            })
    }
}
