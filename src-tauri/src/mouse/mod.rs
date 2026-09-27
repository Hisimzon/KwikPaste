//! Windows 全局鼠标钩子（`WH_MOUSE_LL`），两项功能共用一颗钩子：
//!
//! - 失焦隐藏：剪贴板窗口 `focusable=false` 让 Tauri 在 Windows 上收不到 `tauri://blur`，
//!   无法用焦点事件实现「失焦自动隐藏」。剪贴板窗口可见期间监听全局鼠标按下，
//!   命中剪贴板窗口外的位置就让它隐藏。macOS 走 NSPanel 的 `window_did_resign_key`，无需本模块。
//! - 鼠标按键唤起：设置开启期间常驻，接管选定的中键或侧键，单击打开或隐藏剪贴板窗口，
//!   按键原有的单击功能随之停用（中键拖动仍交还给应用）。两项放在同一颗钩子里，
//!   被接管的按下就不会再被当成窗外点击。
//!
//! 故仅 windows target 启用。

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "windows")]
pub use windows::{disable_outside_click_hide, enable_outside_click_hide, set_mouse_trigger};
