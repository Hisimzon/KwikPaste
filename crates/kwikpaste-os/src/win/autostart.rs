//! Windows 自启的注册表细节（见 [`crate::autostart`]）：HKLM 同名值的处理与清理。

use std::io;

use windows::core::{Error, HRESULT};
use windows_registry::{CURRENT_USER, Key, LOCAL_MACHINE};

const RUN_KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const STARTUP_APPROVED_RUN_KEY: &str =
    r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";
const ERROR_FILE_NOT_FOUND: u32 = 2;
const ERROR_ACCESS_DENIED: u32 = 5;

/// 打开自启前调用：HKLM 有同名值就先删；删不掉（没有权限）时清掉 HKCU 的、保留 HKLM 的作为
/// 唯一启动项并返回 `true`（调用方不要再写 HKCU，否则开机会启动两次）。
pub(crate) fn keep_system_entry(name: &str) -> io::Result<bool> {
    if !system_entry_exists(name)? {
        return Ok(false);
    }
    match remove_entry(LOCAL_MACHINE, name) {
        Ok(()) => Ok(false),
        Err(err) if err.code() == HRESULT::from_win32(ERROR_ACCESS_DENIED) => {
            remove_entry(CURRENT_USER, name).map_err(io::Error::other)?;
            log::warn!(
                "autostart: the HKLM Run value {name} cannot be removed; keeping it as the only startup entry"
            );
            Ok(true)
        }
        Err(err) => Err(io::Error::other(err)),
    }
}

/// 关闭自启：删 HKCU 的值；HKLM 只在真有这个值时才删（删不掉就报错，自启实际上还开着）。
pub(crate) fn remove_entries(name: &str) -> io::Result<()> {
    remove_entry(CURRENT_USER, name).map_err(io::Error::other)?;
    if system_entry_exists(name)? {
        remove_entry(LOCAL_MACHINE, name).map_err(io::Error::other)?;
    }

    Ok(())
}

fn system_entry_exists(name: &str) -> io::Result<bool> {
    match LOCAL_MACHINE
        .open(RUN_KEY)
        .and_then(|key| key.get_string(name))
    {
        Ok(_) => Ok(true),
        Err(err) if is_not_found(&err) => Ok(false),
        Err(err) => Err(io::Error::other(err)),
    }
}

/// 删 Run 值和任务管理器的启用标记；值本来就不在算成功。
fn remove_entry(root: &Key, name: &str) -> Result<(), Error> {
    match root
        .options()
        .write()
        .open(RUN_KEY)
        .and_then(|key| key.remove_value(name))
    {
        Ok(()) => {}
        Err(err) if is_not_found(&err) => {}
        Err(err) => return Err(err),
    }
    match root
        .options()
        .write()
        .open(STARTUP_APPROVED_RUN_KEY)
        .and_then(|key| key.remove_value(name))
    {
        Ok(()) => {}
        Err(err) if is_not_found(&err) => {}
        Err(err) => log::debug!("autostart: StartupApproved value {name} not removed: {err}"),
    }

    Ok(())
}

fn is_not_found(err: &Error) -> bool {
    err.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND)
}

/// 当前用户 Run 下这个值的内容（验证、诊断用）；没有时为 `None`。
pub fn current_user_value(name: &str) -> Option<String> {
    CURRENT_USER
        .open(RUN_KEY)
        .and_then(|key| key.get_string(name))
        .ok()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::autostart::Autostart;

    /// 测试结束（包括断言失败）时删掉测试写的值，不在本机留下启动项。
    struct Cleanup(String);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = remove_entry(CURRENT_USER, &self.0);
        }
    }

    #[test]
    fn enable_writes_the_quoted_path_and_disable_removes_it() {
        let name = format!("kwikpaste-os-test-autostart-{}", std::process::id());
        let _cleanup = Cleanup(name.clone());
        let entry = Autostart::new(
            &name,
            Path::new(r"C:\Program Files\Kwik Test\KwikPaste.exe"),
        )
        .expect("autostart");

        entry.set_enabled(true).expect("enable");
        assert_eq!(
            current_user_value(&name).as_deref(),
            Some(r#""C:\Program Files\Kwik Test\KwikPaste.exe" --auto-launch"#)
        );
        assert!(entry.is_enabled().expect("is_enabled"));

        entry.set_enabled(false).expect("disable");
        assert_eq!(current_user_value(&name), None);
        assert!(!entry.is_enabled().expect("is_enabled"));
        // 关两次也不报错。
        entry.set_enabled(false).expect("disable again");
    }
}
