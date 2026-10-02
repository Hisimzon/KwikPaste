//! 只能由平台层实现、core 又要用到的能力。
//!
//! 按 workspace 的分工，直接调用 Win32 / AppKit 的代码放 `kwikpaste-os`：前台应用识别、
//! 运行中应用枚举、应用包元信息读取、提示音播放。core 只定义接口，宿主在启动后用
//! `Core::set_platform_services` 接上实现；没接上时用 [`NoPlatformServices`]，相关功能静默降级。

use std::path::{Path, PathBuf};

use crate::db::models::Platform;
use crate::error::{AppError, Result};

/// 这次剪贴板变更来自的前台应用。必须在剪贴板变化回调里**同步**探测，延后再问前台早就切走了。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrontmostApp {
    /// 稳定主键。macOS = bundle id（如 `com.apple.Safari`），Windows = exe 绝对路径。
    pub id: String,
    /// 显示名（macOS localizedName；Windows exe 文件名去扩展名）。
    pub name: String,
    pub platform: Platform,
    /// 抽取应用图标用的路径（macOS `.app` 包 / Windows exe）；拿不到则 `None`。
    pub icon_source: Option<PathBuf>,
}

/// 扫描到的一个应用：运行中应用、用户手动选择的应用包或按 id 查到的应用。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScannedApp {
    pub id: String,
    pub name: String,
    /// 应用包 / exe 路径，用来抽图标。
    pub path: Option<PathBuf>,
    pub platform: Platform,
}

/// 平台层提供给 core 的能力。方法可能在任意线程上调用，实现必须线程安全；
/// 除 [`PlatformServices::frontmost_app`] 外都可能较慢，core 会放到阻塞线程池里调用。
pub trait PlatformServices: Send + Sync + 'static {
    /// 当前前台应用；失败、没有前台应用或拿不到稳定 id 时返回 `None`。
    /// 1.x 实现：macOS `NSWorkspace.frontmostApplication`（无 bundle id 的进程返回 `None`），
    /// Windows `GetForegroundWindow` + `QueryFullProcessImageNameW`。
    fn frontmost_app(&self) -> Option<FrontmostApp>;

    /// 当前运行中的用户应用，按 id 去重。1.x 实现：macOS 取 activationPolicy 为 Regular 的
    /// `runningApplications`；Windows 枚举可见且有标题的顶层窗口所属进程的 exe（规范化路径）。
    fn running_apps(&self) -> Vec<ScannedApp>;

    /// 用户手动选择的应用。macOS 只接受 `.app` 包（读 Info.plist 与本地化名称），
    /// Windows 只接受 `.exe`（规范化路径）。路径不合法时返回用户可读错误。
    fn app_from_path(&self, path: &Path) -> Result<ScannedApp>;

    /// 按应用 id 查应用（macOS 用 Spotlight 按 bundle id 找 `.app`；Windows 没有对应能力返回 `None`）。
    /// 用于把默认忽略的应用（钥匙串访问、密码）补成完整记录。
    fn app_from_id(&self, id: &str) -> Option<ScannedApp>;

    /// 异步播放一次复制提示音，不阻塞调用方。
    fn play_copy_sound(&self);
}

/// 没接平台层时的默认实现：识别不到前台应用、没有运行中应用、不播放提示音。
#[derive(Debug, Default, Clone, Copy)]
pub struct NoPlatformServices;

impl PlatformServices for NoPlatformServices {
    fn frontmost_app(&self) -> Option<FrontmostApp> {
        None
    }

    fn running_apps(&self) -> Vec<ScannedApp> {
        Vec::new()
    }

    fn app_from_path(&self, _path: &Path) -> Result<ScannedApp> {
        Err(AppError::Clipboard(
            "app lookup is not available on this platform".to_owned(),
        ))
    }

    fn app_from_id(&self, _id: &str) -> Option<ScannedApp> {
        None
    }

    fn play_copy_sound(&self) {}
}
