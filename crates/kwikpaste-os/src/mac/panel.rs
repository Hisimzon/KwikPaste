//! macOS 面板的最小实现：不激活地显示 / 隐藏，并放到光标附近。
//!
//! GPUI 的 `WindowKind::PopUp` 在 macOS 上已经是 `NSPanel` + `NonactivatingPanel`。还没做的
//! （附录 C §9，需要 CI 自测和远程 Mac 验收）：TODO(macOS) 面板 level 改 Dock 层、显示 / 隐藏两态的
//! collectionBehavior、styleMask 加 Resizable、`makeKeyWindow` 与等待 key 状态、16 px 圆角。
//!
//! 所有 NSWindow 调用都必须在 GPUI 的 `App` 借用之外进行（`cx.spawn` 的任务体里、`update` 返回之后）：
//! `setFrameOrigin:`、`orderOut:` 会同步回调 GPUI，借用中回调会被丢掉，留下过期的缩放和显示器状态。

use std::ffi::c_void;
use std::io;
use std::ptr::NonNull;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSEvent, NSScreen, NSView, NSWindow,
};
use objc2_foundation::{NSPoint, NSRect};

use crate::geometry::{Point, Rect, Size, follow_cursor};

/// 切成不进 Dock、不出现在 ⌘Tab 里的辅助应用。GPUI 启动时无条件设成 Regular，
/// 所以要在 `Application::run` 回调里第一时间调用。
pub fn use_accessory_activation_policy() -> io::Result<()> {
    let main_thread = main_thread()?;
    let app = NSApplication::sharedApplication(main_thread);
    if !app.setActivationPolicy(NSApplicationActivationPolicy::Accessory) {
        return Err(io::Error::other(
            "NSApp refused the accessory activation policy",
        ));
    }

    Ok(())
}

/// 面板窗口的原生视图。只能在主线程使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Panel {
    view: NonNull<c_void>,
}

impl Panel {
    /// 包装 GPUI 窗口的 `NSView`。
    ///
    /// # Safety
    /// `ns_view` 必须是仍然存活的 `NSView`，并且只在主线程使用。
    pub unsafe fn from_raw(ns_view: NonNull<c_void>) -> Self {
        Self { view: ns_view }
    }

    fn window(&self) -> Option<Retained<NSWindow>> {
        let view = unsafe { self.view.cast::<NSView>().as_ref() };
        view.window()
    }

    pub fn is_visible(&self) -> bool {
        self.window().is_some_and(|window| window.isVisible())
    }

    /// 把面板左上角放到光标处，放不下时推回光标所在屏幕的可见区域（去掉菜单栏和 Dock）。
    pub fn place_near_cursor(&self) -> io::Result<()> {
        let main_thread = main_thread()?;
        let window = self
            .window()
            .ok_or_else(|| io::Error::other("the panel view has no window"))?;
        let mouse = NSEvent::mouseLocation();
        let screen = NSScreen::screens(main_thread)
            .iter()
            .find(|screen| contains(screen.frame(), mouse))
            .or_else(|| NSScreen::mainScreen(main_thread))
            .ok_or_else(|| io::Error::other("no screen to place the panel on"))?;

        // AppKit 的屏幕坐标 y 轴向上，翻成 y 轴向下再套用与 Windows 相同的跟随光标规则。
        let visible = screen.visibleFrame();
        let work_area = Rect {
            left: visible.origin.x.round() as i32,
            top: (-(visible.origin.y + visible.size.height)).round() as i32,
            right: (visible.origin.x + visible.size.width).round() as i32,
            bottom: (-visible.origin.y).round() as i32,
        };
        let frame = window.frame();
        let size = Size {
            width: frame.size.width.round() as i32,
            height: frame.size.height.round() as i32,
        };
        let cursor = Point {
            x: mouse.x.round() as i32,
            y: (-mouse.y).round() as i32,
        };
        let target = follow_cursor(cursor, work_area, size);

        window.setFrameOrigin(NSPoint::new(
            f64::from(target.left),
            f64::from(-target.bottom),
        ));
        Ok(())
    }

    /// 不激活应用、不成为 key 地显示。
    pub fn show_without_activating(&self) {
        if let Some(window) = self.window() {
            window.orderFrontRegardless();
        }
    }

    pub fn hide(&self) {
        if let Some(window) = self.window() {
            window.orderOut(None);
        }
    }
}

fn main_thread() -> io::Result<MainThreadMarker> {
    MainThreadMarker::new()
        .ok_or_else(|| io::Error::other("AppKit calls must run on the main thread"))
}

fn contains(rect: NSRect, point: NSPoint) -> bool {
    point.x >= rect.origin.x
        && point.x < rect.origin.x + rect.size.width
        && point.y >= rect.origin.y
        && point.y < rect.origin.y + rect.size.height
}
