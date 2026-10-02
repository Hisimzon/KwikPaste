use std::path::PathBuf;

use anyhow::Context;

use crate::error::Result;
use crate::paths::CorePaths;

const DB_FILENAME: &str = "clipboard.db";

pub fn db_path(paths: &CorePaths) -> Result<PathBuf> {
    // 数据库连同 WAL 的两个 sidecar（-wal/-shm）落在独立 db 目录下。
    // dev/prod 的隔离由 CorePaths 的环境子目录（dev/ vs prod/）保证，文件名不带后缀。
    let dir = paths.db_dir()?;

    std::fs::create_dir_all(&dir).with_context(|| format!("failed to create db dir at {dir:?}"))?;

    Ok(dir.join(DB_FILENAME))
}
