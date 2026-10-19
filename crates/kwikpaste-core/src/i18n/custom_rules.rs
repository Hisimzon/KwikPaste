//! 自定义文本规则的校验根因。
use crate::{clipboard::rules::RuleError, settings::Language};

pub fn message(language: Language, error: RuleError) -> &'static str {
    let (zh, en) = match error {
        RuleError::TooMany => (
            "最多允许 50 条自定义规则",
            "At most 50 custom rules are allowed",
        ),
        RuleError::Identity => (
            "规则 ID 必须非空且不重复",
            "Rule IDs must be non-empty and unique",
        ),
        RuleError::Name => ("规则名称不能为空", "Rule name must not be empty"),
        RuleError::TooLong => (
            "正则表达式不能超过 500 个字符",
            "Pattern must not exceed 500 characters",
        ),
        RuleError::Invalid => (
            "正则表达式无效或过于复杂",
            "Pattern is invalid or too complex",
        ),
        RuleError::Empty => (
            "正则表达式不能匹配空字符串",
            "Pattern must not match the empty string",
        ),
    };
    match language {
        Language::ZhCN => zh,
        Language::EnUS => en,
    }
}
