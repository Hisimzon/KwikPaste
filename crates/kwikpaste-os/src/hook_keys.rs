//! Windows 面板非编辑态下由低级键盘钩子接管的按键：钩子吞键与 UI 的按键绑定都以这张表为准。
//!
//! 面板从不激活，系统不会把键盘消息发给它；面板可见时钩子在系统层把这些键截下来，
//! 转成 GPUI 按键派发给面板，目标应用收不到。表外的键（字母、Ctrl+V 等）原样交给目标应用。
//! 按住 Alt 或 Win 时一律放行（Alt+Tab、Win 组合键、AltGr 都不受影响）。
//!
//! 每项精确声明 Ctrl、Shift 状态；增删组合时 UI 的绑定一起改。
//! 表本身与平台无关，方便 UI 在任何平台的单测里核对绑定；只有 Windows 装钩子。

/// 被吞按键的派发方式，与允许的修饰键组合分开声明。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKeyKind {
    /// 按住时的自动重复照常派发（方向键连续移动）。
    Repeat,
    /// 按下和松开都派发，自动重复不派发（按住空格预览、松开关闭）。
    Hold,
}

/// 一个允许的精确修饰键状态；Alt、Win 由钩子统一放行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookModifiers {
    pub ctrl: bool,
    pub shift: bool,
}

const NONE: HookModifiers = HookModifiers {
    ctrl: false,
    shift: false,
};
const CTRL: HookModifiers = HookModifiers {
    ctrl: true,
    shift: false,
};
const SHIFT: HookModifiers = HookModifiers {
    ctrl: false,
    shift: true,
};

/// 一个被接管的键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookKey {
    /// Windows 虚拟键码；非美式布局下 Ctrl+字母仍按物理键位命中。
    pub vk: u16,
    /// GPUI `Keystroke` 的 key 名。
    pub key: &'static str,
    pub kind: HookKeyKind,
    /// Ctrl、Shift 必须与其中一个状态完全一致，才接管首次按下。
    pub modifiers: &'static [HookModifiers],
}

const fn key(
    vk: u16,
    key: &'static str,
    kind: HookKeyKind,
    modifiers: &'static [HookModifiers],
) -> HookKey {
    HookKey {
        vk,
        key,
        kind,
        modifiers,
    }
}

use HookKeyKind::{Hold, Repeat};

/// 全部被接管的键。
pub const HOOK_KEYS: &[HookKey] = &[
    key(0x25, "left", Repeat, &[NONE]),
    key(0x26, "up", Repeat, &[NONE]),
    key(0x27, "right", Repeat, &[NONE]),
    key(0x28, "down", Repeat, &[NONE]),
    key(0x0D, "enter", Repeat, &[NONE, CTRL]),
    key(0x1B, "escape", Repeat, &[NONE]),
    key(0x09, "tab", Repeat, &[NONE, SHIFT]),
    key(0x20, "space", Hold, &[NONE]),
    key(0x41, "a", Repeat, &[CTRL]),
    key(0x43, "c", Repeat, &[CTRL]),
    key(0x44, "d", Repeat, &[CTRL]),
    key(0x46, "f", Repeat, &[CTRL]),
    key(0x4B, "k", Repeat, &[CTRL]),
    key(0x4D, "m", Repeat, &[CTRL]),
    key(0x4E, "n", Repeat, &[CTRL]),
    key(0x4F, "o", Repeat, &[CTRL]),
    key(0x50, "p", Repeat, &[CTRL]),
    key(0x51, "q", Repeat, &[CTRL]),
    key(0x53, "s", Repeat, &[CTRL]),
    key(0x54, "t", Repeat, &[CTRL]),
    key(0xBC, ",", Repeat, &[CTRL]),
    key(0x30, "0", Repeat, &[CTRL]),
    key(0x31, "1", Repeat, &[CTRL]),
    key(0x32, "2", Repeat, &[CTRL]),
    key(0x33, "3", Repeat, &[CTRL]),
    key(0x34, "4", Repeat, &[CTRL]),
    key(0x35, "5", Repeat, &[CTRL]),
    key(0x36, "6", Repeat, &[CTRL]),
    key(0x37, "7", Repeat, &[CTRL]),
    key(0x38, "8", Repeat, &[CTRL]),
    key(0x39, "9", Repeat, &[CTRL]),
    key(0x08, "backspace", Repeat, &[CTRL]),
    key(0x2E, "delete", Repeat, &[NONE, CTRL]),
];

