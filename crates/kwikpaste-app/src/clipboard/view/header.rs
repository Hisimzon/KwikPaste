//! 主窗口头部（1.x `Header.tsx` + `SearchInput.tsx`）：logo、搜索框、固定窗口与偏好设置。
//!
//! 搜索框走平台层的编辑态：在输入框上按下鼠标请求 `BeginEditing(Mouse)`，`EditingStarted` 之后
//! 才真正能打字（Windows 上面板此时才是前台窗口）。输入 200 ms 防抖、去掉首尾空白后交给列表；
//! 输入法组字期间不触发（[`kwikpaste_ui::TextInput::on_event_in`]）。

use std::{sync::Arc, time::Duration};

use gpui::{
    Context, EventEmitter, ImageSource, InteractiveElement as _, IntoElement, MouseButton,
    MouseDownEvent, ParentElement as _, Render, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Task, Window, WindowControlArea, div, img,
};
use kwikpaste_ui::{IconName, Input, TextInput, TextInputEvent, theme};

use super::{
    SEARCH_CONTEXT,
    card::{dp, logo},
    chrome::{Glyph, Look, icon_button},
};
use crate::{
    i18n::{self, t},
    platform::EditTrigger,
};

/// 1.x `useDebounceFn(..., { wait: 200 })`。
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(200);

/// 头部发给主窗口的事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HeaderEvent {
    /// 防抖后的搜索词（已去掉首尾空白）。
    Keyword(Arc<str>),
    /// 在输入框上按下了鼠标，要进入编辑态。
    RequestEditing(EditTrigger),
    TogglePin,
    OpenPreferences,
}

pub struct Header {
    input: TextInput,
    /// 按住修饰键：图标换成快捷键角标。
    hints: bool,
    /// 窗口已固定（1.x 固定按钮的本地状态）。
    pinned: bool,
    /// 搜索框处于编辑态。
    editing: bool,
    language: i18n::Language,
    debounce: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<HeaderEvent> for Header {}

impl Header {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = TextInput::new(t("clipboard:header.searchPlaceholder"), window, cx);
        let changes = input.on_event_in(window, cx, |header, event, _, cx| {
            if let TextInputEvent::Change(value) = event {
                header.debounce_keyword(value, cx);
            }
        });

        Self {
            input,
            hints: false,
            pinned: false,
            editing: false,
            language: i18n::language(),
            debounce: None,
            _subscriptions: vec![changes],
        }
    }

    pub fn input(&self) -> &TextInput {
        &self.input
    }

    pub fn set_hints(&mut self, hints: bool, cx: &mut Context<Self>) {
        if self.hints != hints {
            self.hints = hints;
            cx.notify();
        }
    }

    pub fn set_pinned(&mut self, pinned: bool, cx: &mut Context<Self>) {
        self.pinned = pinned;
        cx.notify();
    }

    pub fn editing(&self) -> bool {
        self.editing
    }

    pub fn set_editing(&mut self, editing: bool, cx: &mut Context<Self>) {
        self.editing = editing;
        cx.notify();
    }

    /// 清空搜索框，取消还没发出的防抖。
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.debounce = None;
        self.input.set_value("", window, cx);
        cx.notify();
    }

    /// 编辑态开始后聚焦输入框并全选（1.x `focus({ cursor: "all" })`）。
    pub fn focus_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.focus(window, cx);
        self.input.select_all(window, cx);
    }

    /// 自测用：当作用户输入了 `value`（与真实输入一样经过防抖）。
    pub fn type_text(&mut self, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.input.set_value(value.to_owned(), window, cx);
        self.debounce_keyword(SharedString::from(value.to_owned()), cx);
    }

    fn debounce_keyword(&mut self, value: SharedString, cx: &mut Context<Self>) {
        let keyword: Arc<str> = Arc::from(value.trim());
        // 只记长度：搜索词是用户内容，不进日志。
        log::debug!("search input changed ({} chars)", keyword.chars().count());
        self.debounce = Some(cx.spawn(async move |header, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            header
                .update(cx, |_, cx| cx.emit(HeaderEvent::Keyword(keyword)))
                .ok();
        }));
    }
}

impl Render for Header {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx);
        if self.language != i18n::language() {
            self.language = i18n::language();
            self.input
                .set_placeholder(t("clipboard:header.searchPlaceholder"), window, cx);
        }
        let hint = |key: &str| self.hints.then(|| SharedString::from(key.to_owned()));
        let pin_label = if self.pinned {
            t("clipboard:header.unpin")
        } else {
            t("clipboard:header.pin")
        };

        div()
            .flex()
            .flex_none()
            .w_full()
            .items_center()
            .justify_between()
            .px(dp(12.))
            .pt(dp(12.))
            .pb(dp(8.))
            .child(
                div()
                    .flex_none()
                    .window_control_area(WindowControlArea::Drag)
                    .child(img(ImageSource::Image(logo())).size(dp(20.))),
            )
            .child(
                // 空白拖动区要有高度才能被命中（行内居中时空 div 的高度是 0）。
                div()
                    .flex_1()
                    .min_w_0()
                    .h(dp(32.))
                    .window_control_area(WindowControlArea::Drag),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(dp(4.))
                    .child(
                        div()
                            .id("clipboard-search")
                            .key_context(SEARCH_CONTEXT)
                            .capture_any_mouse_down(cx.listener(
                                |header, event: &MouseDownEvent, _, cx| {
                                    if event.button == MouseButton::Left && !header.editing {
                                        cx.emit(HeaderEvent::RequestEditing(EditTrigger::Mouse));
                                    }
                                },
                            ))
                            .child(
                                Input::search(&self.input)
                                    .small()
                                    .width(dp(160.))
                                    .hint_key(hint("F"))
                                    .accessibility_label(t("clipboard:header.searchPlaceholder")),
                            ),
                    )
                    .child(
                        icon_button(
                            tokens,
                            "clipboard-header-pin",
                            Glyph::Icon(IconName::PinWindow),
                            pin_label,
                            hint("P"),
                            if self.pinned {
                                Look::Selected
                            } else {
                                Look::Text
                            },
                        )
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(HeaderEvent::TogglePin))),
                    )
                    .child(
                        icon_button(
                            tokens,
                            "clipboard-header-preferences",
                            Glyph::Icon(IconName::SettingLine),
                            t("clipboard:header.openPreference"),
                            hint(","),
                            Look::Text,
                        )
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(HeaderEvent::OpenPreferences))),
                    ),
            )
    }
}
