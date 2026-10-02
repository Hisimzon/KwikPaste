//! 安装交接：安装包已经下载、验签、写进临时目录之后，按安装形态换上新版本并退出本进程。
//!
//! 三种形态的顺序（附录 E §5.4，去掉了老更新器兼容与便携版健康检查回滚）：
//!
//! - **NSIS**：写 `update-handoff.json` → 宿主停输入（钩子、全局快捷键）→ core 关停（停剪贴板监听、
//!   WAL checkpoint 并关库，最多等 3 秒）→ `ShellExecuteW` 拉起安装包并**检查返回值** →
//!   成功：删托盘 → 释放单实例 → 退出；失败（如拒绝 UAC）：删掉交接文件 → 删托盘 → 释放单实例 →
//!   用原参数重新启动自己 → 退出。
//! - **便携版**：先换 exe（失败就回滚并报错，什么都不关）→ 写交接文件 → 停输入 → core 关停 → 删托盘 →
//!   **先释放单实例**再启动新 exe（否则新进程会把自己当成第二实例退出）→ 退出。
//! - **macOS**：解包、换 `.app`（失败回滚）→ 写交接文件 → 停输入（Carbon 热键、轮询）→ core 关停 →
//!   移除状态栏图标 → 释放单实例（删 socket）→ 用 `sh` 等本进程退出后 `open` 新包 → 退出。
//!
//! 宿主要做的事经 [`HandoffHost`] 交给平台层；本模块只按顺序调用、记日志。

#[cfg(any(target_os = "macos", test))]
pub(crate) mod macos;
#[cfg(any(target_os = "windows", test))]
pub(crate) mod nsis;
#[cfg(any(target_os = "windows", test))]
pub(crate) mod portable;

use std::ffi::OsString;
use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::Duration;

use anyhow::Context;
use chrono::{DateTime, Utc};
use kwikpaste_core::{Core, CorePaths};
use serde::{Deserialize, Serialize};

/// 交接文件名，放在启动锚点目录（`<bootstrap>/update-handoff.json`）。
const HANDOFF_FILENAME: &str = "update-handoff.json";
const HANDOFF_VERSION: u16 = 1;
/// 关库最多等这么久；超时也继续：WAL 下次打开时会自动恢复，这里只是尽量把数据落盘。
const CORE_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// 宿主返回的 future，可以在宿主自己的线程（如主线程队列）上完成。
pub type HostFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 安装交接时需要宿主（平台层）做的事。每一步都在更新器的后台任务里按顺序 await；
/// 涉及窗口、托盘的原生调用由宿主自己派发到主线程，各占一个独立的事件循环 turn。
pub trait HandoffHost: Send + Sync + 'static {
    /// 停掉输入：卸载低级键盘 / 鼠标钩子，注销全局快捷键（macOS 是 Carbon 热键），停掉轮询。
    /// 剪贴板监听由 core 关停时停。
    fn stop_input(&self) -> HostFuture<'_, ()>;

    /// 删除托盘图标（Windows 不删会在任务栏留下幽灵图标；macOS 移除 NSStatusItem）。
    fn remove_tray(&self) -> HostFuture<'_, ()>;

    /// 释放单实例：Windows 关掉互斥体、销毁 `-siw` 消息窗口；macOS 删除单实例 socket。
    /// 新进程启动前必须做完，否则新进程会把自己当成第二实例退出。
    fn release_single_instance(&self) -> HostFuture<'_, ()>;

    /// 退出进程，走宿主自己的退出路径（GPUI 的 `QuitMode::Explicit`）。Windows 上不要用
    /// `cx.restart()`（依赖 PowerShell）；重启由更新器负责拉起。
    fn exit(&self, code: i32);
}

/// 拉起外部程序。正式运行用系统实现；测试换成只记录、什么都不执行的假实现。
pub(crate) trait Launcher: Send + Sync {
    /// Windows：`ShellExecuteW("open", file, parameters)`，失败（含用户拒绝 UAC）返回错误。
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    fn shell_execute(&self, file: &Path, parameters: &str) -> anyhow::Result<()>;
    /// 启动后不等待。
    fn spawn(&self, program: &Path, args: &[OsString]) -> anyhow::Result<()>;
    /// 启动并等待结束，退出码非 0 算失败。macOS 换包时用（`touch`、`osascript`）。
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    fn run(&self, program: &Path, args: &[OsString]) -> anyhow::Result<()>;
}

pub(crate) struct SystemLauncher;

impl Launcher for SystemLauncher {
    fn shell_execute(&self, file: &Path, parameters: &str) -> anyhow::Result<()> {
        #[cfg(target_os = "windows")]
        {
            nsis::shell_execute(file, parameters)
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (file, parameters);
            anyhow::bail!("installers are only run on Windows")
        }
    }