/// 查表：Ctrl、Shift 状态精确匹配时返回该接管的表项。
pub fn lookup(vk: u16, ctrl: bool, shift: bool) -> Option<&'static HookKey> {
    HOOK_KEYS
        .iter()
        .find(|entry| entry.vk == vk && entry.modifiers.contains(&HookModifiers { ctrl, shift }))
}

/// 钩子会派发给 GPUI 的按键组合，写成 GPUI keystroke 字符串（`up`、`shift-tab`、`ctrl-f`……），
/// 只列出表内精确允许的组合，供 UI 核对每个被吞的组合都有绑定。
pub fn keystrokes() -> Vec<String> {
    let mut keystrokes = Vec::new();
    for entry in HOOK_KEYS {
        for modifiers in entry.modifiers {
            let prefix = match (modifiers.ctrl, modifiers.shift) {
                (false, false) => "",
                (true, false) => "ctrl-",
                (false, true) => "shift-",
                (true, true) => "ctrl-shift-",
            };
            keystrokes.push(format!("{prefix}{}", entry.key));
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
    fn modifiers_must_match_exactly() {
        for (vk, ctrl, shift, swallowed) in [
            (0x41, true, true, false),
            (0x41, true, false, true),
            (0x41, false, false, false),
            (0x20, true, false, false),
            (0x20, false, true, false),
            (0x20, false, false, true),
            (0x1B, true, false, false),
            (0x1B, false, false, true),
            (0x2E, false, false, true),
            (0x2E, true, false, true),
            (0x08, false, false, false),
            (0x08, true, false, true),
            (0x09, false, true, true),
            (0x09, true, false, false),
            (0x0D, true, false, true),
            (0x0D, false, true, false),
            (0x28, true, false, false),
            (0x28, false, true, false),
            (0x56, true, false, false),
        ] {
            assert_eq!(
                lookup(vk, ctrl, shift).is_some(),
                swallowed,
                "vk 0x{vk:X}, ctrl={ctrl}, shift={shift}"
            );
        }
        assert_eq!(
            lookup(0x20, false, false).map(|entry| entry.kind),
            Some(Hold)
        );
    }

    #[test]
    fn keystrokes_are_gpui_strings() {
        let keystrokes = keystrokes();

        assert!(keystrokes.contains(&"ctrl-f".to_owned()));
        assert!(keystrokes.contains(&"shift-tab".to_owned()));
        assert!(keystrokes.contains(&"space".to_owned()));
        assert!(keystrokes.contains(&"delete".to_owned()));
        assert!(!keystrokes.contains(&"ctrl-shift-a".to_owned()));
        assert!(!keystrokes.contains(&"ctrl-space".to_owned()));
        assert!(!keystrokes.contains(&"shift-space".to_owned()));
        assert!(!keystrokes.contains(&"ctrl-escape".to_owned()));
        assert!(!keystrokes.contains(&"backspace".to_owned()));
        assert!(!keystrokes.contains(&"f".to_owned()));
    }

    #[test]
    fn keystrokes_list_every_allowed_modifier_set_once() {
        let listed = keystrokes();
        let mut expected = Vec::new();
        for entry in HOOK_KEYS {
            for (ctrl, shift, prefix) in [
                (false, false, ""),
                (true, false, "ctrl-"),
                (false, true, "shift-"),
                (true, true, "ctrl-shift-"),
            ] {
                if lookup(entry.vk, ctrl, shift).is_some() {
                    expected.push(format!("{prefix}{}", entry.key));
                }
            }
        }
        assert_eq!(listed, expected);
    }
}
