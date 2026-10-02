//! Windows 便携模式的判定。
//!
//! exe 同目录存在 [`MARKER_FILENAME`] 时进入便携模式：exe 旁的 `data/` 取代
//! `%LOCALAPPDATA%\<identifier>`，内部布局与之一致（`<env>/`、`logs/`），
//! 整个文件夹可以随 U 盘带走，本机不留数据。判定只看 exe 路径。

use std::path::{Path, PathBuf};

/// 便携标记文件名，随便携包分发；内容只是给用户看的说明。
pub const MARKER_FILENAME: &str = "portable.txt";
/// 便携数据根目录名，在 exe 旁边。
pub const DATA_DIR_NAME: &str = "data";
/// 便携模式的日志目录名，挂在便携数据根下。
pub const LOGS_DIR_NAME: &str = "logs";

/// 当前进程的便携数据根 `<exe 目录>/data`；非便携模式（以及 macOS）返回 `None`。
pub fn detect() -> Option<PathBuf> {
    if !cfg!(target_os = "windows") {
        return None;
    }

    let exe = std::env::current_exe().ok()?;
    data_root_for_exe(&exe)
}

/// 给定 exe 路径时的便携数据根：同目录有标记文件才算便携。
pub fn data_root_for_exe(exe: &Path) -> Option<PathBuf> {
    let exe_dir = exe.parent()?;

    exe_dir
        .join(MARKER_FILENAME)
        .is_file()
        .then(|| exe_dir.join(DATA_DIR_NAME))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn marker_next_to_exe_turns_on_portable_mode() {
        let dir = std::env::temp_dir().join(format!("kwikpaste-portable-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("KwikPaste.exe");

        assert_eq!(data_root_for_exe(&exe), None);

        fs::write(dir.join(MARKER_FILENAME), "portable").unwrap();
        assert_eq!(data_root_for_exe(&exe), Some(dir.join("data")));

        fs::remove_dir_all(&dir).ok();
    }
}