    fn spawn(&self, program: &Path, args: &[OsString]) -> anyhow::Result<()> {
        std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start {program:?}"))?;
        Ok(())
    }

    fn run(&self, program: &Path, args: &[OsString]) -> anyhow::Result<()> {
        let status = std::process::Command::new(program)
            .args(args)
            .status()
            .with_context(|| format!("failed to run {program:?}"))?;
        anyhow::ensure!(status.success(), "{program:?} exited with {status}");
        Ok(())
    }
}

/// 交接文件：新进程据此提示「已更新到 X」。旧版本会忽略这个文件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffRecord {
    pub version: u16,
    pub from: String,
    pub to: String,
    pub kind: String,
    pub at: DateTime<Utc>,
}

impl HandoffRecord {
    /// 本进程就是交接要换上的版本：更新成功。
    pub fn succeeded(&self, running: &semver::Version) -> bool {
        self.to == running.to_string()
    }
}

/// 新进程启动时取走交接文件（读完即删）；没有或读不懂时返回 `None`。
pub fn take_handoff(paths: &CorePaths) -> Option<HandoffRecord> {
    let path = paths.bootstrap_dir().join(HANDOFF_FILENAME);
    let bytes = fs::read(&path).ok()?;
    if let Err(err) = fs::remove_file(&path) {
        log::warn!("remove {path:?} failed: {err}");
    }
    serde_json::from_slice::<HandoffRecord>(&bytes)
        .ok()
        .filter(|record| record.version == HANDOFF_VERSION)
}

/// 一次交接用到的东西。
pub(crate) struct Handoff<'a> {
    pub core: &'a Core,
    pub host: &'a dyn HandoffHost,
    pub launcher: &'a dyn Launcher,
    /// 当前 exe 与启动参数（不含 argv[0]），重启时原样带上。
    pub exe: PathBuf,
    pub args: Vec<OsString>,
    pub from: String,
    pub to: String,
}

