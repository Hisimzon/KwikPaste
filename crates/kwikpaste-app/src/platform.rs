//! 创建 GPUI 平台。
//!
//! Windows 上不走 `gpui_platform::application()`：它对 `WindowsPlatform::new` 的错误直接 panic，
//! 这里自己调用、拿到 `Result`，初始化失败时才有机会告诉用户原因。

use std::rc::Rc;

use gpui::Platform;

#[cfg(target_os = "windows")]
pub fn create() -> anyhow::Result<Rc<dyn Platform>> {
    let platform = gpui_windows::WindowsPlatform::new(false)?;

    Ok(Rc::new(platform))
}

#[cfg(target_os = "macos")]
pub fn create() -> anyhow::Result<Rc<dyn Platform>> {
    Ok(gpui_platform::current_platform(false))
}
