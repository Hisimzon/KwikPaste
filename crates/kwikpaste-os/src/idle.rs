//! 当前登录会话距最后一次键盘或鼠标输入的时间；查询失败时由调用方推迟更新。

use std::time::Duration;

/// Windows 的输入时间戳与 GetTickCount 都是 32 位毫秒值，差值按回绕计算。
#[cfg(target_os = "windows")]
pub fn duration() -> anyhow::Result<Duration> {
    use windows::Win32::System::SystemInformation::GetTickCount;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

    let mut input = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    // SAFETY: 结构体已初始化，大小正确，调用期间指针有效。
    if !unsafe { GetLastInputInfo(&mut input) }.as_bool() {
        anyhow::bail!(
            "GetLastInputInfo failed: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(Duration::from_millis(u64::from(
        unsafe { GetTickCount() }.wrapping_sub(input.dwTime),
    )))
}

/// macOS 合并会话状态的任意输入事件；不安装事件监听器，也不需要辅助功能权限。
#[cfg(target_os = "macos")]
pub fn duration() -> anyhow::Result<Duration> {
    use objc2_core_graphics::{CGEventSource, CGEventSourceStateID, CGEventType};

    // kCGAnyInputEventType = ~0；绑定未提供这个宏常量。
    let seconds = CGEventSource::seconds_since_last_event_type(
        CGEventSourceStateID::CombinedSessionState,
        CGEventType(u32::MAX),
    );
    Duration::try_from_secs_f64(seconds).map_err(anyhow::Error::from)
}
