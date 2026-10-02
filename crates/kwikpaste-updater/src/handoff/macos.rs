//! macOS：解压新的 `.app`、换掉当前的、等本进程退出后用 `open` 重新启动。
//!
//! 解包比 Tauri 多做几项检查：拒绝 `..`、绝对路径、硬链接、设备文件和指向外面的符号链接，
//! 跳过 AppleDouble 的 `._*` 条目（混进包里会破坏代码签名），写文件前确认目标仍在解包目录里。

use std::ffi::OsString;
use std::fs;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, anyhow, bail};

use super::Launcher;

/// 解压 `.app.tar.gz` 到 `dest`，去掉每个条目路径的第一段（`KwikPaste.app/`）。
pub(crate) fn extract_app(archive: &[u8], dest: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(dest).with_context(|| format!("failed to create {dest:?}"))?;
    let root = dest
        .canonicalize()
        .with_context(|| format!("failed to resolve {dest:?}"))?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(Cursor::new(archive)));
    let mut extracted = 0usize;

    for entry in archive.entries().context("failed to read update archive")? {
        let mut entry = entry.context("failed to read update archive")?;
        let path = entry
            .path()
            .context("invalid path in update archive")?
            .into_owned();
        let Some(relative) = strip_first_component(&path)? else {
            continue;
        };
        if relative
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("._"))
        {
            continue;
        }

        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir() || kind.is_symlink()) {
            bail!("unsupported entry {path:?} in update archive");
        }
        if kind.is_symlink() {
            let target = entry
                .link_name()
                .context("invalid link in update archive")?
                .ok_or_else(|| anyhow!("symlink without target in update archive"))?;
            if target.is_absolute()
                || escapes(&relative.parent().unwrap_or(Path::new("")).join(&target))
            {
                bail!("symlink {path:?} points outside the app");
            }
        }

        let target = root.join(&relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).with_context(|| format!("failed to create {parent:?}"))?;
            let resolved = parent
                .canonicalize()
                .with_context(|| format!("failed to resolve {parent:?}"))?;
            if !resolved.starts_with(&root) {
                bail!("entry {path:?} escapes the update directory");
            }
        }
        entry
            .unpack(&target)
            .with_context(|| format!("failed to unpack {path:?}"))?;
        extracted += 1;
    }

    if extracted == 0 {
        bail!("update archive is empty");
    }
    Ok(())
}

/// 去掉第一段（`./` 不算一段）；只有第一段的条目（包目录本身）返回 `None`。
fn strip_first_component(path: &Path) -> anyhow::Result<Option<PathBuf>> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("unsafe path {path:?} in update archive")
            }
        }
    }
    if parts.len() < 2 {
        return Ok(None);
    }
    Ok(Some(parts[1..].iter().collect()))
}

/// 相对路径按字面走一遍，是否会回到起点之外。
fn escapes(path: &Path) -> bool {
    let mut depth = 0i32;
    for component in path.components() {
        match component {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return true;
                }
            }
            Component::RootDir | Component::Prefix(_) => return true,
        }
    }
    false
}

