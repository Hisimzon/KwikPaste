//! 固定窗口与点外部隐藏（1.x `CLIPBOARD_WINDOW_PINNED` 与 `AUTO_HIDE_SUSPENDED`）。
//!
//! - 固定（头部的图钉按钮、Mod+P）：点面板外部不隐藏，粘贴、复制之后面板留着；托盘、Esc 照常隐藏。
//!   Windows 上点外部、粘贴、复制之后把按键还给目标应用，这时唤起热键先把按键拉回面板，再按才隐藏。
//!   固定状态跨显示保留，与 1.x 相同。
//! - 打开系统文件对话框（保存图片、导入分组图标）期间临时不因点外部隐藏：对话框是另一个前台窗口，
//!   在它上面点击就是点面板外部。
//!
//! 平台层只认 `PanelCommand::SetHideOnOutsideClick`，两个原因在这里合并；输入捕获状态由平台层按
//! 鼠标、粘贴和复制结果同步。

use std::path::{Path, PathBuf};

use gpui::{App, Global, PathPromptOptions, Task};

use super::request_panel;
use crate::platform::PanelCommand;

#[derive(Default)]
struct WindowPin {
    pinned: bool,
    /// 正在打开的系统文件对话框个数。
    dialogs: u32,
}

impl Global for WindowPin {}

/// 面板是否固定。
pub fn pinned(cx: &App) -> bool {
    cx.try_global::<WindowPin>().is_some_and(|pin| pin.pinned)
}

pub fn set_pinned(pinned: bool, cx: &mut App) {
    cx.default_global::<WindowPin>().pinned = pinned;
    sync(cx);
}

fn sync(cx: &mut App) {
    let pin = cx.default_global::<WindowPin>();
    let hide = !pin.pinned && pin.dialogs == 0;
    request_panel(cx, PanelCommand::SetHideOnOutsideClick(hide));
}

fn begin_dialog(cx: &mut App) {
    cx.default_global::<WindowPin>().dialogs += 1;
    sync(cx);
}

fn end_dialog(cx: &mut App) {
    let pin = cx.default_global::<WindowPin>();
    pin.dialogs = pin.dialogs.saturating_sub(1);
    sync(cx);
}

/// 系统的另存为对话框；取消或失败时为 `None`（失败写日志）。
pub fn prompt_for_new_path(directory: &Path, name: &str, cx: &mut App) -> Task<Option<PathBuf>> {
    begin_dialog(cx);
    let answer = cx.prompt_for_new_path(directory, Some(name));

    cx.spawn(async move |cx| {
        let path = match answer.await {
            Ok(Ok(path)) => path,
            Ok(Err(err)) => {
                log::warn!("save dialog failed: {err:#}");
                None
            }
            Err(_) => None,
        };
        cx.update(end_dialog);
        path
    })
}

/// 系统的打开文件对话框；取消或失败时为 `None`（失败写日志）。
pub fn prompt_for_paths(options: PathPromptOptions, cx: &mut App) -> Task<Option<Vec<PathBuf>>> {
    begin_dialog(cx);
    let answer = cx.prompt_for_paths(options);

    cx.spawn(async move |cx| {
        let paths = match answer.await {
            Ok(Ok(paths)) => paths,
            Ok(Err(err)) => {
                log::warn!("open dialog failed: {err:#}");
                None
            }
            Err(_) => None,
        };
        cx.update(end_dialog);
        paths
    })
}
