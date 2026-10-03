//! macOS 首次引导需要的权限检测与系统设置入口。

use std::{io, process::Command};

/// 打开完全磁盘访问权限页。
pub fn open_full_disk_access_settings() -> io::Result<()> {
    Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles")
        .spawn()
        .map(|_| ())
}
