//! Windows 命令行参数的引号规则（与 1.x `core/windows_args.rs` 相同）。

/// 给一个参数加引号：含空白或引号时用 C 运行库的规则包起来，否则原样返回。
pub fn quote_arg(value: &str) -> String {
    if value.is_empty() {
        return "\"\"".to_owned();
    }
    if !value
        .bytes()
        .any(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | b'"'))
    {
        return value.to_owned();
    }

    let mut result = String::from("\"");
    let mut backslashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            backslashes += 1;
            continue;
        }
        if ch == '"' {
            result.push_str(&"\\".repeat(backslashes * 2 + 1));
            result.push('"');
            backslashes = 0;
            continue;
        }
        result.push_str(&"\\".repeat(backslashes));
        backslashes = 0;
        result.push(ch);
    }
    result.push_str(&"\\".repeat(backslashes * 2));
    result.push('"');

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_values_stay_plain() {
        assert_eq!(quote_arg("--auto-launch"), "--auto-launch");
        assert_eq!(quote_arg(""), "\"\"");
    }

    #[test]
    fn spaces_are_wrapped() {
        assert_eq!(
            quote_arg(r"C:\Program Files\KwikPaste\KwikPaste.exe"),
            r#""C:\Program Files\KwikPaste\KwikPaste.exe""#
        );
    }

    #[test]
    fn quotes_and_trailing_backslashes_are_escaped() {
        assert_eq!(quote_arg(r#"value"tail"#), r#""value\"tail""#);
        assert_eq!(quote_arg(r"C:\a b\"), r#""C:\a b\\""#);
    }
}
