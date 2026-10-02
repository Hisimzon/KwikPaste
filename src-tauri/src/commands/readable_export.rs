//! 可读格式导出命令：只负责取得连接池、语言及转发。

use tauri::AppHandle;

use crate::core::Result;
use crate::db::DatabaseState;
use crate::readable_export::{ExportOptions, ExportPreview, ExportResult};

/// 查询导出范围的分组与记录数量，不修改历史数据。
#[tauri::command]
pub async fn preview_readable_export(
    app: AppHandle,
    db: tauri::State<'_, DatabaseState>,
    options: ExportOptions,
) -> Result<ExportPreview> {
    log::debug!("{} requested", crate::readable_export::PREVIEW_COMMAND);
    let pool = db.pool().await;
    crate::readable_export::preview(&pool, &options, crate::i18n::current_language(&app)).await
}

/// 预览指纹校验通过后生成可读文件，不参与备份恢复。
#[tauri::command]
pub async fn export_readable_data(
    app: AppHandle,
    db: tauri::State<'_, DatabaseState>,
    options: ExportOptions,
    fingerprint: String,
    target_path: String,
) -> Result<ExportResult> {
    log::debug!("{} requested", crate::readable_export::EXPORT_COMMAND);
    let pool = db.pool().await;
    crate::readable_export::export(
        &pool,
        options,
        fingerprint,
        target_path,
        crate::i18n::current_language(&app),
    )
    .await
}
