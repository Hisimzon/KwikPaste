//! macOS 上界面要跟随的系统设置。
//!
//! macOS 没有 Windows 那样的「文本大小」，系数恒为 1。高对比度对应“增强对比度”，减少动画对应
//! “减弱动态效果”。监听 `NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification`，运行中切换
//! 设置后通知宿主重读。

use std::cell::RefCell;
use std::io;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{
    NSAppearanceNameDarkAqua, NSApplication, NSWorkspace,
    NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification,
};
use objc2_foundation::{NSNotification, NSNotificationCenter};

thread_local! {
    static DISPLAY_OPTIONS_OBSERVER: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>> =
        const { RefCell::new(None) };
}

/// 一次读到的系统设置，字段与 Windows 版相同。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SystemSettings {
    pub text_scale: f64,
    pub high_contrast: bool,
    pub reduce_motion: bool,
    /// 系统要求减少透明度时为 false。
    pub transparency: bool,
    pub dark: bool,
}

pub fn read() -> SystemSettings {
    let workspace = NSWorkspace::sharedWorkspace();
    let dark = MainThreadMarker::new().is_some_and(|marker| {
        let appearance = NSApplication::sharedApplication(marker).effectiveAppearance();
        let dark_name = unsafe { NSAppearanceNameDarkAqua };
        appearance.name().isEqualToString(dark_name)
    });

    SystemSettings {
        text_scale: 1.0,
        high_contrast: workspace.accessibilityDisplayShouldIncreaseContrast(),
        reduce_motion: workspace.accessibilityDisplayShouldReduceMotion(),
        transparency: !workspace.accessibilityDisplayShouldReduceTransparency(),
        dark,
    }
}

/// 监听 macOS 的辅助功能显示选项；句柄绑定主线程并随进程存活。
pub fn watch(on_change: impl Fn() + 'static) -> io::Result<()> {
    DISPLAY_OPTIONS_OBSERVER.with(|slot| {
        if slot.borrow().is_some() {
            return Ok(());
        }
        let block = RcBlock::new(move |_notification: NonNull<NSNotification>| on_change());
        let center = NSNotificationCenter::defaultCenter();
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                None,
                None,
                &block,
            )
        };
        *slot.borrow_mut() = Some(observer);
        Ok(())
    })
}
