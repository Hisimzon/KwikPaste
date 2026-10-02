//! 文案加载器（附录 D §7）：直接读与 1.x `src/locales` 同结构的 i18next JSON，不引入 rust-i18n。
//!
//! - key 写成 `命名空间:点分路径`，如 `common:actions.save`，与 1.x 的 `t("common:actions.save")` 一致；
//! - `{{var}}` 插值，不转义（界面一律按纯文本渲染）；
//! - 复数：en 在 n == 1 时取 `_one`，否则取 `_other`；zh 取基础 key；
//! - 找不到时依次回退到基础 key、zh-CN（i18next 的 `fallbackLng`），最后原样返回 key。
//!
//! 文案文件用 `include_str!` 编进二进制，启动时展开成一张表。切换语言后刷新所有窗口，
//! 并把组件层的默认文案（确定、取消等）和 gpui-component 的内置语言一起换掉。

use std::{
    collections::HashMap,
    sync::{
        LazyLock,
        atomic::{AtomicU8, Ordering},
    },
};

use gpui::{App, SharedString};
use kwikpaste_ui::{UiLocale, UiStrings};
use serde_json::Value;

/// 界面语言，取值与 1.x 设置 `appearance.language` 相同。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Language {
    #[default]
    ZhCn,
    EnUs,
}

impl Language {
    pub const ALL: [Self; 2] = [Self::ZhCn, Self::EnUs];

    pub fn tag(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::EnUs => "en-US",
        }
    }

    pub fn from_tag(tag: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|language| language.tag() == tag)
    }

    fn index(self) -> u8 {
        match self {
            Self::ZhCn => 0,
            Self::EnUs => 1,
        }
    }

    fn from_index(index: u8) -> Self {
        match index {
            1 => Self::EnUs,
            _ => Self::ZhCn,
        }
    }
}

/// 回退语言（1.x `fallbackLng`）。
const FALLBACK: Language = Language::ZhCn;

/// 命名空间与 1.x 一致；`gallery` 是原生版组件展示窗专用的新增命名空间。
const SOURCES: &[(Language, &str, &str)] = &[
    (
        Language::ZhCn,
        "common",
        include_str!("../locales/zh-CN/common.json"),
    ),
    (
        Language::ZhCn,
        "commands",
        include_str!("../locales/zh-CN/commands.json"),
    ),
    (
        Language::ZhCn,
        "clipboard",
        include_str!("../locales/zh-CN/clipboard.json"),
    ),
    (
        Language::ZhCn,
        "preferences",
        include_str!("../locales/zh-CN/preferences.json"),
    ),
    (
        Language::ZhCn,
        "gallery",
        include_str!("../locales/zh-CN/gallery.json"),
    ),
    (
        Language::EnUs,
        "common",
        include_str!("../locales/en-US/common.json"),
    ),
    (
        Language::EnUs,
        "commands",
        include_str!("../locales/en-US/commands.json"),
    ),
    (
        Language::EnUs,
        "clipboard",
        include_str!("../locales/en-US/clipboard.json"),
    ),
    (
        Language::EnUs,
        "preferences",
        include_str!("../locales/en-US/preferences.json"),
    ),
    (
        Language::EnUs,
        "gallery",
        include_str!("../locales/en-US/gallery.json"),
    ),
];

/// 两种语言展开后的文案表：`"ns:a.b.c" -> 文案`。
#[derive(Default)]
struct Catalogs {
    entries: HashMap<(Language, String), String>,
}

impl Catalogs {
    /// 解析一组 `(语言, 命名空间, JSON)`；任何一份不是“对象套字符串”都返回错误。
    fn parse(sources: &[(Language, &str, &str)]) -> Result<Self, String> {
        let mut catalogs = Self::default();
        for (language, namespace, json) in sources {
            let value: Value = serde_json::from_str(json)
                .map_err(|error| format!("{}/{namespace}.json: {error}", language.tag()))?;
            let mut flat = Vec::new();
            flatten(&value, "", &mut flat).map_err(|path| {
                format!(
                    "{}/{namespace}.json: {path} is not a string",
                    language.tag()
                )
            })?;
            for (path, text) in flat {
                catalogs
                    .entries
                    .insert((*language, format!("{namespace}:{path}")), text);
            }
        }

        Ok(catalogs)
    }

