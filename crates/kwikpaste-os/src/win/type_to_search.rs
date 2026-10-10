//! 免焦点搜索的按键判定(Windows):面板不激活时把可打印键转成搜索框的字符。
//!
//! 与 [`crate::hook_keys`] 分开:那张表只列「UI 必须有对应绑定」的导航键,这张表按当前键盘布局
//! 求字符,不参与 `keystrokes()` 的绑定核对。
//!
//! - 排除 [`EXCLUDED`] 里的键:导航键、Enter/Esc/Tab、空格预览、Delete 删记录都要保持原语义。
//! - Backspace 不带修饰键时吞掉并派发「删掉最后一个字符」。
//! - 其余可打印键用 `ToUnicodeEx` 按布局求字符,只接受单个、非控制字符。
//! - 按住 Ctrl / Alt / Win 一律放行(AltGr 表现为 Ctrl+Alt,一并放行)。

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetKeyboardLayout, HKL, MAPVK_VK_TO_VSC, MapVirtualKeyExW, ToUnicodeEx,
    VIRTUAL_KEY, VK_CAPITAL, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};

/// 免焦点搜索不接管的键:留给现有语义(方向键/Enter/Esc/Tab 导航、空格按住预览、Delete 删记录)。
const EXCLUDED: [u16; 7] = [0x09, 0x0D, 0x1B, 0x20, 0x25, 0x26, 0x27];

/// 一次免焦点按键的判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeKey {
    /// 吞掉并追加这个字符。
    Char(char),
    /// 吞掉并删掉搜索框最后一个字符。
    Backspace,
}

/// 给按键事件判一次是否吞掉,以及要派发什么。
///
/// - `backspace` 且不带 Ctrl/Shift/Alt → [`TypeKey::Backspace`];
/// - 排除表里的键、按住 Ctrl/Alt/Win → `None`(放行);
/// - 其余可打印键按当前布局求字符,求不出单个非控制字符 → `None`。
pub fn classify(vk: u16, ctrl: bool, shift: bool, alt: bool, win: bool) -> Option<TypeKey> {
    if ctrl || alt || win {
        return None;
    }
    if EXCLUDED.contains(&vk) {
        return None;
    }
    if vk == 0x08 {
        return Some(TypeKey::Backspace);
    }

    printable_char(vk, shift).map(TypeKey::Char)
}

/// 按当前键盘布局求一个按键的字符;不是单个非控制字符时返回 `None`。
pub fn printable_char(vk: u16, shift: bool) -> Option<char> {
    let layout = foreground_layout();
    let scan = unsafe { MapVirtualKeyExW(u32::from(vk), MAPVK_VK_TO_VSC, Some(layout)) };
    if scan == 0 {
        return None;
    }

    // 钩子线程没有自己的输入队列,`GetKeyboardState` 拿到的是钩子线程的状态,不能用来求字符:
    // 手工按 Shift / CapsLock 构造键盘状态。
    let mut state = [0u8; 256];
    if shift {
        state[usize::from(VK_SHIFT.0)] = 0x80;
    }
    if unsafe { GetKeyState(i32::from(VK_CAPITAL.0)) & 1 } != 0 {
        state[usize::from(VK_CAPITAL.0)] = 0x01;
    }

    let mut buffer = [0u16; 4];
    let len = unsafe { ToUnicodeEx(u32::from(vk), scan, &state, &mut buffer, 0, Some(layout)) };
    if len < 0 {
        // 死键:再调一次同样参数清掉输入缓冲,这次只放行。
        unsafe { ToUnicodeEx(u32::from(vk), scan, &state, &mut buffer, 0, Some(layout)) };
        return None;
    }
    if len != 1 {
        return None;
    }

    let ch = char::from_u32(u32::from(buffer[0]))?;
    if ch.is_control() {
        return None;
    }
    Some(ch)
}

/// 当前前台窗口线程的键盘布局;取不到时用当前线程的布局。
fn foreground_layout() -> HKL {
    let foreground = unsafe { GetForegroundWindow() };
    let thread = if foreground == HWND(std::ptr::null_mut()) {
        0
    } else {
        unsafe { GetWindowThreadProcessId(foreground, None) }
    };
    unsafe { GetKeyboardLayout(thread) }
}

/// 当前是否按着 Ctrl / Alt / Win 中的一个(免焦点搜索要放行这些组合键)。
pub fn modifier_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(i32::from(vk.0)) < 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_and_preview_keys_are_left_alone() {
        for vk in [0x09, 0x0D, 0x1B, 0x20, 0x25, 0x26, 0x27] {
            assert_eq!(
                classify(vk, false, false, false, false),
                None,
                "vk 0x{vk:X}"
            );
        }
    }

    #[test]
    fn plain_backspace_deletes_the_last_character() {
        assert_eq!(
            classify(0x08, false, false, false, false),
            Some(TypeKey::Backspace)
        );
        // Ctrl+Backspace 是删记录,交给 HOOK_KEYS 处理。
        assert_eq!(classify(0x08, true, false, false, false), None);
    }

    #[test]
    fn modified_keys_are_left_alone() {
        for (ctrl, alt, win) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
        ] {
            assert_eq!(classify(0x41, ctrl, false, alt, win), None);
        }
    }
}
