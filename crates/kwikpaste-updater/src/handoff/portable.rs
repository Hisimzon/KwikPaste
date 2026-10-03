//! 便携版自更新（从 1.x 的 `update/portable.rs` 搬来）。
//!
//! Windows 允许给运行中的 exe 改名、不允许覆盖：新 exe 先写到旁边，当前 exe 改名为 `.old`，
//! 新 exe 再改回原名。改名后 `current_exe()` 仍返回原路径，重启拉起的就是新版本。
//! `.old` 仍被当前进程占用，通常当场删不掉，由新进程在启动时清理。

use std::ffi::OsString;
use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, anyhow};
use zip::ZipArchive;

const STAGED_SUFFIX: &str = "new";
const REPLACED_SUFFIX: &str = "old";
/// 旧进程退出、映像解除占用需要一点时间，启动清理按这个间隔重试。
const CLEANUP_RETRY_INTERVAL: Duration = Duration::from_secs(1);
const CLEANUP_ATTEMPTS: u32 = 10;

/// 删除上次自更新留下的 `.old` 与未完成的 `.new`，在后台线程里重试直到旧进程释放文件。
pub(crate) fn cleanup_leftovers(exe: &Path) {
    let leftovers = [sibling(exe, REPLACED_SUFFIX), sibling(exe, STAGED_SUFFIX)];
    if leftovers.iter().all(|path| !path.exists()) {
        return;
    }

    let spawned = std::thread::Builder::new()
        .name("portable-cleanup".to_owned())
        .spawn(move || {
            for _ in 0..CLEANUP_ATTEMPTS {
                let pending = leftovers
                    .iter()
                    .filter(|path| path.exists() && fs::remove_file(path).is_err())
                    .count();
                if pending == 0 {
                    return;
                }

                std::thread::sleep(CLEANUP_RETRY_INTERVAL);
            }

            log::warn!("portable update leftovers are still locked: {leftovers:?}");
        });
    if let Err(err) = spawned {
        log::warn!("portable update cleanup could not start: {err}");
    }
}

/// 从便携包里取出唯一的 exe。签名已经验过，这里只防打包出错把 exe 换成坏文件。
pub(crate) fn extract_binary(package: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut archive =
        ZipArchive::new(Cursor::new(package)).context("failed to read portable package")?;
    let mut exe_index = None;

    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .context("failed to read portable package entry")?;
        if !entry.is_file()
            || !entry_file_name(entry.name())
                .to_ascii_lowercase()
                .ends_with(".exe")
        {
            continue;
        }

        if exe_index.replace(index).is_some() {
            return Err(anyhow!(
                "portable package contains more than one executable"
            ));
        }
    }

    let index = exe_index.ok_or_else(|| anyhow!("portable package contains no executable"))?;
    let entry = archive
        .by_index(index)
        .context("failed to read portable package entry")?;
    let size = entry.size();
    let binary = crate::download::read_entry(entry, size)
        .context("failed to extract executable from portable package")?;

    if !binary.starts_with(b"MZ") {
        return Err(anyhow!(
            "portable package executable is not a Windows program"
        ));
    }

    Ok(binary)
}

/// 先完整写好新 exe 再换名；任何一步失败都回滚，保证原位置始终有一个能启动的 exe。
pub(crate) fn replace_binary(exe: &Path, binary: &[u8]) -> anyhow::Result<()> {
    let staged = sibling(exe, STAGED_SUFFIX);
    let replaced = sibling(exe, REPLACED_SUFFIX);

    fs::write(&staged, binary).with_context(|| format!("failed to write {staged:?}"))?;

    // 上次留下的 `.old` 可能还在；删不掉也没关系，改名时会直接覆盖。
    let _ = fs::remove_file(&replaced);

    if let Err(err) = fs::rename(exe, &replaced) {
        let _ = fs::remove_file(&staged);
        return Err(anyhow::Error::new(err)
            .context(format!("failed to move current executable to {replaced:?}")));
    }

    if let Err(err) = fs::rename(&staged, exe) {
        let _ = fs::rename(&replaced, exe);
        let _ = fs::remove_file(&staged);
        return Err(
            anyhow::Error::new(err).context(format!("failed to move new executable to {exe:?}"))
        );
    }

    if let Err(err) = fs::remove_file(&replaced) {
        log::info!("previous executable stays until next launch: {err}");
    }

    Ok(())
}

/// zip 规范用 `/`，但 Windows PowerShell 5.1 的 `Compress-Archive` 会写成 `\`，两种都认。
fn entry_file_name(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// `KwikPaste.exe` → `KwikPaste.exe.<suffix>`，与 exe 同目录，保证改名不跨卷。
fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.file_name().map(OsString::from).unwrap_or_default();
    name.push(".");
    name.push(suffix);

    exe.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use zip::ZipWriter;
    use zip::write::SimpleFileOptions;

    use super::*;

    fn package(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(content).unwrap();
        }

        zip.finish().unwrap().into_inner()
    }

    #[test]
    fn extracts_the_only_executable() {
        let bytes = package(&[
            ("KwikPaste/portable.txt", b"marker"),
            ("KwikPaste/KwikPaste.exe", b"MZ new"),
        ]);

        assert_eq!(extract_binary(&bytes).unwrap(), b"MZ new");
    }

    #[test]
    fn accepts_backslash_entry_names() {
        let bytes = package(&[("KwikPaste\\KwikPaste.EXE", b"MZ new")]);

        assert_eq!(extract_binary(&bytes).unwrap(), b"MZ new");
    }

    #[test]
    fn rejects_packages_without_a_single_valid_executable() {
        assert!(extract_binary(&package(&[("KwikPaste/portable.txt", b"marker")])).is_err());
        assert!(extract_binary(&package(&[("a.exe", b"MZ a"), ("b.exe", b"MZ b")])).is_err());
        assert!(extract_binary(&package(&[("KwikPaste.exe", b"not a program")])).is_err());
    }

    #[test]
    fn replaces_binary_in_place_and_cleans_up() {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("KwikPaste.exe");
        fs::write(&exe, b"MZ old").unwrap();
        fs::write(sibling(&exe, REPLACED_SUFFIX), b"leftover").unwrap();

        replace_binary(&exe, b"MZ new").unwrap();

        assert_eq!(fs::read(&exe).unwrap(), b"MZ new");
        assert!(!sibling(&exe, STAGED_SUFFIX).exists());
        assert!(!sibling(&exe, REPLACED_SUFFIX).exists());
    }

    #[test]
    fn keeps_current_binary_when_it_cannot_be_moved() {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("KwikPaste.exe");

        assert!(replace_binary(&exe, b"MZ new").is_err());
        assert!(!sibling(&exe, STAGED_SUFFIX).exists());
    }

    #[test]
    fn leftovers_are_removed_in_the_background() {
        let temp = tempfile::tempdir().unwrap();
        let exe = temp.path().join("KwikPaste.exe");
        fs::write(sibling(&exe, REPLACED_SUFFIX), b"old").unwrap();
        fs::write(sibling(&exe, STAGED_SUFFIX), b"new").unwrap();

        cleanup_leftovers(&exe);

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while sibling(&exe, REPLACED_SUFFIX).exists() || sibling(&exe, STAGED_SUFFIX).exists() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
