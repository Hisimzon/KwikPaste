//! macOS 上界面要跟随的系统设置。
//!
//! macOS 没有 Windows 那样的「文本大小」，系数恒为 1。高对比度对应“增强对比度”，减少动画对应
//! “减弱动态效果”。TODO(macOS)：监听 `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification`，
//! 运行中切换设置后通知宿主重读。

use objc2_app_kit::NSWorkspace;

/// 一次读到的系统设置，字段与 Windows 版相同。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SystemSettings {
    pub text_scale: f64,
    pub high_contrast: bool,
    pub reduce_motion: bool,
}

pub fn read() -> SystemSettings {
    let workspace = NSWorkspace::sharedWorkspace();

    SystemSettings {
        text_scale: 1.0,
        high_contrast: workspace.accessibilityDisplayShouldIncreaseContrast(),
        reduce_motion: workspace.accessibilityDisplayShouldReduceMotion(),
    }
}
