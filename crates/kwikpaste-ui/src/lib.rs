//! gpui-component 的隔离层：应用只经这个 crate 使用 gpui-component，它的破坏性升级只改这里。
//!
//! - [`theme`]：冻结的 antd token、语义层 [`theme::KpTokens`]、字号与度量，明暗切换与文本缩放。
//! - 组件：[`Button`]、[`Checkbox`]、[`Switch`]、[`Input`]、[`Select`]、[`Tag`]、[`Kbd`]、[`KeyHint`]、
//!   Tooltip（[`TooltipExt`]）、[`toast`]、[`confirm()`]。
//! - 组件层不带文案：默认文字由应用经 [`set_ui_strings`] 注入。
// 渲染与事件回调里 panic 会让进程直接以 0xC0000409 退出，组件层一律不许 unwrap / expect / 越界下标。
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)
)]

mod button;
mod confirm;
mod icon;
mod input;
mod overlay;
mod select;
mod strings;
mod styled;
mod tag;
pub mod theme;
pub mod toast;
mod toggle;
mod tooltip;

use gpui::{AnyWindowHandle, App, AppContext, Entity, Render, Window, WindowOptions};

pub use button::{Button, ButtonKind, ButtonSize};
pub use confirm::{ConfirmBody, ConfirmSpec, confirm};
pub use icon::{Icon, IconName};
pub use input::{Input, InputSize, TextInput};
pub use select::{Select, SelectOption, SelectState};
pub use strings::{UiLocale, UiStrings, set_ui_strings, ui_strings};
pub use styled::KpStyled;
pub use tag::{Kbd, KeyHint, Shortcut, Tag, TagColor};
pub use toggle::{Checkbox, Switch};
pub use tooltip::{TooltipBubble, TooltipExt};

/// gpui-component 内置组件用到的图标资源（lucide 的默认子集）。创建 `Application` 时用
/// `.with_assets(kwikpaste_ui::Assets)` 装上，否则勾选框、下拉箭头等图标是空的。
pub use gpui_kit_assets::Assets;

/// 初始化组件层：gpui-component（连带 gpui-base）、浮层插件、主题。打开任何窗口之前调用一次。
pub fn init(cx: &mut App) {
    gpui_component::init(cx);
    overlay::init(cx);
    theme::init(cx);
}

/// 打开一个以 gpui-base `Root` 包裹的窗口，返回窗口句柄和根视图。
///
/// gpui-component 的弹层、对话框、主题，以及本 crate 的 toast 与 Tooltip 都依赖 `Root`，
/// 所以应用窗口一律经这里打开。窗口同时开始转发系统明暗变化。
pub fn open_window<V: Render>(
    options: WindowOptions,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> gpui::Result<(AnyWindowHandle, Entity<V>)> {
    let mut built = None;
    let window = cx.open_window(options, |window, cx| {
        let view = build(window, cx);
        built = Some(view.clone());
        cx.new(|cx| {
            cx.observe_window_appearance(window, |_, window, cx| {
                theme::sync_system_appearance(window, cx);
            })
            .detach();

            gpui_base::Root::new(view, window, cx)
        })
    })?;
    let view = built.ok_or_else(|| anyhow::anyhow!("open_window did not run its build closure"))?;

    Ok((window.into(), view))
}
