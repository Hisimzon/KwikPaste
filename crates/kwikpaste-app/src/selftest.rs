//! 自测开关：必须同时传 `--selftest-*` 参数并设置 `KWIKPASTE_SELFTEST=1`，普通启动不会误触。

use std::time::Duration;

use gpui::App;

const ENV: &str = "KWIKPASTE_SELFTEST";
const SMOKE_FLAG: &str = "--selftest-smoke";
const SMOKE_DURATION: Duration = Duration::from_secs(3);

/// 冒烟自测：窗口打开几秒后正常退出，CI 用退出码判断应用能否启动并出画面。
pub fn schedule(cx: &mut App) {
    if !enabled(SMOKE_FLAG) {
        return;
    }

    cx.spawn(async move |cx| {
        cx.background_executor().timer(SMOKE_DURATION).await;
        cx.update(|cx| cx.quit());
    })
    .detach();
}

fn enabled(flag: &str) -> bool {
    std::env::var_os(ENV).is_some_and(|value| value == "1")
        && std::env::args().any(|arg| arg == flag)
}
