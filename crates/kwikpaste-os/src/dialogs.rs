//! 不依赖 GPUI 的原生提示框与外部链接打开。

use std::path::Path;

/// 原生对话框中的一个自定义按钮。
#[derive(Clone, Debug)]
pub struct DialogButton {
    pub label: String,
}

impl DialogButton {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
        }
    }
}

/// 显示带自定义按钮的系统对话框，返回按钮下标；Esc/关闭返回 `None`。
pub fn show(title: &str, body: &str, buttons: &[DialogButton]) -> Option<usize> {
    #[cfg(target_os = "windows")]
    {
        return windows::show(title, body, buttons);
    }
    #[cfg(target_os = "macos")]
    {
        return mac::show(title, body, buttons);
    }
    #[allow(unreachable_code)]
    {
        log::warn!("native dialog is unavailable on this platform");
        None
    }
}

/// 打开外部链接；调用方负责先校验链接来源。
pub fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        return std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()
            .map(|_| ());
    }
    #[cfg(target_os = "macos")]
    {
        return std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map(|_| ());
    }
    #[allow(unreachable_code)]
    Err(std::io::Error::other("opening URLs is unsupported"))
}

/// 在文件管理器中打开目录。
pub fn open_path(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        return std::process::Command::new("explorer.exe")
            .arg(path)
            .spawn()
            .map(|_| ());
    }
    #[cfg(target_os = "macos")]
    {
        return std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map(|_| ());
    }
    #[allow(unreachable_code)]
    Err(std::io::Error::other("opening paths is unsupported"))
}

#[cfg(target_os = "windows")]
mod windows {
    use std::mem::size_of;

    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::Controls::{
        TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TDF_ALLOW_DIALOG_CANCELLATION, TaskDialogIndirect,
    };
    use windows::core::PCWSTR;

    use super::DialogButton;

    pub(super) fn show(title: &str, body: &str, buttons: &[DialogButton]) -> Option<usize> {
        if buttons.is_empty() {
            return None;
        }
        let title = wide(title);
        let body = wide(body);
        let labels: Vec<Vec<u16>> = buttons.iter().map(|button| wide(&button.label)).collect();
        let native_buttons: Vec<TASKDIALOG_BUTTON> = labels
            .iter()
            .enumerate()
            .map(|(index, label)| TASKDIALOG_BUTTON {
                nButtonID: 1000 + i32::try_from(index).unwrap_or(i32::MAX - 1000),
                pszButtonText: PCWSTR(label.as_ptr()),
            })
            .collect();
        let config = TASKDIALOGCONFIG {
            cbSize: size_of::<TASKDIALOGCONFIG>() as u32,
            hwndParent: HWND::default(),
            dwFlags: TDF_ALLOW_DIALOG_CANCELLATION,
            pszWindowTitle: PCWSTR(title.as_ptr()),
            pszMainInstruction: PCWSTR(title.as_ptr()),
            pszContent: PCWSTR(body.as_ptr()),
            cButtons: native_buttons.len() as u32,
            pButtons: native_buttons.as_ptr(),
            nDefaultButton: native_buttons.first().map_or(0, |button| button.nButtonID),
            ..Default::default()
        };
        let mut result = 0;
        if let Err(error) = unsafe { TaskDialogIndirect(&config, Some(&mut result), None, None) } {
            log::warn!("TaskDialogIndirect failed: {error}");
            return None;
        }
        usize::try_from(result - 1000)
            .ok()
            .filter(|index| *index < buttons.len())
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::DialogButton;

    pub(super) fn show(_title: &str, _body: &str, _buttons: &[DialogButton]) -> Option<usize> {
        // TODO(macOS): 用 NSAlert 提供与 Windows TaskDialog 相同的按钮顺序。
        log::warn!("NSAlert dialog integration is not available yet");
        None
    }
}
