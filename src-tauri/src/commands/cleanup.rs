//! 历史自动清理的命令入口：状态查询、按候选设置预演、立即清理。

use tauri::AppHandle;

use crate::clipboard::{CleanupPreview, CleanupReport, CleanupStatus};
use crate::core::Result;
use crate::settings::History;

/// 读取最近一次清理结果与存储占用检查。
#[tauri::command]
pub async fn get_cleanup_status(app: AppHandle) -> Result<CleanupStatus> {
    Ok(crate::clipboard::cleanup_status(&app))
}

/// 按候选清理设置预演一轮清理，不删除任何记录；偏好页保存前用它确认影响范围。
#[tauri::command]
pub async fn preview_history_cleanup(app: AppHandle, history: History) -> Result<CleanupPreview> {
    crate::clipboard::preview_cleanup(&app, &history).await
}

/// 按当前设置立即完整清理一次。
#[tauri::command]
pub async fn run_history_cleanup(app: AppHandle) -> Result<CleanupReport> {
    crate::clipboard::run_cleanup_now(&app).await
}
