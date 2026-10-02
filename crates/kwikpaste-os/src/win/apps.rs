//! Windows 的应用识别（1.x `clipboard/source.rs` 与 `apps_registry.rs` 的 `mod windows`）。
//! 应用 id 是 exe 绝对路径，显示名是 exe 文件名去扩展名。

use std::collections::HashSet;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt as _;
use std::path::{Path, PathBuf};

use kwikpaste_core::db::models::Platform;
use kwikpaste_core::platform::{FrontmostApp, ScannedApp};
use kwikpaste_core::{AppError, Result};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetForegroundWindow, GetWindowTextLengthW, GetWindowThreadProcessId,
    IsWindowVisible,
};
use windows::core::{BOOL, PWSTR};

/// 当前前台窗口所属进程的 exe。在剪贴板变化回调里同步调用；拿不到时返回 `None`。
///
/// 不过滤自身：自身写回由 core 的写回守卫按内容判定，与 macOS 一致。
pub fn frontmost_app() -> Option<FrontmostApp> {
    let window = unsafe { GetForegroundWindow() };
    if window.is_invalid() {
        return None;
    }
    let path = process_image_name(window_process(window)?)?;
    // 与 1.x 一致：前台应用的 id 用系统给的原样路径，不做规范化。
    let id = String::from_utf16_lossy(&path);
    let name = Path::new(&id)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or(&id)
        .to_owned();

    Some(FrontmostApp {
        icon_source: Some(PathBuf::from(&id)),
        id,
        name,
        platform: Platform::Windows,
    })
}

/// 可见且有标题的顶层窗口所属进程的 exe，按规范化后的路径去重。
pub fn running_apps() -> Vec<ScannedApp> {
    let mut paths = Vec::<PathBuf>::new();
    let result = unsafe { EnumWindows(Some(collect_window_exe), LPARAM(&raw mut paths as isize)) };
    if let Err(err) = result {
        log::debug!("enumerating windows for running apps stopped early: {err}");
    }

    let mut seen = HashSet::new();
    paths
        .into_iter()
        .map(normalize_exe_path)
        .filter_map(|path| {
            let id = path.to_string_lossy().into_owned();
            seen.insert(id.clone()).then(|| ScannedApp {
                id,
                name: exe_display_name(&path),
                path: Some(path),
                platform: Platform::Windows,
            })
        })
        .collect()
}

/// 用户手动选择的应用：只接受 `.exe`，id 用规范化路径。调用方已确认路径存在。
pub fn app_from_path(path: &Path) -> Result<ScannedApp> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    if extension.as_deref() != Some("exe") {
        return Err(AppError::Clipboard(
            "please choose a Windows executable".to_owned(),
        ));
    }

    let normalized = path
        .canonicalize()
        .map(normalize_verbatim_path)
        .map_err(|_| AppError::Clipboard("app path cannot be resolved".to_owned()))?;
    Ok(ScannedApp {
        id: normalized.to_string_lossy().into_owned(),
        name: exe_display_name(&normalized),
        path: Some(normalized),
        platform: Platform::Windows,
    })
}

/// Windows 没有按 id 找应用的能力。
pub fn app_from_id(_id: &str) -> Option<ScannedApp> {
    None
}

unsafe extern "system" fn collect_window_exe(window: HWND, lparam: LPARAM) -> BOOL {
    let titled = unsafe { IsWindowVisible(window).as_bool() && GetWindowTextLengthW(window) > 0 };
    if !titled {
        return true.into();
    }
    if let Some(path) = window_process(window).and_then(process_image_name) {
        let paths = unsafe { &mut *(lparam.0 as *mut Vec<PathBuf>) };
        paths.push(PathBuf::from(OsString::from_wide(&path)));
    }

    true.into()
}

fn window_process(window: HWND) -> Option<u32> {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, Some(&raw mut pid)) };

    (pid != 0).then_some(pid)
}

/// 进程 exe 的完整路径（UTF-16）。`PROCESS_QUERY_LIMITED_INFORMATION` 不需要管理员权限。
fn process_image_name(pid: u32) -> Option<Vec<u16>> {
    let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let mut buffer = [0u16; 1024];
    let mut size = buffer.len() as u32;
    let queried = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut size,
        )
    };
    let _ = unsafe { CloseHandle(process) };
    if queried.is_err() || size == 0 {
        return None;
    }

    Some(buffer[..size as usize].to_vec())
}

fn normalize_exe_path(path: PathBuf) -> PathBuf {
    path.canonicalize()
        .map(normalize_verbatim_path)
        .unwrap_or(path)
}

/// 把 `canonicalize` 给出的 verbatim 路径（`\\?\` 前缀）还原成 Shell 图标 API 接受的形式。
fn normalize_verbatim_path(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(rest) = raw.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    if let Some(rest) = raw.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }

    path
}

fn exe_display_name(path: &Path) -> String {
    match path.file_stem().and_then(|stem| stem.to_str()) {
        Some(name) => name.to_owned(),
        None => path.to_string_lossy().into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbatim_prefixes_are_stripped() {
        assert_eq!(
            normalize_verbatim_path(PathBuf::from(r"\\?\C:\Tools\app.exe")),
            PathBuf::from(r"C:\Tools\app.exe")
        );
        assert_eq!(
            normalize_verbatim_path(PathBuf::from(r"\\?\UNC\server\share\app.exe")),
            PathBuf::from(r"\\server\share\app.exe")
        );
        assert_eq!(
            normalize_verbatim_path(PathBuf::from(r"C:\Tools\app.exe")),
            PathBuf::from(r"C:\Tools\app.exe")
        );
    }

    #[test]
    fn an_exe_is_named_after_its_file_stem() {
        assert_eq!(
            exe_display_name(Path::new(r"C:\Program Files\Foo\Foo Bar.exe")),
            "Foo Bar"
        );
    }

    #[test]
    fn only_executables_can_be_chosen() {
        let not_exe = std::env::temp_dir();

        let err = app_from_path(&not_exe).unwrap_err();

        assert_eq!(err.to_string(), "please choose a Windows executable");
    }

    #[test]
    fn a_chosen_executable_gets_its_canonical_path_as_id() {
        let exe = std::env::current_exe().expect("test exe path");

        let app = app_from_path(&exe).expect("the test exe is an app");

        assert!(!app.id.starts_with(r"\\?\"), "{}", app.id);
        assert_eq!(app.path.as_deref(), Some(Path::new(&app.id)));
        assert_eq!(app.name, exe.file_stem().unwrap().to_string_lossy());
        assert_eq!(app.platform, Platform::Windows);
    }

    #[test]
    fn running_apps_are_unique_exe_paths() {
        let apps = running_apps();

        let mut ids = HashSet::new();
        for app in &apps {
            assert!(ids.insert(app.id.as_str()), "duplicate {}", app.id);
            assert!(!app.id.starts_with(r"\\?\"), "{}", app.id);
        }
    }

    #[test]
    fn this_process_image_name_is_its_exe() {
        let path = process_image_name(std::process::id()).expect("own image name");

        let own = std::env::current_exe().expect("test exe path");
        assert_eq!(
            PathBuf::from(OsString::from_wide(&path))
                .canonicalize()
                .ok(),
            own.canonicalize().ok()
        );
    }
}
