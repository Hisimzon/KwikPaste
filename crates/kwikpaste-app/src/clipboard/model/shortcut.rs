//! 快捷键的显示文案，移植自 1.x `utils/shortcut.ts` 的 `formatShortcutDisplay`：按 `+` 拆开，
//! 每个键换成平台写法，再用 ` + ` 连起来（`CmdOrCtrl+Backspace` → Windows `Ctrl + ⌫`、macOS `⌘ + ⌫`）。

/// 删除选中记录的主快捷键：Windows 单按 Delete；macOS 笔记本没有 ⌦，沿用 ⌘⌫。
pub const DELETE_SELECTED: &str = if cfg!(target_os = "macos") {
    "CmdOrCtrl+Backspace"
} else {
    "Delete"
};

/// 当前平台的显示文案。
pub fn display(pattern: &str) -> String {
    display_for(pattern, cfg!(target_os = "macos"))
}

fn display_for(pattern: &str, mac: bool) -> String {
    pattern
        .split('+')
        .map(|key| key_display(key.trim(), mac))
        .collect::<Vec<_>>()
        .join(" + ")
}

/// 1.x `KEY_DISPLAY`；不在表里的单个字符转大写，其余原样。
fn key_display(key: &str, mac: bool) -> String {
    let pick = |macos: &str, other: &str| (if mac { macos } else { other }).to_owned();

    match key {
        "Alt" | "Option" => pick("⌥", "Alt"),
        "Cmd" | "CmdOrCtrl" | "CommandOrControl" | "Mod" => pick("⌘", "Ctrl"),
        "Command" | "Meta" | "Super" => pick("⌘", "Win"),
        "Control" | "Ctrl" => pick("⌃", "Ctrl"),
        "Shift" => pick("⇧", "Shift"),
        "Enter" | "Return" => pick("⏎", "Enter"),
        "Esc" | "Escape" => pick("⎋", "Esc"),
        "Delete" => pick("⌦", "Del"),
        "Space" => pick("␣", "Space"),
        "Tab" => pick("⇥", "Tab"),
        "Backspace" => "⌫".to_owned(),
        "ArrowDown" | "Down" => "↓".to_owned(),
        "ArrowLeft" | "Left" => "←".to_owned(),
        "ArrowRight" | "Right" => "→".to_owned(),
        "ArrowUp" | "Up" => "↑".to_owned(),
        _ => {
            let mut chars = key.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => c.to_uppercase().collect(),
                _ => key.to_owned(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_spells_modifiers_out() {
        assert_eq!(display_for("CmdOrCtrl+Backspace", false), "Ctrl + ⌫");
        assert_eq!(display_for("CmdOrCtrl+Enter", false), "Ctrl + Enter");
        assert_eq!(display_for("CmdOrCtrl+a", false), "Ctrl + A");
        assert_eq!(display_for("Enter", false), "Enter");
        assert_eq!(display_for("Escape", false), "Esc");
    }

    #[test]
    fn macos_uses_symbols() {
        assert_eq!(display_for("CmdOrCtrl+Backspace", true), "⌘ + ⌫");
        assert_eq!(display_for("CmdOrCtrl+Enter", true), "⌘ + ⏎");
        assert_eq!(display_for("Escape", true), "⎋");
    }
}