/// 用 `new_bundle` 换掉 `bundle`：当前包先改名到旁边留底，新包就位后删掉旧的；
/// 新包放不进去就把旧的换回来。没有权限改名（装在管理员目录里）时用 osascript 申请管理员权限替换。
pub(crate) fn swap_bundle(
    bundle: &Path,
    new_bundle: &Path,
    launcher: &dyn Launcher,
) -> anyhow::Result<()> {
    let backup = previous_path(bundle);
    if backup.exists() {
        fs::remove_dir_all(&backup).ok();
    }

    match fs::rename(bundle, &backup) {
        Ok(()) => {
            if let Err(err) = fs::rename(new_bundle, bundle) {
                fs::rename(&backup, bundle).ok();
                return Err(anyhow::Error::new(err)
                    .context(format!("failed to move the new app into {bundle:?}")));
            }
            if let Err(err) = fs::remove_dir_all(&backup) {
                log::warn!("remove previous app {backup:?} failed: {err}");
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::PermissionDenied => {
            log::info!("replacing {bundle:?} needs administrator privileges");
            launcher
                .run(
                    Path::new("/usr/bin/osascript"),
                    &[
                        OsString::from("-e"),
                        OsString::from(privileged_script(bundle, new_bundle)),
                    ],
                )
                .context("failed to replace the app with administrator privileges")?;
        }
        Err(err) => {
            return Err(anyhow::Error::new(err)
                .context(format!("failed to move the current app {bundle:?} aside")));
        }
    }

    if let Err(err) = launcher.run(
        Path::new("/usr/bin/touch"),
        &[bundle.as_os_str().to_owned()],
    ) {
        log::warn!("touch {bundle:?} failed: {err:#}");
    }
    Ok(())
}

/// `/Applications/KwikPaste.app` → `/Applications/KwikPaste.app.previous`：与当前包同目录，改名不跨卷。
fn previous_path(bundle: &Path) -> PathBuf {
    let mut name = bundle.file_name().map(OsString::from).unwrap_or_default();
    name.push(".previous");
    bundle.with_file_name(name)
}

/// `do shell script "rm -rf '<app>' && mv -f '<new>' '<app>'" with administrator privileges`，
/// 路径先按 shell 单引号转义，再按 AppleScript 字符串转义。
pub(crate) fn privileged_script(bundle: &Path, new_bundle: &Path) -> String {
    let app = shell_quote(&bundle.to_string_lossy());
    let new = shell_quote(&new_bundle.to_string_lossy());
    let command = format!("rm -rf {app} && mv -f {new} {app}");
    format!(
        "do shell script \"{}\" with administrator privileges",
        command.replace('\\', "\\\\").replace('"', "\\\"")
    )
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// 本进程退出后用 LaunchServices 重新打开新包，原参数经 `--args` 带过去。
/// 参数走 `sh -c` 的位置参数，不拼进脚本文本，路径里有空格或引号也不会出错。
pub(crate) fn relaunch_command(
    pid: u32,
    bundle: &Path,
    args: &[OsString],
) -> (PathBuf, Vec<OsString>) {
    const SCRIPT: &str = "pid=$1; app=$2; shift 2; \
        while kill -0 \"$pid\" 2>/dev/null; do sleep 0.05; done; \
        if [ \"$#\" -gt 0 ]; then exec /usr/bin/open \"$app\" --args \"$@\"; \
        else exec /usr/bin/open \"$app\"; fi";

    let mut command = vec![
        OsString::from("-c"),
        OsString::from(SCRIPT),
        OsString::from("kwikpaste-relaunch"),
        OsString::from(pid.to_string()),
        bundle.as_os_str().to_owned(),
    ];
    command.extend(args.iter().cloned());
    (PathBuf::from("/bin/sh"), command)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::sync::Mutex;

    use super::*;

    fn archive(build: impl FnOnce(&mut tar::Builder<Vec<u8>>)) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        build(&mut builder);
        let tar = builder.into_inner().unwrap();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    fn file(builder: &mut tar::Builder<Vec<u8>>, path: &str, content: &[u8]) {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder.append_data(&mut header, path, content).unwrap();
    }

    /// 绕过 `append_data` 的路径检查，写出带 `..` 或绝对路径的恶意条目。
    fn raw_file(builder: &mut tar::Builder<Vec<u8>>, path: &str, content: &[u8]) {
        let mut header = tar::Header::new_old();
        header.as_old_mut().name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder.append(&header, content).unwrap();
    }

    fn app() -> Vec<u8> {
        archive(|builder| {
            file(builder, "KwikPaste.app/Contents/Info.plist", b"<plist/>");
            file(
                builder,
                "./KwikPaste.app/Contents/MacOS/KwikPaste",
                b"\xcf\xfa\xed\xfe new",
            );
            file(
                builder,
                "KwikPaste.app/Contents/._Info.plist",
                b"apple double",
            );
        })
    }

    #[test]
    fn extracts_the_bundle_contents_without_the_top_directory() {
        let temp = tempfile::tempdir().unwrap();
        let dest = temp.path().join("new.app");

        extract_app(&app(), &dest).unwrap();

        assert_eq!(
            fs::read(dest.join("Contents/Info.plist")).unwrap(),
            b"<plist/>"
        );
        assert!(dest.join("Contents/MacOS/KwikPaste").is_file());
        assert!(!dest.join("Contents/._Info.plist").exists());
    }

    #[test]
    fn rejects_entries_that_escape_the_bundle() {
        for path in [
            "KwikPaste.app/../../evil",
            "../evil",
            "/etc/evil",
            "KwikPaste.app/Contents/../../../evil",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let bytes = archive(|builder| raw_file(builder, path, b"evil"));
            assert!(
                extract_app(&bytes, &temp.path().join("new.app")).is_err(),
                "{path}"
            );
            assert!(!temp.path().join("evil").exists());
        }

        let temp = tempfile::tempdir().unwrap();
        let empty = archive(|builder| file(builder, "KwikPaste.app", b""));
        assert!(extract_app(&empty, &temp.path().join("new.app")).is_err());
    }

    #[test]
    fn rejects_symlinks_pointing_outside() {
        for target in ["/etc/passwd", "../../outside"] {
            let temp = tempfile::tempdir().unwrap();
            let bytes = archive(|builder| {
                let mut header = tar::Header::new_gnu();
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_size(0);
                header.set_mode(0o777);
                builder
                    .append_link(&mut header, "KwikPaste.app/Contents/link", target)
                    .unwrap();
            });
            assert!(
                extract_app(&bytes, &temp.path().join("new.app")).is_err(),
                "{target}"
            );
        }
    }

    #[test]
    fn strips_only_the_first_real_component() {
        assert_eq!(
            strip_first_component(Path::new("./KwikPaste.app/Contents/x")).unwrap(),
            Some(PathBuf::from("Contents/x"))
        );
        assert_eq!(
            strip_first_component(Path::new("KwikPaste.app/")).unwrap(),
            None
        );
        assert!(escapes(Path::new("a/../../b")));
        assert!(!escapes(Path::new("Versions/A/../Current")));
    }

    #[derive(Default)]
    struct Recorder {
        runs: Mutex<Vec<(PathBuf, Vec<OsString>)>>,
    }

    impl Launcher for Recorder {
        fn shell_execute(&self, _file: &Path, _parameters: &str) -> anyhow::Result<()> {
            unreachable!("macOS never runs installers")
        }

        fn spawn(&self, _program: &Path, _args: &[OsString]) -> anyhow::Result<()> {
            Ok(())
        }

        fn run(&self, program: &Path, args: &[OsString]) -> anyhow::Result<()> {
            self.runs
                .lock()
                .unwrap()
                .push((program.to_path_buf(), args.to_vec()));
            Ok(())
        }
    }

    #[test]
    fn swaps_the_bundle_and_removes_the_previous_one() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("KwikPaste.app");
        let new_bundle = temp.path().join("work").join("new.app");
        fs::create_dir_all(bundle.join("Contents")).unwrap();
        fs::write(bundle.join("Contents/version"), b"old").unwrap();
        fs::create_dir_all(new_bundle.join("Contents")).unwrap();
        fs::write(new_bundle.join("Contents/version"), b"new").unwrap();
        let launcher = Recorder::default();

        swap_bundle(&bundle, &new_bundle, &launcher).unwrap();

        assert_eq!(fs::read(bundle.join("Contents/version")).unwrap(), b"new");
        assert!(!previous_path(&bundle).exists());
        let runs = launcher.runs.into_inner().unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].0, PathBuf::from("/usr/bin/touch"));
    }

    #[test]
    fn failed_swap_puts_the_current_bundle_back() {
        let temp = tempfile::tempdir().unwrap();
        let bundle = temp.path().join("KwikPaste.app");
        fs::create_dir_all(&bundle).unwrap();
        fs::write(bundle.join("marker"), b"current").unwrap();

        let missing = temp.path().join("work").join("missing.app");
        assert!(swap_bundle(&bundle, &missing, &Recorder::default()).is_err());
        assert_eq!(fs::read(bundle.join("marker")).unwrap(), b"current");
    }

    #[test]
    fn privileged_script_quotes_paths() {
        let script = privileged_script(
            Path::new("/Applications/Kwik 'Paste\".app"),
            Path::new("/tmp/new.app"),
        );

        assert_eq!(
            script,
            "do shell script \"rm -rf '/Applications/Kwik '\\\\''Paste\\\".app' && mv -f '/tmp/new.app' '/Applications/Kwik '\\\\''Paste\\\".app'\" with administrator privileges"
        );
    }

    #[test]
    fn relaunch_passes_arguments_as_positional_parameters() {
        let (program, args) = relaunch_command(
            4242,
            Path::new("/Applications/Kwik Paste.app"),
            &[OsString::from("--auto-launch")],
        );

        assert_eq!(program, PathBuf::from("/bin/sh"));
        assert_eq!(args[0], "-c");
        assert!(args[1].to_string_lossy().contains("--args \"$@\""));
        assert_eq!(
            &args[3..],
            [
                OsString::from("4242"),
                OsString::from("/Applications/Kwik Paste.app"),
                OsString::from("--auto-launch")
            ]
        );
    }
}