impl Handoff<'_> {
    /// Windows 安装版：拉起 NSIS 安装包。
    #[cfg(any(target_os = "windows", test))]
    pub(crate) async fn nsis(&self, installer: &Path) -> anyhow::Result<()> {
        let head = fs::read(installer)
            .with_context(|| format!("failed to read {installer:?}"))?
            .get(..2)
            .map(<[u8]>::to_vec);
        anyhow::ensure!(
            head.as_deref() == Some(b"MZ"),
            "installer is not a Windows program"
        );

        self.write_record("nsis")?;
        self.shutdown().await;

        let parameters = nsis::parameters(&self.args);
        log::info!("starting installer {installer:?} {parameters}");
        match self.launcher.shell_execute(installer, &parameters) {
            Ok(()) => {
                self.leave().await;
                Ok(())
            }
            Err(err) => {
                log::error!("installer did not start, restarting the current version: {err:#}");
                self.remove_record();
                self.host.remove_tray().await;
                self.host.release_single_instance().await;
                if let Err(err) = self.launcher.spawn(&self.exe, &self.args) {
                    log::error!("restart after a failed update failed: {err:#}");
                }
                self.host.exit(0);
                Err(err.context("the installer did not start"))
            }
        }
    }

    /// Windows 便携版：原地换 exe 后启动新 exe。
    #[cfg(any(target_os = "windows", test))]
    pub(crate) async fn portable(&self, package: &Path) -> anyhow::Result<()> {
        let bytes = fs::read(package).with_context(|| format!("failed to read {package:?}"))?;
        let binary = portable::extract_binary(&bytes)?;
        portable::replace_binary(&self.exe, &binary)?;
        log::info!("replaced {:?} with {}", self.exe, self.to);

        self.write_record("portable")?;
        self.shutdown().await;
        self.host.remove_tray().await;
        self.host.release_single_instance().await;
        if let Err(err) = self.launcher.spawn(&self.exe, &self.args) {
            log::error!("start the updated executable failed: {err:#}");
        }
        self.host.exit(0);
        Ok(())
    }

    /// macOS：换掉 `.app`，退出后由 `sh` 等本进程结束再 `open` 新包。
    #[cfg(any(target_os = "macos", test))]
    pub(crate) async fn mac_app(
        &self,
        bundle: &Path,
        archive: &Path,
        work_dir: &Path,
    ) -> anyhow::Result<()> {
        let bytes = fs::read(archive).with_context(|| format!("failed to read {archive:?}"))?;
        let extracted = work_dir.join("KwikPaste.app");
        if extracted.exists() {
            fs::remove_dir_all(&extracted).ok();
        }
        macos::extract_app(&bytes, &extracted)?;
        macos::swap_bundle(bundle, &extracted, self.launcher)?;
        log::info!("replaced {bundle:?} with {}", self.to);

        self.write_record("app")?;
        self.shutdown().await;
        self.host.remove_tray().await;
        self.host.release_single_instance().await;
        let (program, args) = macos::relaunch_command(std::process::id(), bundle, &self.args);
        if let Err(err) = self.launcher.spawn(&program, &args) {
            log::error!("schedule relaunch failed: {err:#}");
        }
        self.host.exit(0);
        Ok(())
    }

    /// 停输入，再关停 core（剪贴板监听、同步、清理、WAL checkpoint 与关库），最多等 3 秒。
    async fn shutdown(&self) {
        log::info!("update handoff: stopping input");
        self.host.stop_input().await;
        log::info!("update handoff: shutting down the core");
        match tokio::time::timeout(CORE_SHUTDOWN_TIMEOUT, self.core.shutdown()).await {
            Ok(Ok(())) => {}
            Ok(Err(err)) => log::warn!("core shutdown before update failed: {err}"),
            Err(_) => log::warn!("core shutdown before update timed out"),
        }
    }

    /// 交接成功：删托盘、释放单实例、退出。
    #[cfg(any(target_os = "windows", test))]
    async fn leave(&self) {
        self.host.remove_tray().await;
        self.host.release_single_instance().await;
        self.host.exit(0);
    }

    fn record_path(&self) -> PathBuf {
        self.core.paths().bootstrap_dir().join(HANDOFF_FILENAME)
    }

    fn write_record(&self, kind: &str) -> anyhow::Result<()> {
        let record = HandoffRecord {
            version: HANDOFF_VERSION,
            from: self.from.clone(),
            to: self.to.clone(),
            kind: kind.to_owned(),
            at: Utc::now(),
        };
        let path = self.record_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("failed to create {dir:?}"))?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&record)?)
            .with_context(|| format!("failed to write {tmp:?}"))?;
        fs::rename(&tmp, &path).with_context(|| format!("failed to write {path:?}"))?;
        Ok(())
    }

    #[cfg(any(target_os = "windows", test))]
    fn remove_record(&self) {
        let path = self.record_path();
        if let Err(err) = fs::remove_file(&path) {
            log::warn!("remove {path:?} failed: {err}");
        }
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// 宿主与启动器共用的流水账，按发生顺序记录每一步。
    pub(crate) type Journal = Arc<Mutex<Vec<String>>>;

    /// 记录宿主收到的每一步，什么都不真正执行。
    pub(crate) struct RecordingHost {
        pub journal: Journal,
    }

    impl RecordingHost {
        fn push(&self, step: impl Into<String>) {
            self.journal.lock().unwrap().push(step.into());
        }
    }

    impl HandoffHost for RecordingHost {
        fn stop_input(&self) -> HostFuture<'_, ()> {
            self.push("host: stop_input");
            Box::pin(async {})
        }

        fn remove_tray(&self) -> HostFuture<'_, ()> {
            self.push("host: remove_tray");
            Box::pin(async {})
        }

        fn release_single_instance(&self) -> HostFuture<'_, ()> {
            self.push("host: release_single_instance");
            Box::pin(async {})
        }

        fn exit(&self, code: i32) {
            self.push(format!("host: exit({code})"));
        }
    }

    /// 只记录调用，绝不真的启动任何程序。
    pub(crate) struct FakeLauncher {
        pub journal: Journal,
        pub shell_execute_fails: bool,
    }

    impl FakeLauncher {
        fn push(&self, step: String) {
            self.journal.lock().unwrap().push(step);
        }
    }

    impl Launcher for FakeLauncher {
        fn shell_execute(&self, file: &Path, parameters: &str) -> anyhow::Result<()> {
            self.push(format!(
                "shell_execute {} {parameters}",
                file.file_name().unwrap().to_string_lossy()
            ));
            if self.shell_execute_fails {
                anyhow::bail!("ShellExecuteW failed (5): the operation was canceled by the user");
            }
            Ok(())
        }

        fn spawn(&self, program: &Path, args: &[OsString]) -> anyhow::Result<()> {
            let args: Vec<_> = args
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            self.push(format!(
                "spawn {} {}",
                program.file_name().unwrap().to_string_lossy(),
                args.join(" ")
            ));
            Ok(())
        }

        fn run(&self, program: &Path, _args: &[OsString]) -> anyhow::Result<()> {
            self.push(format!("run {}", program.display()));
            Ok(())
        }
    }

    /// 一对共用流水账的假宿主与假启动器。
    pub(crate) fn recorders(shell_execute_fails: bool) -> (RecordingHost, FakeLauncher, Journal) {
        let journal = Journal::default();
        (
            RecordingHost {
                journal: journal.clone(),
            },
            FakeLauncher {
                journal: journal.clone(),
                shell_execute_fails,
            },
            journal,
        )
    }
}

#[cfg(test)]
mod tests;