    fn get(&self, language: Language, key: &str) -> Option<&str> {
        self.entries
            .get(&(language, key.to_string()))
            .map(String::as_str)
    }

    /// 按复数规则和回退链找文案。
    fn lookup(&self, language: Language, key: &str, count: Option<i64>) -> Option<&str> {
        for language in [language, FALLBACK] {
            if let Some(count) = count {
                let suffix = plural_suffix(language, count);
                if let Some(text) = self.get(language, &format!("{key}{suffix}")) {
                    return Some(text);
                }
            }
            if let Some(text) = self.get(language, key) {
                return Some(text);
            }
        }

        None
    }

    #[cfg(test)]
    fn keys(&self, language: Language) -> impl Iterator<Item = &str> {
        self.entries
            .keys()
            .filter(move |(entry_language, _)| *entry_language == language)
            .map(|(_, key)| key.as_str())
    }
}

/// 把嵌套对象展开成点分路径；遇到非字符串的叶子返回它的路径。
fn flatten(value: &Value, prefix: &str, out: &mut Vec<(String, String)>) -> Result<(), String> {
    let Value::Object(map) = value else {
        return match value {
            Value::String(text) => {
                out.push((prefix.to_string(), text.clone()));
                Ok(())
            }
            _ => Err(prefix.to_string()),
        };
    };

    for (key, child) in map {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        flatten(child, &path, out)?;
    }

    Ok(())
}

/// i18next 的复数后缀：en 区分 one / other；zh 只有一种形式，用基础 key。
fn plural_suffix(language: Language, count: i64) -> &'static str {
    match language {
        Language::EnUs if count == 1 => "_one",
        Language::EnUs => "_other",
        Language::ZhCn => "",
    }
}

/// 替换 `{{name}}`（允许花括号内两侧有空格）；没给值的占位符原样保留。
fn interpolate(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let Some(len) = rest[start + 2..].find("}}") else {
            break;
        };
        let name = rest[start + 2..start + 2 + len].trim();
        out.push_str(&rest[..start]);
        match args.iter().find(|(arg, _)| *arg == name) {
            Some((_, value)) => out.push_str(value),
            None => out.push_str(&rest[start..start + 2 + len + 2]),
        }
        rest = &rest[start + 2 + len + 2..];
    }
    out.push_str(rest);

    out
}

static CATALOGS: LazyLock<Catalogs> =
    LazyLock::new(|| Catalogs::parse(SOURCES).unwrap_or_default());
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn language() -> Language {
    Language::from_index(CURRENT.load(Ordering::Relaxed))
}

/// 初始化：把当前语言的组件默认文案注入组件层。在 `kwikpaste_ui::init` 之后调用。
pub fn init(cx: &mut App) {
    inject_ui_strings(cx);
}

/// 切换界面语言并刷新所有窗口。
pub fn set_language(language: Language, cx: &mut App) {
    CURRENT.store(language.index(), Ordering::Relaxed);
    inject_ui_strings(cx);
}

fn inject_ui_strings(cx: &mut App) {
    let locale = match language() {
        Language::ZhCn => UiLocale::ZhCn,
        Language::EnUs => UiLocale::EnUs,
    };
    let strings = UiStrings {
        ok: t("common:ui.ok"),
        cancel: t("common:actions.cancel"),
        select_placeholder: t("common:ui.selectPlaceholder"),
    };

    kwikpaste_ui::set_ui_strings(locale, strings, cx);
}

fn translate(key: &str, count: Option<i64>, args: &[(&str, &str)]) -> SharedString {
    let Some(text) = CATALOGS.lookup(language(), key, count) else {
        return SharedString::from(key.to_string());
    };
    if args.is_empty() {
        return SharedString::from(text.to_string());
    }

    interpolate(text, args).into()
}

/// 取文案。
pub fn t(key: &str) -> SharedString {
    translate(key, None, &[])
}

