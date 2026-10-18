//! 反馈附件的纯数据处理：脱敏与设置差异，不读取剪贴板。

use regex::{Regex, RegexBuilder};
use serde_json::{Map, Value};

/// 一次采集复用同一套身份替换规则；空身份不做替换。
pub struct Redactor {
    home: Option<Regex>,
    username: Option<Regex>,
    replacement: &'static str,
}

impl Redactor {
    /// Windows 路径忽略大小写，接受斜杠、转义反斜杠和扩展路径前缀。
    pub fn new(home: &str, username: &str, windows: bool) -> Result<Self, regex::Error> {
        let home = home.trim_end_matches(['/', '\\']);
        let home = if home.is_empty() {
            None
        } else {
            let pattern = if windows {
                let parts: Vec<_> = home
                    .split(['/', '\\'])
                    .filter(|s| !s.is_empty())
                    .map(regex::escape)
                    .collect();
                format!(r"(?:[\\/]+\?[\\/]+)?{}", parts.join(r"[\\/]+"))
            } else {
                regex::escape(home)
            };
            Some(
                RegexBuilder::new(&pattern)
                    .case_insensitive(windows)
                    .build()?,
            )
        };
        let username = if username.is_empty() {
            None
        } else {
            Some(
                RegexBuilder::new(&regex::escape(username))
                    .case_insensitive(windows)
                    .build()?,
            )
        };
        Ok(Self {
            home,
            username,
            replacement: if windows { "%USERPROFILE%" } else { "~" },
        })
    }

    /// 只替换目录边界和独立用户名，不误改同名前缀或单词中的片段。
    pub fn text(&self, text: &str) -> String {
        let mut output = text.to_owned();
        if let Some(home) = &self.home {
            output = replace_matches(&output, home, self.replacement, |before, after| {
                !before.is_some_and(word) && !after.is_some_and(word)
            });
        }
        if let Some(username) = &self.username {
            output = replace_matches(&output, username, "<user>", |before, after| {
                let part = |ch: char| ch.is_alphanumeric() || ch == '_';
                !before.is_some_and(part) && !after.is_some_and(part)
            });
        }
        output
    }

    /// 在解析后的 JSON 上递归处理文本值，保持协议字段名及数值、布尔与 null 的类型。
    pub fn value(&self, value: &mut Value) {
        match value {
            Value::String(text) => *text = self.text(text),
            Value::Array(values) => values.iter_mut().for_each(|value| self.value(value)),
            Value::Object(values) => {
                values.values_mut().for_each(|value| self.value(value));
            }
            _ => {}
        }
    }
}

fn word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_' || ch == '-' || ch == '.'
}

/// 按原始 UTF-8 索引替换，避免大小写折叠改变字节长度后切错字符。
fn replace_matches(
    text: &str,
    regex: &Regex,
    replacement: &str,
    accept: impl Fn(Option<char>, Option<char>) -> bool,
) -> String {
    let mut output = String::new();
    let mut end = 0;
    for found in regex.find_iter(text) {
        if accept(
            text[..found.start()].chars().next_back(),
            text[found.end()..].chars().next(),
        ) {
            output.push_str(&text[end..found.start()]);
            output.push_str(replacement);
            end = found.end();
        }
    }
    output.push_str(&text[end..]);
    output
}

/// 逐叶比较当前设置与默认设置，键为转义后的 JSON Pointer；空容器也保留差异。
pub fn settings_changed(current: &Value, default: &Value) -> Value {
    fn visit(current: &Value, default: &Value, pointer: String, result: &mut Map<String, Value>) {
        if current == default {
            return;
        }
        match current {
            Value::Object(values) if !values.is_empty() => {
                for (key, value) in values {
                    let escaped = key.replace('~', "~0").replace('/', "~1");
                    visit(value, &default[key], format!("{pointer}/{escaped}"), result);
                }
            }
            Value::Array(values) if !values.is_empty() => {
                for (index, value) in values.iter().enumerate() {
                    visit(value, &default[index], format!("{pointer}/{index}"), result);
                }
                if default
                    .as_array()
                    .is_some_and(|old| old.len() > values.len())
                {
                    result.insert(pointer, current.clone());
                }
            }
            _ => {
                result.insert(pointer, current.clone());
            }
        }
    }
    let mut result = Map::new();
    visit(current, default, String::new(), &mut result);
    Value::Object(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_windows_path_spellings_and_standalone_names() {
        let redactor = Redactor::new(r"C:\Users\Alice", "Alice", true).unwrap();
        for text in [
            r"C:\Users\Alice\logs",
            "c:/users/ALICE/logs",
            r"\\?\C:\Users\Alice\logs",
            r"C:\\Users\\Alice\\logs",
        ] {
            assert!(redactor.text(text).starts_with("%USERPROFILE%"), "{text}");
        }
        assert_eq!(
            redactor.text("Alice ALICE malice Alice2"),
            "<user> <user> malice Alice2"
        );
        assert_eq!(redactor.text("(Alice), ALICE."), "(<user>), <user>.");
        assert_eq!(redactor.text(r"C:\Users\Alice2"), r"C:\Users\Alice2");
    }

    #[test]
    fn redacts_macos_and_json_without_changing_types() {
        let redactor = Redactor::new("/Users/小明", "小明", false).unwrap();
        let mut value = json!({"path":"/Users/小明/logs", "name":"小明", "enabled":true, "n":null});
        redactor.value(&mut value);
        assert_eq!(
            value,
            json!({"path":"~/logs", "name":"<user>", "enabled":true, "n":null})
        );
        assert_eq!(Redactor::new("", "", false).unwrap().text("abc"), "abc");
    }

    #[test]
    fn settings_diff_escapes_pointers_and_visits_leaves() {
        assert_eq!(
            settings_changed(
                &json!({"a/b":{"~x":[1,3]}, "empty":[]}),
                &json!({"a/b":{"~x":[1,2]}, "empty":[1]})
            ),
            json!({"/a~1b/~0x/1":3,"/empty":[]})
        );
        let defaults = serde_json::to_value(crate::settings::Settings::default()).unwrap();
        assert_eq!(settings_changed(&defaults, &defaults), json!({}));
    }
}
