//! Windows 非激活面板的精确吞键表，由 core 有效键位生成。
//! Alt、Win 一律由钩子放行；表外组合继续交给目标应用。

use kwikpaste_core::settings::{PanelAction, Shortcuts, effective_panel_keys};

/// 被吞按键的派发方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKeyKind {
    /// 方向键等按住时持续派发。
    Repeat,
    /// 空格只派发按下、松开，不派发重复。
    Hold,
}

/// Alt、Win 放行之后还须精确匹配的修饰键。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookModifiers {
    pub ctrl: bool,
    pub shift: bool,
}

/// 一条可派发的物理组合键；键名来自固定映射，不引用可替换表的内存。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HookKey {
    pub vk: u16,
    pub key: &'static str,
    pub kind: HookKeyKind,
    pub modifiers: HookModifiers,
}

/// Windows 虚拟键与 GPUI 键名的唯一映射；OEM 标点按美式物理键位表示。
const KEY_NAMES: &[(u16, &str)] = &[
    (0x25, "left"),
    (0x26, "up"),
    (0x27, "right"),
    (0x28, "down"),
    (0x0D, "enter"),
    (0x1B, "escape"),
    (0x09, "tab"),
    (0x20, "space"),
    (0x08, "backspace"),
    (0x2E, "delete"),
    (0x41, "a"),
    (0x42, "b"),
    (0x43, "c"),
    (0x44, "d"),
    (0x45, "e"),
    (0x46, "f"),
    (0x47, "g"),
    (0x48, "h"),
    (0x49, "i"),
    (0x4A, "j"),
    (0x4B, "k"),
    (0x4C, "l"),
    (0x4D, "m"),
    (0x4E, "n"),
    (0x4F, "o"),
    (0x50, "p"),
    (0x51, "q"),
    (0x52, "r"),
    (0x53, "s"),
    (0x54, "t"),
    (0x55, "u"),
    (0x56, "v"),
    (0x57, "w"),
    (0x58, "x"),
    (0x59, "y"),
    (0x5A, "z"),
    (0x30, "0"),
    (0x31, "1"),
    (0x32, "2"),
    (0x33, "3"),
    (0x34, "4"),
    (0x35, "5"),
    (0x36, "6"),
    (0x37, "7"),
    (0x38, "8"),
    (0x39, "9"),
    (0x70, "f1"),
    (0x71, "f2"),
    (0x72, "f3"),
    (0x73, "f4"),
    (0x74, "f5"),
    (0x75, "f6"),
    (0x76, "f7"),
    (0x77, "f8"),
    (0x78, "f9"),
    (0x79, "f10"),
    (0x7A, "f11"),
    (0x7B, "f12"),
    (0x7C, "f13"),
    (0x7D, "f14"),
    (0x7E, "f15"),
    (0x7F, "f16"),
    (0x80, "f17"),
    (0x81, "f18"),
    (0x82, "f19"),
    (0x83, "f20"),
    (0x84, "f21"),
    (0x85, "f22"),
    (0x86, "f23"),
    (0x87, "f24"),
    (0xBC, ","),
    (0xBE, "."),
    (0xBF, "/"),
    (0xBA, ";"),
    (0xDE, "'"),
    (0xDB, "["),
    (0xDD, "]"),
    (0xBD, "-"),
    (0xBB, "="),
    (0xC0, "`"),
    (0xDC, "\\"),
];

/// 不变的物理键信息；已吞键即使中途解绑，重复与松开仍须吞掉。
pub fn physical_key(vk: u16) -> Option<HookKey> {
    KEY_NAMES
        .iter()
        .find(|(code, _)| *code == vk)
        .map(|(_, key)| HookKey {
            vk,
            key,
            kind: if *key == "space" {
                HookKeyKind::Hold
            } else {
                HookKeyKind::Repeat
            },
            modifiers: HookModifiers {
                ctrl: false,
                shift: false,
            },
        })
}

/// 一次设置快照对应的不可变吞键表。
#[derive(Debug, Default)]
pub struct HookKeys(Vec<HookKey>);

