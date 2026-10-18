//! 反馈用的只读系统探针；各字段独立失败，不采集窗口标题或完整进程路径。

#[derive(Default)]
pub struct Snapshot {
    pub os_version: Option<String>,
    pub elevated: Option<bool>,
    pub display_scales: Option<Vec<Option<f64>>>,
    pub foreground_exe: Option<String>,
    pub foreground_privilege: Option<&'static str>,
}

/// macOS 不适用的权限探针保留 null；其余平台信息由宿主补齐。
#[cfg(target_os = "macos")]
pub fn snapshot() -> Snapshot {
    let os_version = std::process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|version| version.trim().to_owned());
    Snapshot {
        os_version,
        ..Snapshot::default()
    }
}

#[cfg(target_os = "windows")]
pub use windows_probes::snapshot;

#[cfg(target_os = "windows")]
mod windows_probes {
    use super::Snapshot;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_ELEVATION,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenElevation, TokenIntegrityLevel,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    use windows::core::{BOOL, PWSTR};

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    fn token(process: HANDLE) -> Option<Handle> {
        let mut token = HANDLE::default();
        unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
        Some(Handle(token))
    }

    fn elevated(token: HANDLE) -> Option<bool> {
        let mut value = TOKEN_ELEVATION::default();
        let mut length = 0;
        unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                Some((&raw mut value).cast()),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut length,
            )
        }
        .ok()?;
        Some(value.TokenIsElevated != 0)
    }

    /// UIPI 比较完整性级别，而非仅比较管理员标志（低完整性与普通进程也不同）。
    fn integrity(token: HANDLE) -> Option<u32> {
        let mut length = 0;
        let _ = unsafe { GetTokenInformation(token, TokenIntegrityLevel, None, 0, &mut length) };
        if length < size_of::<TOKEN_MANDATORY_LABEL>() as u32 || length > 65536 {
            return None;
        }
        let mut buffer = vec![0usize; (length as usize).div_ceil(size_of::<usize>())];
        unsafe {
            GetTokenInformation(
                token,
                TokenIntegrityLevel,
                Some(buffer.as_mut_ptr().cast()),
                length,
                &mut length,
            )
        }
        .ok()?;
        let label = unsafe { &*buffer.as_ptr().cast::<TOKEN_MANDATORY_LABEL>() };
        let count = unsafe { *GetSidSubAuthorityCount(label.Label.Sid) };
        if count == 0 {
            return None;
        }
        Some(unsafe { *GetSidSubAuthority(label.Label.Sid, u32::from(count - 1)) })
    }

    fn process_name(process: HANDLE) -> Option<String> {
        let mut buffer = vec![0u16; 32768];
        let mut length = buffer.len() as u32;
        unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(buffer.as_mut_ptr()),
                &mut length,
            )
        }
        .ok()?;
        let path = String::from_utf16(buffer.get(..length as usize)?).ok()?;
        path.rsplit(['\\', '/']).next().map(str::to_owned)
    }

    fn display_scales() -> Option<Vec<Option<f64>>> {
        unsafe extern "system" fn collect(
            monitor: HMONITOR,
            _: HDC,
            _: *mut RECT,
            data: LPARAM,
        ) -> BOOL {
            let values = unsafe { &mut *(data.0 as *mut Vec<Option<f64>>) };
            let (mut x, mut y) = (0, 0);
            let scale = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut x, &mut y) }
                .ok()
                .map(|_| f64::from(x) / 96.0);
            values.push(scale);
            true.into()
        }
        let mut values = Vec::<Option<f64>>::new();
        unsafe { EnumDisplayMonitors(None, None, Some(collect), LPARAM(&raw mut values as isize)) }
            .as_bool()
            .then_some(values)
    }

    pub fn snapshot() -> Snapshot {
        let os_version = windows_registry::LOCAL_MACHINE
            .open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
            .ok()
            .and_then(|key| {
                let major = key.get_u32("CurrentMajorVersionNumber").ok()?;
                let minor = key.get_u32("CurrentMinorVersionNumber").ok()?;
                let build = key.get_string("CurrentBuildNumber").ok()?;
                Some(format!("{major}.{minor}.{build}"))
            });
        let own_token = token(unsafe { GetCurrentProcess() });
        let mut snapshot = Snapshot {
            os_version,
            elevated: own_token.as_ref().and_then(|token| elevated(token.0)),
            display_scales: display_scales(),
            ..Snapshot::default()
        };
        let window = unsafe { GetForegroundWindow() };
        if window.0.is_null() {
            return snapshot;
        }
        let mut pid = 0;
        unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
        if pid == 0 {
            return snapshot;
        }
        let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
        else {
            snapshot.foreground_privilege = Some("likely-higher");
            return snapshot;
        };
        let process = Handle(process);
        snapshot.foreground_exe = process_name(process.0);
        snapshot.foreground_privilege = match token(process.0) {
            None => Some("likely-higher"),
            Some(foreground) => own_token
                .as_ref()
                .and_then(|own| integrity(own.0))
                .zip(integrity(foreground.0))
                .map(|(own, foreground)| if foreground > own { "higher" } else { "same" }),
        };
        snapshot
    }
}
