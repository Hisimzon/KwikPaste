//! 设置项的点分路径，与 `settings.json` 的 camelCase 键一一对应。
//!
//! 宿主收到 [`crate::CoreEvent::SettingsUpdated`] 后用 [`super::SettingsDelta::touches`] 判断要不要重排列表、
//! 重注册快捷键等；路径写成常量，界面各处共用，不在调用处手拼字符串。

/// 文本摘要最多显示行数（`Display::text_max_lines`）。
pub const TEXT_MAX_LINES: &str = "clipboard.display.textMaxLines";
/// 图片卡片显示高度上限（`Display::image_max_height`）。
pub const IMAGE_MAX_HEIGHT: &str = "clipboard.display.imageMaxHeight";
/// 列表样式：卡片或贴边（`Display::list_style`）。
pub const LIST_STYLE: &str = "clipboard.display.listStyle";
/// 列表疏密（`Display::density`）。
pub const DENSITY: &str = "clipboard.display.density";
/// 自定义疏密下的各项尺寸（`Display::custom_layout`）。
pub const CUSTOM_LAYOUT: &str = "clipboard.display.customLayout";
/// 文件记录最多显示的条目数（`Display::file_max_count`）。
pub const FILE_MAX_COUNT: &str = "clipboard.display.fileMaxCount";
/// 文本记录下方的快捷信息（`Display::quick_snippets`）。
pub const QUICK_SNIPPETS: &str = "clipboard.display.quickSnippets";
/// 打开剪贴板窗口时回到列表顶部（`Window::scroll_to_top_on_open`）。
pub const SCROLL_TO_TOP_ON_OPEN: &str = "clipboard.window.scrollToTopOnOpen";
/// 点击列表项时的自动粘贴行为（`Content::auto_paste`）。
pub const AUTO_PASTE: &str = "clipboard.content.autoPaste";
/// 历史列表默认排序（`Content::sort`）。
pub const SORT: &str = "clipboard.content.sort";

/// 改了就要重新排版列表的设置。
pub const LIST_LAYOUT: [&str; 7] = [
    TEXT_MAX_LINES,
    IMAGE_MAX_HEIGHT,
    LIST_STYLE,
    DENSITY,
    CUSTOM_LAYOUT,
    FILE_MAX_COUNT,
    QUICK_SNIPPETS,
];

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::settings::{Settings, SettingsDelta};

    const ALL: [&str; 10] = [
        TEXT_MAX_LINES,
        IMAGE_MAX_HEIGHT,
        LIST_STYLE,
        DENSITY,
        CUSTOM_LAYOUT,
        FILE_MAX_COUNT,
        QUICK_SNIPPETS,
        SCROLL_TO_TOP_ON_OPEN,
        AUTO_PASTE,
        SORT,
    ];

    fn pointer(path: &str) -> String {
        format!("/{}", path.replace('.', "/"))
    }

    /// 按路径建一个只改这一项的 patch，值取默认设置里的原值。
    fn patch_for(path: &str, value: Value) -> Value {
        path.rsplit('.')
            .fold(value, |inner, key| json!({ key: inner }))
    }

    /// 每个路径都对应设置文件里真实存在的键，改到它时 delta 能判断出来，改别处时不会误判。
    #[test]
    fn every_path_names_a_real_setting_and_is_seen_by_the_delta() {
        let defaults = serde_json::to_value(Settings::default()).unwrap();

        for path in ALL {
            let value = defaults
                .pointer(&pointer(path))
                .unwrap_or_else(|| panic!("{path} is not a settings key"))
                .clone();
            let delta = SettingsDelta::from_patch(&patch_for(path, value));

            assert!(delta.touches(path), "{path}");
            for other in ALL.iter().filter(|other| **other != path) {
                assert!(!delta.touches(other), "{path} should not touch {other}");
            }
        }
    }

    #[test]
    fn replacing_the_display_section_touches_every_layout_setting() {
        let delta = SettingsDelta::from_patch(&json!({"clipboard": {"display": null}}));

        for path in LIST_LAYOUT {
            assert!(delta.touches(path), "{path}");
        }
        assert!(!delta.touches(AUTO_PASTE));
        assert!(SettingsDelta::replaced().touches(SCROLL_TO_TOP_ON_OPEN));
    }
}