impl HookKeys {
    /// 与 GPUI 共用 core 的有效键位；固定动作也从同一动作表读取。
    pub fn from_shortcuts(shortcuts: &Shortcuts) -> Self {
        let mut entries = Vec::new();
        for action in PanelAction::ALL {
            for combo in effective_panel_keys(shortcuts, action) {
                let (ctrl, key) = combo
                    .strip_prefix("secondary-")
                    .map_or((false, combo.as_str()), |key| (true, key));
                let (shift, key) = key
                    .strip_prefix("shift-")
                    .map_or((false, key), |key| (true, key));
                let Some((vk, _)) = KEY_NAMES.iter().find(|(_, name)| *name == key) else {
                    log::warn!("unmapped panel keystroke: {combo}");
                    continue;
                };
                if let Some(mut entry) = physical_key(*vk) {
                    entry.modifiers = HookModifiers { ctrl, shift };
                    if !entries.contains(&entry) {
                        entries.push(entry);
                    }
                }
            }
        }
        Self(entries)
    }

    /// 首次按下必须精确匹配 Ctrl、Shift，不把表外按键吞掉。
    pub fn lookup(&self, vk: u16, ctrl: bool, shift: bool) -> Option<HookKey> {
        self.0
            .iter()
            .find(|entry| entry.vk == vk && entry.modifiers == HookModifiers { ctrl, shift })
            .copied()
    }

    /// 与钩子查表同源，供 UI 验证每个被吞的组合都有精确绑定。
    pub fn keystrokes(&self) -> Vec<String> {
        self.0
            .iter()
            .map(|entry| {
                format!(
                    "{}{}{}",
                    if entry.modifiers.ctrl { "ctrl-" } else { "" },
                    if entry.modifiers.shift { "shift-" } else { "" },
                    entry.key
                )
            })
            .collect()
    }
}

#[cfg(target_os = "windows")]
static CURRENT: arc_swap::ArcSwapOption<HookKeys> = arc_swap::ArcSwapOption::const_empty();

/// 在 UI 线程构造并发布快照；回调只做无锁读取，不初始化、不等待写入者。
#[cfg(target_os = "windows")]
pub fn apply(shortcuts: &Shortcuts) {
    CURRENT.store(Some(std::sync::Arc::new(HookKeys::from_shortcuts(
        shortcuts,
    ))));
}

/// 钩子取当前快照的一条记录；返回值不借用快照，更新不影响已派发事件。
#[cfg(target_os = "windows")]
pub fn lookup(vk: u16, ctrl: bool, shift: bool) -> Option<HookKey> {
    CURRENT
        .load()
        .as_ref()
        .and_then(|keys| keys.lookup(vk, ctrl, shift))
}

/// 诊断读取当前运行表。
#[cfg(target_os = "windows")]
pub fn keystrokes() -> Vec<String> {
    CURRENT
        .load()
        .as_ref()
        .map(|keys| keys.keystrokes())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_exact_modifiers_and_keep_space_hold() {
        let keys = HookKeys::from_shortcuts(&Shortcuts::default());
        assert!(keys.lookup(0x41, true, false).is_some());
        assert!(keys.lookup(0x41, true, true).is_none());
        assert!(keys.lookup(0x08, false, false).is_none());
        assert!(keys.lookup(0x2E, false, false).is_some());
        assert_eq!(
            keys.lookup(0x20, false, false).map(|key| key.kind),
            Some(HookKeyKind::Hold)
        );
        assert!(keys.lookup(0x20, true, false).is_none());
        assert!(keys.keystrokes().contains(&"shift-tab".into()));
    }

    #[test]
    fn override_removes_old_key_and_unbinding_removes_capture() {
        let mut shortcuts = Shortcuts::default();
        shortcuts
            .panel
            .insert("favoriteSelected".into(), "secondary-shift-b".into());
        shortcuts.panel.insert("copySelected".into(), String::new());
        let keys = HookKeys::from_shortcuts(&shortcuts);
        assert!(keys.lookup(0x44, true, false).is_none());
        assert!(keys.lookup(0x43, true, false).is_none());
        assert!(keys.lookup(0x42, true, false).is_none());
        assert!(keys.lookup(0x42, true, true).is_some());
        assert!(keys.keystrokes().contains(&"ctrl-shift-b".into()));
    }

    #[test]
    fn supported_keys_have_unique_physical_mappings() {
        for (index, (vk, key)) in KEY_NAMES.iter().enumerate() {
            assert!(
                KEY_NAMES[index + 1..]
                    .iter()
                    .all(|(other_vk, other_key)| vk != other_vk && key != other_key)
            );
            if kwikpaste_core::settings::panel_keystroke_allowed(&format!("secondary-{key}")) {
                let mut shortcuts = Shortcuts::default();
                shortcuts
                    .panel
                    .insert("favoriteSelected".into(), format!("secondary-{key}"));
                assert!(
                    HookKeys::from_shortcuts(&shortcuts)
                        .lookup(*vk, true, false)
                        .is_some()
                );
            }
        }
    }
}
