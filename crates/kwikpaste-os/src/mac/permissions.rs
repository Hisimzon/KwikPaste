//! macOS 首次引导需要的权限检测与系统设置入口。

use std::{io, process::Command};

/// 打开辅助功能权限页。
pub fn open_accessibility_settings() -> io::Result<()> {
    open_privacy_pane("Privacy_Accessibility")
}

/// 打开完全磁盘访问权限页。
pub fn open_full_disk_access_settings() -> io::Result<()> {
    open_privacy_pane("Privacy_AllFiles")
}

fn open_privacy_pane(anchor: &str) -> io::Result<()> {
    Command::new("open")
        .arg(format!(
            "x-apple.systempreferences:com.apple.preference.security?{anchor}"
        ))
        .spawn()
        .map(|_| ())
}
