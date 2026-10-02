//! NSIS 安装版：用 `/P /R /UPDATE /ARGS <原参数>` 拉起安装包（被动模式、装完重启、按更新处理、
//! 重启时带上原来的命令行参数）。参数转义照抄 Tauri，`/` 也要加引号，免得 NSIS 当成自己的开关。

use std::ffi::OsString;

/// 安装包的命令行：模式开关 + 原进程参数（不含 argv[0]）。
pub(crate) fn parameters(args: &[OsString]) -> String {
    ["/P", "/R", "/UPDATE", "/ARGS"]
        .into_iter()
        .map(str::to_owned)
        .chain(args.iter().map(|arg| escape_arg(&arg.to_string_lossy())))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 与 Tauri `escape_nsis_current_exe_arg` 相同：按 Windows 命令行规则加引号和反斜杠，
/// 含空白、`/` 或为空时整体加引号。
pub(crate) fn escape_arg(arg: &str) -> String {
    let quote = arg.chars().any(|c| c == ' ' || c == '\t' || c == '/') || arg.is_empty();
    let mut out = String::with_capacity(arg.len() + 2);
    if quote {
        out.push('"');
    }
    let mut backslashes = 0usize;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
        } else {
            if c == '"' {
                out.extend(std::iter::repeat_n('\\', backslashes + 1));
            }
            backslashes = 0;
        }
        out.push(c);
    }
    if quote {
        out.extend(std::iter::repeat_n('\\', backslashes));
        out.push('"');
    }
    out
}

/// `ShellExecuteW("open", file, parameters)`，检查返回值：小于等于 32 是失败
/// （用户拒绝 UAC 时是 `ERROR_CANCELLED`）。Tauri 不检查，失败时应用会直接消失。
#[cfg(target_os = "windows")]
pub(crate) fn shell_execute(file: &std::path::Path, parameters: &str) -> anyhow::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOW;

    let wide = |value: &std::ffi::OsStr| -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    };
    let operation = wide(std::ffi::OsStr::new("open"));
    let file = wide(file.as_os_str());
    let parameters = wide(std::ffi::OsStr::new(parameters));

    // SAFETY: 三个字符串都是以 0 结尾的 UTF-16 缓冲区，在调用期间有效；hwnd 与工作目录传空。
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            parameters.as_ptr(),
            std::ptr::null(),
            SW_SHOW,
        )
    };
    if result as usize > 32 {
        return Ok(());
    }

    Err(anyhow::anyhow!(
        "ShellExecuteW failed ({}): {}",
        result as usize,
        std::io::Error::last_os_error()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 与 Tauri `it_escapes_correctly_for_nsis` 的向量相同。
    #[test]
    fn escapes_like_tauri() {
        let cases = [
            ("something", "something"),
            ("--flag", "--flag"),
            ("--empty=", "--empty="),
            ("--arg=value", "--arg=value"),
            ("some space", "\"some space\""),
            ("--arg value", "\"--arg value\""),
            ("--arg=unwrapped space", "\"--arg=unwrapped space\""),
            ("--arg=\"wrapped\"", "--arg=\\\"wrapped\\\""),
            ("--arg=\"wrapped space\"", "\"--arg=\\\"wrapped space\\\"\""),
            (
                "--arg=midword\"wrapped space\"",
                "\"--arg=midword\\\"wrapped space\\\"\"",
            ),
            ("", "\"\""),
        ];

        for (arg, escaped) in cases {
            assert_eq!(escape_arg(arg), escaped, "{arg}");
        }
        // 比 std 多一条：`/` 也要加引号。
        assert_eq!(escape_arg("/S"), "\"/S\"");
        assert_eq!(escape_arg("C:\\dir\\"), "C:\\dir\\");
        assert_eq!(escape_arg("C:\\my dir\\"), "\"C:\\my dir\\\\\"");
    }

    #[test]
    fn parameters_carry_the_original_arguments() {
        assert_eq!(parameters(&[]), "/P /R /UPDATE /ARGS");
        assert_eq!(
            parameters(&[OsString::from("--auto-launch"), OsString::from("a b")]),
            "/P /R /UPDATE /ARGS --auto-launch \"a b\""
        );
    }
}
