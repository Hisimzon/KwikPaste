//! Rust 侧用户可见文案。
//!
//! 仅放会直接展示给用户的短文案；日志与内部错误上下文不走这里。

pub mod announcement;
pub mod clipboard_menu;
pub mod commands;
mod en_us;
mod keys;
pub mod readable_export;
#[cfg(target_os = "windows")]
pub mod startup;
pub mod tray;
mod zh_cn;

use crate::settings::{Language, SettingsStore};

/// 读取当前设置语言。
pub fn current_language(settings: &SettingsStore) -> Language {
    settings.snapshot().appearance.language
}
