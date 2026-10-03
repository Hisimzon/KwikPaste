//! 以管理员身份运行（与 1.x `src-tauri/src/admin.rs` 相同）。
//!
//! 设置记意图，当前进程的令牌才是真相。提权的两条路：
//! - 计划任务（正式版名 `KwikPasteAdmin`，XML 定义：`HighestAvailable`、电池也运行、无时限、
//!   优先级 4、参数 `--kwikpaste-admin-restarted`、不设触发器），`schtasks /Run /I` 启动，不弹 UAC；
//!   只有丢掉参数也无妨的启动（无参数或只有 `--auto-launch`）才能走它。
//! - 否则 `ShellExecuteW("runas")`，带原参数加 `--kwikpaste-admin-restarted`（防止提权循环）。
//!
//! 已提权且设置打开时（重）建任务；已提权且设置关闭时删任务。开发版、自测必须传和正式版不同的
//! 任务名（由 identifier 派生）。

use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Output};

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::{PCWSTR, w};

use super::args::quote_arg;
use crate::autostart::AUTO_LAUNCH_ARG;

/// 提权后的进程带的参数：带着它就不再尝试提权。
pub const ADMIN_RESTARTED_ARG: &str = "--kwikpaste-admin-restarted";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 当前进程是否已提权。
pub fn is_running_as_admin() -> bool {
    let mut token = HANDLE::default();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
        return false;
    }
    let mut elevation = TOKEN_ELEVATION::default();
    let mut length = 0u32;
    let queried = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut length,
        )
    };
    let _ = unsafe { CloseHandle(token) };

    queried.is_ok() && elevation.TokenIsElevated != 0
}

/// 本次启动是否已经是提权重启。
pub fn has_restart_marker(args: impl IntoIterator<Item = String>) -> bool {
    args.into_iter().any(|arg| arg == ADMIN_RESTARTED_ARG)
}

/// 提权用的计划任务。
pub struct AdminTask {
    name: String,
}

impl AdminTask {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// 任务存在，并且指向 `exe`（应用挪过位置后旧任务作废）。
    pub fn is_ready(&self, exe: &Path) -> bool {
        let Ok(output) = schtasks(&["/Query", "/TN", &self.name, "/FO", "LIST", "/V"]) else {
            return false;
        };
        output.status.success()
            && String::from_utf8_lossy(&output.stdout)
                .to_lowercase()
                .contains(&exe.to_string_lossy().to_lowercase())
    }

    pub fn exists(&self) -> bool {
        schtasks(&["/Query", "/TN", &self.name]).is_ok_and(|output| output.status.success())
    }

    /// （重）建任务：删掉旧的，按 XML 定义新建。需要已提权。
    pub fn create(&self, exe: &Path) -> io::Result<()> {
        let xml =
            std::env::temp_dir().join(format!("kwikpaste-admin-task-{}.xml", std::process::id()));
        std::fs::write(&xml, utf16le_with_bom(&task_xml(exe)))?;
        let _ = schtasks(&["/Delete", "/TN", &self.name, "/F"]);
        let created = Command::new("schtasks")
            .args(["/Create", "/TN", &self.name, "/XML"])
            .arg(&xml)
            .arg("/F")
            .creation_flags(CREATE_NO_WINDOW)
            .output();
        let _ = std::fs::remove_file(&xml);
        let created = created?;
        if created.status.success() {
            return Ok(());
        }

        Err(io::Error::other(format!(
            "schtasks /Create failed: {}",
            String::from_utf8_lossy(&created.stderr).trim()
        )))
    }

