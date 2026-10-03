//! 把普通（可激活）窗口带到前台，例如偏好设置窗口。
//!
//! 托盘点击、全局热键之后本进程通常有前台权限，`SetForegroundWindow` 直接成功。被系统的前台锁
//! 拒绝时，按面板编辑态的做法注入一次带标记的 Alt、由自己的键盘钩子吞掉，再取前台：钩子没在运行
//! （面板隐藏）就临时装上、关掉导航，用完卸下。绝不注入会漏给目标应用的普通 Alt。

use std::ffi::c_void;
use std::time::Duration;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, ShowWindow};

use super::{keyboard, set_foreground};

/// 钩子确认吞掉带标记的 Alt 的最长等待（实测约 1.5 ms）。
const MARKED_ALT_TIMEOUT: Duration = Duration::from_millis(50);

/// 最小化了就先还原，再取前台。返回之后它是否是前台窗口。
pub fn bring_to_front(hwnd: isize) -> bool {
    let window = HWND(hwnd as *mut c_void);
    if unsafe { IsIconic(window) }.as_bool() {
        let _ = unsafe { ShowWindow(window, SW_RESTORE) };
    }
    if set_foreground(hwnd) {
        return true;
    }

    let started_here = !keyboard::is_running();
    if started_here {
        if let Err(err) = keyboard::start() {
            log::warn!("keyboard hook unavailable for taking the foreground: {err}");
            return false;
        }
        keyboard::set_navigation(false);
    }
    let taken = keyboard::swallow_marked_alt(MARKED_ALT_TIMEOUT) && set_foreground(hwnd);
    if started_here {
        keyboard::stop();
    }
    taken
}
