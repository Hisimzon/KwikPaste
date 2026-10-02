//! 本地数据目录的体积统计，供偏好页存储占用与按存储上限清理共用。

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::error::Result;

/// 文件不存在时按 0 处理，避免首次启动时显示错误状态。
pub fn file_size(path: &Path) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }

    Ok(fs::metadata(path)
        .with_context(|| format!("failed to read metadata at {path:?}"))?
        .len())
}

/// 递归统计目录大小；目录不存在时按 0 处理。
pub fn dir_size(path: &Path) -> Result<u64> {
    dir_size_excluding(path, &[])
}

/// 递归统计目录大小，跳过 `excluded` 里列出的文件；目录不存在时按 0 处理。
pub fn dir_size_excluding(path: &Path, excluded: &[PathBuf]) -> Result<u64> {
    if !path.exists() {
        return Ok(0);
    }

    let mut total = 0;
    for entry in
        fs::read_dir(path).with_context(|| format!("failed to read directory at {path:?}"))?
    {
        let entry = entry.with_context(|| format!("failed to read entry under {path:?}"))?;
        let entry_path = entry.path();
        if excluded.contains(&entry_path) {
            continue;
        }
        let metadata = entry
            .metadata()
            .with_context(|| format!("failed to read metadata at {entry_path:?}"))?;

        if metadata.is_dir() {
            total += dir_size_excluding(&entry_path, excluded)?;
            continue;
        }

        total += metadata.len();
    }

    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excluded_files_are_skipped_while_walking() {
        let temp = tempfile::tempdir().unwrap();
        let db = temp.path().join("db");
        fs::create_dir_all(&db).unwrap();
        fs::write(db.join("clipboard.db"), vec![0u8; 4096]).unwrap();
        fs::write(db.join("clipboard.db-wal"), vec![0u8; 1000]).unwrap();
        fs::write(db.join("clipboard.db-shm"), vec![0u8; 300]).unwrap();
        fs::write(temp.path().join("settings.json"), vec![0u8; 20]).unwrap();
        // 只跳过点名的旁路文件，别处同名后缀的文件照常计入。
        fs::write(temp.path().join("notes-wal"), vec![0u8; 7]).unwrap();

        let excluded = [db.join("clipboard.db-wal"), db.join("clipboard.db-shm")];

        assert_eq!(dir_size(temp.path()).unwrap(), 4096 + 1000 + 300 + 20 + 7);
        assert_eq!(
            dir_size_excluding(temp.path(), &excluded).unwrap(),
            4096 + 20 + 7
        );
        assert_eq!(
            dir_size_excluding(&temp.path().join("missing"), &excluded).unwrap(),
            0
        );
    }
}
