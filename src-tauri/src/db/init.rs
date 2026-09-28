use anyhow::Context;
use sqlx::sqlite::{
    SqliteAutoVacuum, SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
};
use sqlx::ConnectOptions;
use sqlx::SqlitePool;
use tauri::AppHandle;

use crate::core::Result;
use crate::db::db_path;

pub async fn init(app: &AppHandle) -> Result<SqlitePool> {
    let path = db_path(app)?;

    // 新库建表前就开启增量 auto_vacuum，自动清理删行后可以随时收缩文件；
    // 老库要等偏好页「清理缓存」整库 VACUUM 一次才会切过去，在此之前这条 pragma 不生效也不报错。
    let options = SqliteConnectOptions::new()
        .filename(&path)
        .create_if_missing(true)
        .auto_vacuum(SqliteAutoVacuum::Incremental)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .disable_statement_logging();

    let pool = SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .with_context(|| format!("failed to open sqlite database at {path:?}"))?;

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("failed to run sqlite migrations")?;

    log::info!("sqlite pool ready at {path:?}");
    Ok(pool)
}
