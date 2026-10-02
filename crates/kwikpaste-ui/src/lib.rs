//! gpui-component 的隔离层：应用只经这个 crate 使用 gpui-component，它的破坏性升级只改这里。
//!
//! - [`theme`]：冻结的 antd token、语义层 [`theme::KpTokens`]、字号与度量，明暗切换与文本缩放。
// 渲染与事件回调里 panic 会让进程直接以 0xC0000409 退出，组件层一律不许 unwrap / expect / 越界下标。
#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)
)]

mod styled;
pub mod theme;

use gpui::{AnyWindowHandle, App, AppContext, Entity, Render, Window, WindowOptions};

pub use styled::KpStyled;

/// 初始化组件层：gpui-component（连带 gpui-base）与主题。打开任何窗口之前调用一次。
pub fn init(cx: &mut App) {
    gpui_component::init(cx);
    theme::init(cx);
}

/// 打开一个以 gpui-base `Root` 包裹的窗口，返回窗口句柄和根视图。
///
/// gpui-component 的弹层、对话框和主题都依赖 `Root`，所以应用窗口一律经这里打开。
/// 窗口同时开始转发系统明暗变化。
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
