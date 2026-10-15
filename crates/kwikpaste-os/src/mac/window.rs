//! 普通窗口（偏好设置、引导）的前台交接；剪贴板的非激活面板不走这里。

use std::{ffi::c_void, io, ptr::NonNull};

use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{
    NSApplicationActivationOptions, NSRunningApplication, NSView, NSWindow, NSWindowStyleMask,
};

/// 持有 NSWindow，延迟执行时不会用到已释放的原生指针。
pub struct OrdinaryWindow(Retained<NSWindow>);

impl OrdinaryWindow {
    /// 从 GPUI 窗口的 NSView 取出所在的普通窗口；只能在主线程调用。
    ///
    /// # Safety
    /// `view` 必须是仍存活的 NSView 指针。
    pub unsafe fn from_raw(view: NonNull<c_void>) -> io::Result<Self> {
        main_thread()?;
        // SAFETY: 调用方保证 NSView 有效；取到的窗口由 Retained 持有。
        let view = unsafe { view.cast::<NSView>().as_ref() };
        let window = view
            .window()
            .ok_or_else(|| io::Error::other("the view has no window"))?;
        if window
            .styleMask()
            .contains(NSWindowStyleMask::NonactivatingPanel)
        {
            return Err(io::Error::other("cannot activate a nonactivating panel"));
        }
        Ok(Self(window))
    }

    /// 激活应用并把窗口带到最前（最小化的先还原）。必须在 GPUI 借用之外调用：激活回调会重入。
    /// 激活被拒绝时仍把窗口排到最前，只记日志。
    pub fn bring_to_front(&self) -> io::Result<()> {
        main_thread()?;
        if self.0.isMiniaturized() {
            self.0.deminiaturize(None);
        }
        #[allow(deprecated)]
        let activated = NSRunningApplication::currentApplication()
            .activateWithOptions(NSApplicationActivationOptions::empty());
        if !activated {
            log::warn!("macOS refused to activate the app for an ordinary window");
        }
        self.0.makeKeyAndOrderFront(None);
        Ok(())
    }
}

fn main_thread() -> io::Result<MainThreadMarker> {
    MainThreadMarker::new()
        .ok_or_else(|| io::Error::other("AppKit calls must run on the main thread"))
}
