//! 快贴原生版（GPUI）入口。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod core_host;
mod gallery;
mod i18n;
mod identity;
mod platform;
mod selftest;

use gpui::{
    App, AppContext, Application, Context, IntoElement, ParentElement, Render, Styled, Window, div,
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
    let Some(launch) = platform::launch()? else {
        return Ok(());
    };
    let platform = platform::create()?;

    Application::with_platform(platform)
        .with_assets(kwikpaste_ui::Assets)
        .run(move |cx: &mut App| {
            kwikpaste_ui::init(cx);
            i18n::init(cx);

            // 组件展示窗是自测模式：不建面板、托盘和热键，关掉窗口就退出。
            if gallery::open_if_requested(cx) {
                cx.on_window_closed(|cx, _| {
                    if cx.windows().is_empty() {
                        cx.quit();
                    }
                })
                .detach();
                return;
            }

            if let Err(err) = platform::start(cx, launch, |_, cx| cx.new(|_| Home)) {
                log::error!("the panel could not be created: {err:#}");
                std::process::exit(1);
            }

            selftest::schedule(cx);
        });

    Ok(())
}
