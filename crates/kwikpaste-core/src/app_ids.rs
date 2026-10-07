//! 来源应用 id 的比较。Windows 的应用 id 是 exe 原样路径：商店应用装在带版本号的包目录里
//! （`WindowsApps\Claude_2.19675.1.0_x64__pzs8sxrjxfjjc\app\claude.exe`），Squirrel 安装的应用
//! 在 `app-1.2.3\` 下，每次升级都会多出一个应用 id。
//!
//! 已保存的 id（历史记录的 `source_app_id`、忽略列表、暂停列表）是发布过的数据契约，原样保留；
//! 判断「是不是同一个应用」一律按 [`app_family_key`] 比较，应用升级后新版本照样命中。

use std::collections::HashSet;

/// 来源应用的归并键：商店包目录和 Squirrel 的 `app-<版本>` 换成不含版本的形式，其余不区分大小写原样比较。
pub fn app_family_key(app_id: &str) -> String {
    let lower = app_id.to_lowercase();
    let mut previous = "";
    let segments: Vec<String> = lower
        .split('\\')
        .map(|segment| {
            let parts: Vec<&str> = segment.split('_').collect();
            let family = match parts.as_slice() {
                // 包全名是 名称_版本_架构_资源 id_发布者 id，去掉中间三段就是包系列名。
                [name, _, _, _, publisher] if previous == "windowsapps" => {
                    format!("{name}_{publisher}")
                }
                _ if segment.strip_prefix("app-").is_some_and(|version| {
                    version.contains('.') && version.chars().all(|c| c.is_ascii_digit() || c == '.')
                }) =>
                {
                    "app-*".to_owned()
                }
                _ => segment.to_owned(),
            };
            previous = segment;
            family
        })
        .collect();
    segments.join("\\")
}

/// 两个 id 是不是同一个应用（可能是不同的安装版本）。
pub fn same_app(left: &str, right: &str) -> bool {
    app_family_key(left) == app_family_key(right)
}

/// `app_ids` 里有没有 `app_id` 这个应用（任一安装版本）的 id。
pub fn contains_app(app_ids: &[String], app_id: &str) -> bool {
    let family = app_family_key(app_id);
    app_ids.iter().any(|id| app_family_key(id) == family)
}

/// 列表里有几个不同的应用；同一应用的多个版本算一个。
pub fn count_apps<'a>(app_ids: impl IntoIterator<Item = &'a str>) -> usize {
    app_ids
        .into_iter()
        .map(app_family_key)
        .collect::<HashSet<_>>()
        .len()
}

/// 在列表里勾选或取消一个应用。勾选时列表里还没有这个应用的任何版本才追加 `app_id`；
/// 取消时去掉它的全部版本，否则留下的旧版本 id 仍会命中。其余 id 原样保留。
pub fn set_app_listed(app_ids: &mut Vec<String>, app_id: &str, listed: bool) {
    let family = app_family_key(app_id);
    if !listed {
        app_ids.retain(|id| app_family_key(id) != family);
        return;
    }
    if !app_ids.iter().any(|id| app_family_key(id) == family) {
        app_ids.push(app_id.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_OLD: &str =
        r"C:\Program Files\WindowsApps\Claude_2.19675.0.0_x64__pzs8sxrjxfjjc\app\claude.exe";
    const CLAUDE_NEW: &str =
        r"C:\Program Files\WindowsApps\Claude_2.19675.1.0_x64__pzs8sxrjxfjjc\app\claude.exe";
    const CODEX: &str = r"C:\Program Files\WindowsApps\OpenAI.Codex_26.930.3930.0_x64__2p2nqsd0c76g0\app\ChatGPT.exe";
    const DISCORD_OLD: &str = r"C:\Users\a\AppData\Local\Discord\app-1.0.9163\Discord.exe";
    const DISCORD_NEW: &str = r"C:\Users\a\AppData\Local\Discord\app-1.0.9170\discord.exe";

    fn ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| (*id).to_owned()).collect()
    }

    #[test]
    fn app_family_key_drops_install_versions() {
        assert_eq!(app_family_key(CLAUDE_OLD), app_family_key(CLAUDE_NEW));
        assert_eq!(app_family_key(DISCORD_OLD), app_family_key(DISCORD_NEW));
        assert_ne!(
            app_family_key(CLAUDE_NEW),
            app_family_key(r"C:\Users\a\.local\bin\claude.exe")
        );
        assert_ne!(app_family_key(CLAUDE_NEW), app_family_key(CODEX));
        assert_eq!(
            app_family_key(r"C:\Tools\App-Store\run.exe"),
            r"c:\tools\app-store\run.exe"
        );
        assert_eq!(app_family_key("com.apple.Safari"), "com.apple.safari");
    }

    #[test]
    fn stored_ids_match_later_versions_of_the_same_app() {
        let listed = ids(&[CLAUDE_OLD, DISCORD_OLD]);

        assert!(contains_app(&listed, CLAUDE_NEW));
        assert!(contains_app(&listed, DISCORD_NEW));
        assert!(contains_app(&listed, &CLAUDE_OLD.to_uppercase()));
        assert!(!contains_app(&listed, CODEX));
        assert!(!contains_app(&listed, r"C:\Users\a\.local\bin\claude.exe"));
        assert!(!contains_app(&[], CLAUDE_NEW));
    }

    #[test]
    fn versions_of_one_app_count_once() {
        assert_eq!(
            count_apps([CLAUDE_OLD, CLAUDE_NEW, CODEX, DISCORD_OLD, DISCORD_NEW]),
            3
        );
        assert_eq!(count_apps([]), 0);
    }

    #[test]
    fn checking_an_app_keeps_stored_ids_and_adds_none_for_a_listed_family() {
        let mut listed = ids(&[CLAUDE_OLD, "com.apple.Passwords"]);

        set_app_listed(&mut listed, CLAUDE_NEW, true);
        assert_eq!(listed, ids(&[CLAUDE_OLD, "com.apple.Passwords"]));

        set_app_listed(&mut listed, CODEX, true);
        assert_eq!(listed, ids(&[CLAUDE_OLD, "com.apple.Passwords", CODEX]));
    }

    #[test]
    fn unchecking_an_app_removes_every_version() {
        let mut listed = ids(&[CLAUDE_OLD, CODEX, CLAUDE_NEW]);

        set_app_listed(&mut listed, CLAUDE_NEW, false);
        assert_eq!(listed, ids(&[CODEX]));

        set_app_listed(&mut listed, DISCORD_NEW, false);
        assert_eq!(listed, ids(&[CODEX]));
    }
}
