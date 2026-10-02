//! Windows 集成。

pub mod monitor;
pub mod panel;
pub mod single_instance;

use windows::Win32::Foundation::{POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use crate::geometry::{Point, Rect};

/// 当前前台窗口的句柄（没有时为 0），日志和自测用。
pub fn foreground_window() -> isize {
    unsafe { GetForegroundWindow() }.0 as isize
}

fn rect_from_win32(rect: RECT) -> Rect {
    Rect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    }
}

fn point_from_win32(point: POINT) -> Point {
    Point {
        x: point.x,
        y: point.y,
    }
}
