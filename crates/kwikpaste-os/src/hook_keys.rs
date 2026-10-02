//! Windows 面板非编辑态下由低级键盘钩子接管的按键：钩子吞键与 UI 的按键绑定都以这张表为准。
//!
//! 面板从不激活，系统不会把键盘消息发给它；面板可见时钩子在系统层把这些键截下来，
//! 转成 GPUI 按键派发给面板，目标应用收不到。表外的键（字母、Ctrl+V 等）原样交给目标应用。
//! 按住 Alt 或 Win 时一律放行（Alt+Tab、Win 组合键、AltGr 都不受影响）。
//!
//! 与 1.x `keyboard/windows.rs` 的按键集合相同；增删按键时 UI 的绑定一起改。
//! 表本身与平台无关，方便 UI 在任何平台的单测里核对绑定；只有 Windows 装钩子。

/// 什么情况下吞下这个键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKeyKind {
    /// 有无 Ctrl、Shift 都吞；按住时的自动重复照常派发（方向键连续移动）。
    Plain,
    /// 只在按住 Ctrl 时吞。
    Ctrl,
    /// 有无 Ctrl、Shift 都吞；按下和松开都派发，自动重复不派发（按住空格预览、松开关闭）。
    Hold,
}

/// 一个被接管的键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookKey {
    /// Windows 虚拟键码；非美式布局下 Ctrl+字母仍按物理键位命中。
    pub vk: u16,
    /// GPUI `Keystroke` 的 key 名。
    pub key: &'static str,
    pub kind: HookKeyKind,
}

const fn key(vk: u16, key: &'static str, kind: HookKeyKind) -> HookKey {
    HookKey { vk, key, kind }
}

use HookKeyKind::{Ctrl, Hold, Plain};

/// 全部被接管的键。
pub const HOOK_KEYS: &[HookKey] = &[
    key(0x25, "left", Plain),
    key(0x26, "up", Plain),
    key(0x27, "right", Plain),
    key(0x28, "down", Plain),
    key(0x0D, "enter", Plain),
    key(0x1B, "escape", Plain),
    key(0x09, "tab", Plain),
    key(0x20, "space", Hold),
    key(0x41, "a", Ctrl),
    key(0x43, "c", Ctrl),
    key(0x44, "d", Ctrl),
    key(0x46, "f", Ctrl),
    key(0x4B, "k", Ctrl),
    key(0x4D, "m", Ctrl),
    key(0x4E, "n", Ctrl),
    key(0x4F, "o", Ctrl),
    key(0x50, "p", Ctrl),
    key(0x51, "q", Ctrl),
    key(0x53, "s", Ctrl),
    key(0x54, "t", Ctrl),
    key(0xBC, ",", Ctrl),
    key(0x30, "0", Ctrl),
    key(0x31, "1", Ctrl),
    key(0x32, "2", Ctrl),
    key(0x33, "3", Ctrl),
    key(0x34, "4", Ctrl),
    key(0x35, "5", Ctrl),
    key(0x36, "6", Ctrl),
    key(0x37, "7", Ctrl),
    key(0x38, "8", Ctrl),
    key(0x39, "9", Ctrl),
    key(0x08, "backspace", Ctrl),
    key(0x2E, "delete", Ctrl),
];

/// 查表：按下 `vk` 时（`ctrl` 表示 Ctrl 是否按着）是否该吞，返回对应的表项。
pub fn lookup(vk: u16, ctrl: bool) -> Option<&'static HookKey> {
    HOOK_KEYS
        .iter()
        .find(|entry| entry.vk == vk && (entry.kind != Ctrl || ctrl))
}

/// 钩子会派发给 GPUI 的按键组合，写成 GPUI keystroke 字符串（`up`、`shift-tab`、`ctrl-f`……），
/// 供 UI 核对每个被吞的组合都有绑定。Plain / Hold 键另列出带 Ctrl、Shift 的组合。
pub fn keystrokes() -> Vec<String> {
    let mut keystrokes = Vec::new();
    for entry in HOOK_KEYS {
        match entry.kind {
            Plain | Hold => {
                for prefix in ["", "shift-", "ctrl-", "ctrl-shift-"] {
                    keystrokes.push(format!("{prefix}{}", entry.key));
                }
            }
            Ctrl => {
                keystrokes.push(format!("ctrl-{}", entry.key));
                keystrokes.push(format!("ctrl-shift-{}", entry.key));
            }
        }
    }

    keystrokes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_virtual_key_appears_once() {
        for (index, entry) in HOOK_KEYS.iter().enumerate() {
            assert!(
                HOOK_KEYS[index + 1..]
                    .iter()
                    .all(|other| other.vk != entry.vk),
                "vk 0x{:X} listed twice",
                entry.vk
            );
        }
    }

    #[test]
    fn ctrl_keys_need_ctrl_and_the_rest_do_not() {
        assert_eq!(lookup(0x46, false), None);
        assert_eq!(lookup(0x46, true).map(|entry| entry.key), Some("f"));
        assert_eq!(lookup(0x28, false).map(|entry| entry.key), Some("down"));
        assert_eq!(lookup(0x28, true).map(|entry| entry.key), Some("down"));
        assert_eq!(lookup(0x20, false).map(|entry| entry.kind), Some(Hold));
        // Ctrl+V 交给目标应用。
        assert_eq!(lookup(0x56, true), None);
    }

    #[test]
    fn keystrokes_are_gpui_strings() {
        let keystrokes = keystrokes();

        assert!(keystrokes.contains(&"ctrl-f".to_owned()));
        assert!(keystrokes.contains(&"shift-tab".to_owned()));
        assert!(keystrokes.contains(&"space".to_owned()));
        assert!(!keystrokes.contains(&"f".to_owned()));
    }
}
