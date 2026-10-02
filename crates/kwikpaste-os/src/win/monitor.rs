//! 显示器几何与系统缩放设置。
//!
//! 坐标是物理像素：应用的清单声明了 PerMonitorV2（GPUI 默认嵌入）。不读 GPUI 的显示器缓存，
//! 热插拔、改分辨率后它会过期。

use std::io;

use windows::Win32::Foundation::{LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST,
    MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windows::core::BOOL;

use super::{point_from_win32, rect_from_win32};
use crate::geometry::{Point, Rect};

/// Windows 的基准 DPI（100% 缩放）。
pub const BASE_DPI: u32 = 96;

/// 一块显示器的几何信息。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MonitorInfo {
    /// 取信息时的光标位置。
    pub cursor: Point,
    pub monitor: Rect,
    /// 去掉任务栏等桌面工具栏后的工作区。
    pub work_area: Rect,
    pub dpi: u32,
}

impl MonitorInfo {
    /// DPI 缩放比例，96 DPI 为 1.0。
    pub fn scale(&self) -> f64 {
        f64::from(self.dpi) / f64::from(BASE_DPI)
    }
}

/// 光标所在（或离光标最近）的显示器。
///
/// 进程不在输入桌面上（锁屏、远程会话断开、CI 服务会话）时取不到光标，返回错误，
/// 调用方改用 [`primary`]。
pub fn at_cursor() -> io::Result<MonitorInfo> {
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor) }.map_err(io::Error::other)?;

    let monitor = unsafe { MonitorFromPoint(cursor, MONITOR_DEFAULTTONEAREST) };
    describe(monitor, cursor)
}

/// 主显示器；`cursor` 取工作区中心。
pub fn primary() -> io::Result<MonitorInfo> {
    let monitor = unsafe { MonitorFromPoint(POINT::default(), MONITOR_DEFAULTTOPRIMARY) };
    let info = describe(monitor, POINT::default())?;
    let cursor = Point {
        x: info.work_area.left + info.work_area.width() / 2,
        y: info.work_area.top + info.work_area.height() / 2,
    };

    Ok(MonitorInfo { cursor, ..info })
}

/// 全部显示器；`cursor` 取各自工作区中心。顺序是系统枚举的顺序。
pub fn all() -> Vec<MonitorInfo> {
    unsafe extern "system" fn collect(
        monitor: HMONITOR,
        _: HDC,
        _: *mut RECT,
        data: LPARAM,
    ) -> BOOL {
        let monitors = unsafe { &mut *(data.0 as *mut Vec<HMONITOR>) };
        monitors.push(monitor);
        true.into()
    }

    let mut handles = Vec::<HMONITOR>::new();
    let enumerated = unsafe {
        EnumDisplayMonitors(None, None, Some(collect), LPARAM(&raw mut handles as isize))
    };
    if !enumerated.as_bool() {
        log::warn!("display monitors could not be enumerated");
    }

    handles
        .into_iter()
        .filter_map(|handle| {
            let info = describe(handle, POINT::default()).ok()?;
            let cursor = Point {
                x: info.work_area.left + info.work_area.width() / 2,
                y: info.work_area.top + info.work_area.height() / 2,
            };
            Some(MonitorInfo { cursor, ..info })
        })
        .collect()
}

fn describe(monitor: HMONITOR, cursor: POINT) -> io::Result<MonitorInfo> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return Err(io::Error::last_os_error());
    }

    let (mut dpi_x, mut dpi_y) = (0, 0);
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) }
        .map_err(io::Error::other)?;

    Ok(MonitorInfo {
        cursor: point_from_win32(cursor),
        monitor: rect_from_win32(info.rcMonitor),
        work_area: rect_from_win32(info.rcWork),
        dpi: dpi_x.max(1),
    })
}

/// 读取「设置 → 辅助功能 → 文本大小」（100%–225%），没设置过时是 1.0。
pub fn text_scale_factor() -> f64 {
    windows_registry::CURRENT_USER
        .open(r"Software\Microsoft\Accessibility")
        .and_then(|key| key.get_u32("TextScaleFactor"))
        .map(|percent| (f64::from(percent) / 100.0).clamp(1.0, 2.25))
        .unwrap_or(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_sane(info: MonitorInfo) {
        assert!(info.dpi >= BASE_DPI / 2, "dpi {}", info.dpi);
        assert!(info.work_area.width() > 0 && info.work_area.height() > 0);
        assert!(info.work_area.left >= info.monitor.left);
        assert!(info.work_area.right <= info.monitor.right);
    }

    #[test]
    fn primary_monitor_puts_the_cursor_in_its_work_area() {
        let info = primary().expect("a primary monitor");

        assert_sane(info);
        assert!(info.cursor.x >= info.work_area.left && info.cursor.x < info.work_area.right);
        assert!(info.cursor.y >= info.work_area.top && info.cursor.y < info.work_area.bottom);
    }

    #[test]
    fn cursor_monitor_is_sane_when_the_cursor_is_available() {
        // 不在输入桌面上（CI 服务会话、锁屏）时取不到光标，那时由 primary() 兜底。
        if let Ok(info) = at_cursor() {
            assert_sane(info);
        }
    }

    #[test]
    fn all_monitors_include_the_primary_one() {
        let monitors = all();
        let primary = primary().expect("a primary monitor");

        assert!(
            monitors
                .iter()
                .any(|monitor| monitor.monitor == primary.monitor)
        );
        monitors.into_iter().for_each(assert_sane);
    }

    #[test]
    fn text_scale_factor_is_clamped() {
        let factor = text_scale_factor();

        assert!((1.0..=2.25).contains(&factor), "factor {factor}");
    }
}
