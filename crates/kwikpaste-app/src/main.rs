//! 快贴原生版（GPUI）入口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod platform;
mod selftest;

use gpui::{
    App, AppContext, Application, Context, IntoElement, ParentElement, Render, Styled, Window,
    WindowBounds, WindowOptions, div, px, size,
};

struct Home;

impl Render for Home {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(concat!("KwikPaste ", env!("CARGO_PKG_VERSION")))
    }
}

fn main() -> anyhow::Result<()> {
    let platform = platform::create()?;

    Application::with_platform(platform).run(|cx: &mut App| {
        kwikpaste_ui::init(cx);

        let options = WindowOptions {
            window_bounds: Some(WindowBounds::centered(size(px(480.), px(320.)), cx)),
            ..Default::default()
        };
        kwikpaste_ui::open_window(options, cx, |_, cx| cx.new(|_| Home))
            .expect("the main window opens");

        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        selftest::schedule(cx);
    });

    Ok(())
}