    /// 删任务；本来就没有也算成功。
    pub fn delete(&self) -> io::Result<()> {
        if !self.exists() {
            return Ok(());
        }
        let output = schtasks(&["/Delete", "/TN", &self.name, "/F"])?;
        if output.status.success() {
            return Ok(());
        }

        Err(io::Error::other(format!(
            "schtasks /Delete failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }

    /// `/I` 忽略任务条件：老版本建的任务带「仅接通电源时启动」，电池供电时 `/Run` 会静默跳过却
    /// 仍返回成功。
    fn run(&self) -> bool {
        schtasks(&["/Run", "/I", "/TN", &self.name]).is_ok_and(|output| output.status.success())
    }
}

/// 以管理员身份再启动一次当前程序：参数允许时走计划任务，否则 UAC。成功启动返回 `true`
/// （调用方随后退出）。
pub fn launch_elevated(task: &AdminTask, exe: &Path, args: &[String]) -> bool {
    if can_use_task_for_args(args) && task.is_ready(exe) && task.run() {
        return true;
    }

    launch_with_uac(exe, &restart_args(args))
}

fn launch_with_uac(exe: &Path, args: &[String]) -> bool {
    let params = args
        .iter()
        .map(|arg| quote_arg(arg))
        .collect::<Vec<_>>()
        .join(" ");
    let file: Vec<u16> = exe.as_os_str().encode_wide().chain(Some(0)).collect();
    let params: Vec<u16> = params.encode_utf16().chain(Some(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            None,
            w!("runas"),
            PCWSTR(file.as_ptr()),
            PCWSTR(params.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };

    result.0 as usize > 32
}

fn schtasks(args: &[&str]) -> io::Result<Output> {
    Command::new("schtasks")
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
}

/// `schtasks /Run` 不能传参：只有丢掉参数也无妨的启动才能走任务。`--auto-launch` 只在单实例回调里
/// 识别重复自启，首个实例不读它，开机自启因此也走任务、不弹 UAC。
fn can_use_task_for_args(args: &[String]) -> bool {
    args.iter()
        .all(|arg| arg == ADMIN_RESTARTED_ARG || arg == AUTO_LAUNCH_ARG)
}

/// UAC 重启的参数：原参数去掉旧标记，再加上标记。
fn restart_args(args: &[String]) -> Vec<String> {
    let mut next: Vec<String> = args
        .iter()
        .filter(|arg| *arg != ADMIN_RESTARTED_ARG)
        .cloned()
        .collect();
    next.push(ADMIN_RESTARTED_ARG.to_owned());

    next
}

/// 按需启动的提权任务定义，不设触发器。`schtasks /Create` 的默认设置只在接通电源时启动、优先级
/// 低于正常，这里显式改掉，并去掉运行时长上限、允许并行实例。
fn task_xml(exe: &Path) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Principals>
    <Principal id="Author">
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>Parallel</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>4</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{}</Command>
      <Arguments>{ADMIN_RESTARTED_ARG}</Arguments>
    </Exec>
  </Actions>
</Task>
"#,
        xml_escape(&exe.to_string_lossy())
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// 与 XML 声明的 `encoding="UTF-16"` 一致：带 BOM 的 UTF-16LE。
fn utf16le_with_bom(value: &str) -> Vec<u8> {
    [0xFEFF_u16]
        .into_iter()
        .chain(value.encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn autostart_launches_can_use_the_task() {
        assert!(can_use_task_for_args(&args(&[])));
        assert!(can_use_task_for_args(&args(&["--auto-launch"])));
        assert!(can_use_task_for_args(&args(&[
            "--auto-launch",
            ADMIN_RESTARTED_ARG
        ])));
        assert!(!can_use_task_for_args(&args(&[r"C:\history.kwikpastebak"])));
        assert!(!can_use_task_for_args(&args(&[
            "--auto-launch",
            "--unknown"
        ])));
    }

    #[test]
    fn restart_args_carry_one_marker() {
        assert_eq!(
            restart_args(&args(&["a.kwikpastebak", ADMIN_RESTARTED_ARG])),
            args(&["a.kwikpastebak", ADMIN_RESTARTED_ARG])
        );
    }

    #[test]
    fn task_xml_overrides_schtasks_defaults() {
        let xml = task_xml(Path::new(r"C:\Program Files\KwikPaste\KwikPaste.exe"));

        assert!(xml.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(xml.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(xml.contains("<StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>"));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains("<Priority>4</Priority>"));
        assert!(xml.contains(r"<Command>C:\Program Files\KwikPaste\KwikPaste.exe</Command>"));
        assert!(xml.contains("<Arguments>--kwikpaste-admin-restarted</Arguments>"));
        assert!(!xml.contains("<Triggers>"));
    }

    #[test]
    fn task_xml_escapes_the_path_and_is_utf16le_with_bom() {
        let xml = task_xml(Path::new(r"D:\Tools & <Apps>\KwikPaste.exe"));
        assert!(xml.contains(r"<Command>D:\Tools &amp; &lt;Apps&gt;\KwikPaste.exe</Command>"));
        assert_eq!(
            utf16le_with_bom("<a/>"),
            [0xFF, 0xFE, b'<', 0, b'a', 0, b'/', 0, b'>', 0]
        );
    }
}
