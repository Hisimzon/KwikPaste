//! 设置值的读取与补丁：按点分路径在 `settings.json` 形状的 JSON 里取值，把控件的新值做成
//! `Core::update_settings` 吃的深层补丁（1.x `services/preferenceSettings.ts`）。

use kwikpaste_core::settings::{CustomListLayout, ListDensity, Settings};
use serde_json::{Map, Value, json};

/// 采集类型：勾选框的显示顺序，与 `clipboard.capture` 的五个开关同名。
pub const CAPTURE_KINDS: [&str; 5] = ["text", "html", "rtf", "image", "files"];

/// 设置的 JSON 形状（camelCase，与 `settings.json` 相同）。
pub fn to_json(settings: &Settings) -> Value {
    serde_json::to_value(settings).unwrap_or(Value::Null)
}

/// 按点分路径取值。
pub fn get<'a>(json: &'a Value, path: &str) -> Option<&'a Value> {
    path.split('.').try_fold(json, |node, key| node.get(key))
}

pub fn get_bool(json: &Value, path: &str) -> bool {
    get(json, path).and_then(Value::as_bool).unwrap_or(false)
}

pub fn get_u64(json: &Value, path: &str) -> u64 {
    get(json, path).and_then(Value::as_u64).unwrap_or(0)
}

/// 下拉的取值统一成字符串：枚举本来就是字符串，数字选项（列表间距）转成十进制。
pub fn get_choice(json: &Value, path: &str) -> Option<String> {
    match get(json, path)? {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

/// 只改 `path` 这一项的补丁。
pub fn patch(path: &str, value: Value) -> Value {
    path.rsplit('.')
        .fold(value, |inner, key| json!({ key: inner }))
}

/// 下拉选定后的补丁：数字选项写回数字。
pub fn choice_patch(path: &str, value: &str, numeric: bool) -> Value {
    let value = if numeric {
        value
            .parse::<u64>()
            .map_or_else(|_| Value::String(value.to_owned()), Value::from)
    } else {
        Value::String(value.to_owned())
    };
    patch(path, value)
}

/// 深度合并两个补丁；数组按设置语义整体替换。
pub fn merge(left: Value, right: Value) -> Value {
    match (left, right) {
        (Value::Object(mut left), Value::Object(right)) => {
            for (key, value) in right {
                let merged = match left.remove(&key) {
                    Some(previous) => merge(previous, value),
                    None => value,
                };
                left.insert(key, merged);
            }
            Value::Object(left)
        }
        (_, right) => right,
    }
}

/// 勾选的采集类型展开成 `clipboard.capture.<kind>` 五个开关。
pub fn capture_kinds_patch(selected: &[&str]) -> Value {
    let kinds: Map<String, Value> = CAPTURE_KINDS
        .iter()
        .map(|kind| ((*kind).to_owned(), Value::Bool(selected.contains(kind))))
        .collect();
    json!({ "clipboard": { "capture": Value::Object(kinds) } })
}

/// 三档预设换算成自定义尺寸（舒适 / 标准 / 紧凑）。
fn preset_layout(density: ListDensity) -> Option<CustomListLayout> {
    let (header_row, item_gap, padding_y) = match density {
        ListDensity::Comfortable => (true, 12, 8),
        ListDensity::Standard => (true, 8, 6),
        ListDensity::Compact => (false, 4, 4),
        ListDensity::Custom => return None,
    };

    Some(CustomListLayout {
        header_row,
        item_gap,
        padding_y,
    })
}

/// 从预设档位切到「自定义」：没调过的自定义尺寸从刚才的档位起步，界面不会先跳回默认值；
/// 调过的原样保留。
pub fn custom_density_seed(settings: &Settings) -> Option<Value> {
    let display = &settings.clipboard.display;
    let untouched = display.custom_layout == CustomListLayout::default();
    let preset = preset_layout(display.density).filter(|_| untouched)?;

    Some(json!({ "clipboard": { "display": { "customLayout": preset } } }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_read_by_path() {
        let json = to_json(&Settings::default());
        assert!(get_bool(&json, "general.trayIcon"));
        assert_eq!(get_u64(&json, "clipboard.display.textMaxLines"), 3);
        assert_eq!(
            get_choice(&json, "clipboard.display.customLayout.itemGap").as_deref(),
            Some("8")
        );
        assert_eq!(
            get_choice(&json, "appearance.theme").as_deref(),
            Some("auto")
        );
        assert_eq!(get(&json, "no.such.key"), None);
    }

    #[test]
    fn patches_nest_and_merge() {
        assert_eq!(
            patch("clipboard.history.maxCount", json!(500)),
            json!({"clipboard": {"history": {"maxCount": 500}}})
        );
        assert_eq!(
            choice_patch("clipboard.display.customLayout.itemGap", "12", true),
            json!({"clipboard": {"display": {"customLayout": {"itemGap": 12}}}})
        );
        let merged = merge(
            patch("clipboard.display.density", json!("custom")),
            json!({"clipboard": {"display": {"customLayout": {"itemGap": 4}}}}),
        );
        assert_eq!(
            merged,
            json!({"clipboard": {"display": {"density": "custom", "customLayout": {"itemGap": 4}}}})
        );
        let replaced = merge(json!({"a": [1, 2]}), json!({"a": [3]}));
        assert_eq!(replaced, json!({"a": [3]}));
    }

    #[test]
    fn capture_kinds_expand_into_five_switches() {
        assert_eq!(
            capture_kinds_patch(&["text", "image"]),
            json!({"clipboard": {"capture": {
                "text": true, "html": false, "rtf": false, "image": true, "files": false
            }}})
        );
    }

    #[test]
    fn switching_to_custom_density_starts_from_the_preset() {
        let mut settings = Settings::default();
        settings.clipboard.display.density = ListDensity::Compact;
        assert_eq!(
            custom_density_seed(&settings),
            Some(json!({"clipboard": {"display": {"customLayout": {
                "headerRow": false, "itemGap": 4, "paddingY": 4
            }}}}))
        );

        settings.clipboard.display.custom_layout.item_gap = 2;
        assert_eq!(custom_density_seed(&settings), None);
    }
}
