//! 反馈用的只读系统探针；各字段独立失败，不采集窗口标题或完整进程路径。

#[derive(Default)]
pub struct Snapshot {
    pub os_version: Option<String>,
    /// 带版本号的系统名，如 `Windows 11 Pro 24H2`；macOS 只有版本号，保留 null。
    pub os_name: Option<String>,
    /// 整机型号：Windows 取 BIOS 厂商 + 产品名，macOS 取 `hw.model`。
    pub model: Option<String>,
    pub cpu: Option<String>,
    pub cpu_threads: Option<usize>,
    pub memory_bytes: Option<u64>,
    /// 显示适配器名称（去重）；macOS 保留 null。
    pub gpus: Option<Vec<String>>,
    pub elevated: Option<bool>,
    pub display_scales: Option<Vec<Option<f64>>>,
    pub foreground_exe: Option<String>,
    pub foreground_privilege: Option<&'static str>,
}

#[cfg(target_os = "macos")]
fn command(program: &str, args: &[&str]) -> Option<String> {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
}

/// macOS 不适用的权限探针保留 null；其余平台信息由宿主补齐。
#[cfg(target_os = "macos")]
pub fn snapshot() -> Snapshot {
    let sysctl = |name| command("/usr/sbin/sysctl", &["-n", name]);
    Snapshot {
        os_version: command("/usr/bin/sw_vers", &["-productVersion"]),
        model: sysctl("hw.model"),
        cpu: sysctl("machdep.cpu.brand_string"),
        cpu_threads: std::thread::available_parallelism().ok().map(usize::from),
        memory_bytes: sysctl("hw.memsize").and_then(|value| value.parse().ok()),
        ..Snapshot::default()
    }
}

#[cfg(target_os = "windows")]
pub use windows_probes::snapshot;

#[cfg(target_os = "windows")]
mod windows_probes {
    use super::Snapshot;
    use windows::Win32::Foundation::{CloseHandle, HANDLE, LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{
        DISPLAY_DEVICE_MIRRORING_DRIVER, DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors,
        HDC, HMONITOR,
    };
    use windows::Win32::Security::{
        GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_ELEVATION,
        TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenElevation, TokenIntegrityLevel,
    };
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    use windows::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    use windows::core::{BOOL, PCWSTR, PWSTR};

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

    fn registry(path: &str) -> Option<windows_registry::Key> {
        windows_registry::LOCAL_MACHINE.open(path).ok()
    }

    fn text(key: &windows_registry::Key, name: &str) -> Option<String> {
        key.get_string(name)
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    }

    fn os(key: &windows_registry::Key) -> (Option<String>, Option<String>) {
        let build = key.get_string("CurrentBuildNumber").ok();
        let version = (|| {
            let major = key.get_u32("CurrentMajorVersionNumber").ok()?;
            let minor = key.get_u32("CurrentMinorVersionNumber").ok()?;
            let build = build.as_deref()?;
            Some(match key.get_u32("UBR") {
                Ok(ubr) => format!("{major}.{minor}.{build}.{ubr}"),
                Err(_) => format!("{major}.{minor}.{build}"),
            })
        })();
        let name = text(key, "ProductName").map(|name| {
            // Windows 11 的 ProductName 仍写作 "Windows 10 …"，按内部版本号改正。
            let name = if build
                .as_deref()
                .and_then(|build| build.parse::<u32>().ok())
                .is_some_and(|build| build >= 22000)
            {
                name.replacen("Windows 10", "Windows 11", 1)
            } else {
                name
            };
            match text(key, "DisplayVersion") {
                Some(display) => format!("{name} {display}"),
                None => name,
            }
        });
        (version, name)
    }

    fn memory_bytes() -> Option<u64> {
        let mut status = MEMORYSTATUSEX {
            dwLength: size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        unsafe { GlobalMemoryStatusEx(&mut status) }.ok()?;
        Some(status.ullTotalPhys)
    }

    fn gpus() -> Option<Vec<String>> {
        let mut names = Vec::<String>::new();
        for index in 0.. {
            let mut device = DISPLAY_DEVICEW {
                cb: size_of::<DISPLAY_DEVICEW>() as u32,
                ..Default::default()
            };
            if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), index, &mut device, 0) }.as_bool() {
                break;
            }
            if device.StateFlags.contains(DISPLAY_DEVICE_MIRRORING_DRIVER) {
                continue;
            }
            let end = device
                .DeviceString
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(device.DeviceString.len());
            let name = String::from_utf16_lossy(&device.DeviceString[..end])
                .trim()
                .to_owned();
            if !name.is_empty() && !names.contains(&name) {
                names.push(name);
            }
        }
        (!names.is_empty()).then_some(names)
    }

    pub fn snapshot() -> Snapshot {
        let (os_version, os_name) = registry(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
            .map_or((None, None), |key| os(&key));
        let model = registry(r"HARDWARE\DESCRIPTION\System\BIOS").and_then(|key| {
            let parts: Vec<_> = ["SystemManufacturer", "SystemProductName"]
                .into_iter()
                .filter_map(|name| text(&key, name))
                .collect();
            (!parts.is_empty()).then(|| parts.join(" "))
        });
        let own_token = token(unsafe { GetCurrentProcess() });
        let mut snapshot = Snapshot {
            os_version,
            os_name,
            model,
            cpu: registry(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
                .and_then(|key| text(&key, "ProcessorNameString")),
            cpu_threads: std::thread::available_parallelism().ok().map(usize::from),
            memory_bytes: memory_bytes(),
            gpus: gpus(),
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