/// 取文案并插值 `{{name}}`。
pub fn t_args(key: &str, args: &[(&str, &str)]) -> SharedString {
    translate(key, None, args)
}

/// 按数量取复数形式，并把数量作为 `{{count}}` 插入。
pub fn t_count(key: &str, count: i64, args: &[(&str, &str)]) -> SharedString {
    let count_text = count.to_string();
    let mut all_args = Vec::with_capacity(args.len() + 1);
    all_args.push(("count", count_text.as_str()));
    all_args.extend_from_slice(args);

    translate(key, Some(count), &all_args)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const NAMESPACES: &[&str] = &["common", "commands", "clipboard", "preferences", "gallery"];

    fn catalogs() -> &'static Catalogs {
        &CATALOGS
    }

    /// 去掉复数后缀，便于比较两种语言的 key 集合（en 的 `x_one`/`x_other` 对应 zh 的 `x`）。
    fn base_key(key: &str) -> &str {
        ["_zero", "_one", "_two", "_few", "_many", "_other"]
            .iter()
            .find_map(|suffix| key.strip_suffix(suffix))
            .unwrap_or(key)
    }

    fn placeholders(text: &str) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let mut rest = text;
        while let Some(start) = rest.find("{{") {
            let Some(len) = rest[start + 2..].find("}}") else {
                break;
            };
            names.insert(rest[start + 2..start + 2 + len].trim().to_string());
            rest = &rest[start + 2 + len + 2..];
        }
        names
    }

    #[test]
    fn locales_parse_as_nested_strings() {
        let parsed = Catalogs::parse(SOURCES).expect("every locale file is an object of strings");
        for language in Language::ALL {
            for namespace in NAMESPACES {
                assert!(
                    parsed
                        .keys(language)
                        .any(|key| key.starts_with(&format!("{namespace}:"))),
                    "{} has keys in {namespace}",
                    language.tag()
                );
            }
        }
    }

    #[test]
    fn zh_and_en_have_the_same_keys() {
        let keys = |language| -> BTreeSet<String> {
            catalogs()
                .keys(language)
                .map(|key| base_key(key).to_string())
                .collect()
        };
        let zh = keys(Language::ZhCn);
        let en = keys(Language::EnUs);

        assert_eq!(
            zh.difference(&en).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "keys only in zh-CN"
        );
        assert_eq!(
            en.difference(&zh).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "keys only in en-US"
        );
    }

    #[test]
    fn placeholders_match_between_languages() {
        for key in catalogs().keys(Language::EnUs) {
            let en = placeholders(catalogs().get(Language::EnUs, key).unwrap_or_default());
            let zh_text = catalogs()
                .get(Language::ZhCn, key)
                .or_else(|| catalogs().get(Language::ZhCn, base_key(key)))
                .unwrap_or_default();
            assert_eq!(en, placeholders(zh_text), "placeholders of {key}");
        }
    }

    /// 源码里的全部字符串字面量（按 `"` 配对，跳过 `\"` 转义；不处理原始字符串）。
    fn string_literals(source: &str) -> Vec<&str> {
        let bytes = source.as_bytes();
        let mut literals = Vec::new();
        let mut start = None;
        let mut ix = 0;
        while ix < bytes.len() {
            match (bytes[ix], start) {
                (b'\\', Some(_)) => ix += 1,
                (b'"', None) => start = Some(ix + 1),
                (b'"', Some(from)) => {
                    literals.push(&source[from..ix]);
                    start = None;
                }
                _ => {}
            }
            ix += 1;
        }
        literals
    }

    /// `t("…")` 一类调用的第一个参数（`t_args`、`t_count` 也以 `t("` 结尾或开头）。
    fn translated_literals(source: &str) -> Vec<&str> {
        ["t(\"", "t_args(\""]
            .iter()
            .flat_map(|call| source.match_indices(call))
            .filter_map(|(start, call)| {
                let literal = &source[start + call.len()..];
                literal.find('"').map(|close| &literal[..close])
            })
            .collect()
    }

    /// 代码里写出的 key 在两种语言里都存在：展示窗的全部 `"ns:…"` 字面量，以及加载器自己用的 key。
    #[test]
    fn literal_keys_exist() {
        let mut literals = string_literals(include_str!("gallery.rs"));
        literals.extend(translated_literals(include_str!("i18n.rs")));
        for source in [
            include_str!("clipboard/view/card.rs"),
            include_str!("clipboard/view/list.rs"),
            include_str!("clipboard/view/list/ops.rs"),
            include_str!("clipboard/view/list/parts.rs"),
            include_str!("clipboard/view/list/selecting.rs"),
            include_str!("clipboard/view/header.rs"),
            include_str!("clipboard/view/group_bar.rs"),
            include_str!("clipboard/view/panel.rs"),
            include_str!("clipboard/model/empty_state.rs"),
        ] {
            literals.extend(string_literals(source));
        }

        let mut checked = 0;
        for key in literals {
            let Some((namespace, path)) = key.split_once(':') else {
                continue;
            };
            if !NAMESPACES.contains(&namespace)
                || path.is_empty()
                || !path
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            {
                continue;
            }

            for language in Language::ALL {
                let found = catalogs().get(language, key).is_some()
                    || catalogs().get(language, &format!("{key}_other")).is_some();
                assert!(found, "{} is missing {key}", language.tag());
            }
            checked += 1;
        }
        assert!(checked > 40, "the scan found the gallery keys ({checked})");
    }

    fn fixture() -> Catalogs {
        Catalogs::parse(&[
            (
                Language::ZhCn,
                "demo",
                r#"{"apps":"{{count}} 个应用","greet":"你好，{{ name }}","onlyZh":"只有中文"}"#,
            ),
            (
                Language::EnUs,
                "demo",
                r#"{"apps_one":"{{count}} app","apps_other":"{{count}} apps","greet":"Hello, {{name}}"}"#,
            ),
        ])
        .expect("fixture parses")
    }

    #[test]
    fn plurals_follow_i18next() {
        let catalogs = fixture();
        let render = |language, count: i64| {
            let text = catalogs
                .lookup(language, "demo:apps", Some(count))
                .unwrap_or_default();
            interpolate(text, &[("count", &count.to_string())])
        };

        assert_eq!(render(Language::EnUs, 1), "1 app");
        assert_eq!(render(Language::EnUs, 0), "0 apps");
        assert_eq!(render(Language::EnUs, 3), "3 apps");
        assert_eq!(render(Language::ZhCn, 1), "1 个应用");
    }

    #[test]
    fn missing_keys_fall_back_to_zh_cn_then_to_the_key() {
        let catalogs = fixture();
        assert_eq!(
            catalogs.lookup(Language::EnUs, "demo:onlyZh", None),
            Some("只有中文")
        );
        assert_eq!(catalogs.lookup(Language::EnUs, "demo:nope", None), None);
        assert_eq!(t("demo:nope"), "demo:nope");
    }

    #[test]
    fn interpolation_keeps_unknown_placeholders() {
        assert_eq!(
            interpolate("你好，{{ name }}，{{other}}", &[("name", "快贴")]),
            "你好，快贴，{{other}}"
        );
        assert_eq!(interpolate("no braces", &[("x", "y")]), "no braces");
        assert_eq!(
            interpolate("dangling {{name", &[("name", "x")]),
            "dangling {{name"
        );
    }

    #[test]
    fn non_string_leaves_are_rejected() {
        let error = Catalogs::parse(&[(Language::ZhCn, "bad", r#"{"a":{"b":1}}"#)]);
        assert!(error.is_err());
    }

    #[test]
    fn real_keys_resolve_in_both_languages() {
        let check = |language, key: &str, expected: &str| {
            assert_eq!(
                catalogs().lookup(language, key, None),
                Some(expected),
                "{key}"
            );
        };
        check(Language::ZhCn, "common:actions.save", "保存");
        check(Language::EnUs, "common:actions.save", "Save");
        check(
            Language::ZhCn,
            "commands:error",
            "{{label}}失败：{{message}}",
        );
        check(
            Language::EnUs,
            "preferences:schema.settings.appearance.theme.options.auto",
            "Follow system",
        );
    }
}
