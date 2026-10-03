//! 拖出（drag-out）：把记录拖到别的应用（附录 C §5）。这里是与平台无关的部分：载荷、结果和
//! 进程级的「正在拖出」标志；Windows 的 OLE 实现在 `win::drag_out`，macOS 尚未实现。
//!
//! 钩子、窗外点击自动隐藏和自拖过滤层都读 [`is_active`]：拖出期间 GPUI 的前台任务全部冻结，
//! 拖拽中要响应的事（Esc 取消）只能在钩子线程或 `IDropSource` 回调里做。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// 拖出的内容。图片记录拖原图文件（与 1.x 相同），所以只有文本和文件两种。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DragData {
    /// 纯文本必有；`html`、`rtf` 有值时一并提供，接收方按自己的偏好取（Word 取 RTF，浏览器、
    /// 富文本编辑器取 HTML，纯文本应用退回纯文本）。
    Text {
        plain: String,
        html: Option<String>,
        rtf: Option<String>,
    },
    /// 本地文件（CF_HDROP 等 Shell 格式）。
    Files(Vec<PathBuf>),
}

/// 一次拖出怎么结束的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragResult {
    /// 目标接受了（effect 为 COPY）。
    Dropped,
    /// 在某个窗口上松开，但它拒收（effect 为 NONE），包括松开在自己的面板上。
    Refused,
    /// 取消：Esc、松开在不接收拖放的地方，或自测保护。
    Cancelled,
}

/// 拖出结束后的读数。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DragReport {
    pub result: DragResult,
    /// 目标回给 `DoDragDrop` 的 effect。
    pub effect: u32,
    /// `DoDragDrop` 的返回码（`DRAGDROP_S_DROP` / `DRAGDROP_S_CANCEL` / 错误）。
    pub hresult: i32,
    /// 钩子收到 Esc、请求取消的时刻（[`crate::clock`] 刻度）；没按 Esc 时为 `None`。
    pub cancel_requested: Option<i64>,
}

static ACTIVE: AtomicBool = AtomicBool::new(false);

/// 本进程是否正在拖出。
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
pub(crate) fn set_active(active: bool) {
    ACTIVE.store(active, Ordering::SeqCst);
}
