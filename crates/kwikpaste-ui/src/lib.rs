//! gpui-component 的隔离层：应用只经这个 crate 使用 gpui-component，它的破坏性升级只改这里。

use gpui::{AnyWindowHandle, App, AppContext, Entity, Render, Window, WindowOptions};

/// 初始化 gpui-component（同时初始化 gpui-base）。打开任何窗口之前调用一次。
pub fn init(cx: &mut App) {
    gpui_component::init(cx);
}

/// 打开一个以 gpui-base `Root` 包裹的窗口，返回窗口句柄和根视图。
///
/// gpui-component 的弹层、通知和主题都依赖 `Root`，所以应用窗口一律经这里打开。
pub fn open_window<V: Render>(
    options: WindowOptions,
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> gpui::Result<(AnyWindowHandle, Entity<V>)> {
    let mut built = None;
    let window = cx.open_window(options, |window, cx| {
        let view = build(window, cx);
        built = Some(view.clone());
        cx.new(|cx| gpui_base::Root::new(view, window, cx))
    })?;

    Ok((
        window.into(),
        built.expect("open_window ran its build closure"),
    ))
}
