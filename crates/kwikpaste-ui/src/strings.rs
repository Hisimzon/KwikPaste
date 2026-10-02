//! 组件层不带文案：确定、取消等默认文字由应用在初始化和切换语言时注入。

use gpui::{App, Global, SharedString};

/// 组件默认文案。
#[derive(Clone, Debug)]
pub struct UiStrings {
    /// 确认框的确定按钮。
    pub ok: SharedString,
    /// 确认框的取消按钮。
    pub cancel: SharedString,
    /// 选择器的占位文字。
    pub select_placeholder: SharedString,
}

impl Default for UiStrings {
    fn default() -> Self {
        Self {
            ok: "OK".into(),
            cancel: "Cancel".into(),
            select_placeholder: SharedString::default(),
        }
    }
}

impl Global for UiStrings {}

/// 界面语言，同时决定 gpui-component 内置文案（输入框菜单、日期等）用哪种语言。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiLocale {
    #[default]
    ZhCn,
    EnUs,
}

impl UiLocale {
    /// gpui-component 的 rust-i18n 语言名。
    fn kit_locale(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::EnUs => "en",
        }
    }
}

/// 注入组件默认文案并切换 gpui-component 的内置语言，然后刷新所有窗口。
pub fn set_ui_strings(locale: UiLocale, strings: UiStrings, cx: &mut App) {
    gpui_component::set_locale(locale.kit_locale());
    cx.set_global(strings);
    cx.refresh_windows();
}

pub fn ui_strings(cx: &App) -> UiStrings {
    cx.try_global::<UiStrings>().cloned().unwrap_or_default()
}
