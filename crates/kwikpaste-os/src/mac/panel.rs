//! macOS NSPanel 配置、定位和非激活显示。

use std::cell::RefCell;
use std::ffi::c_void;
use std::io;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy, NSEvent,
    NSEventMask, NSRunningApplication, NSScreen, NSView, NSWindow, NSWindowCollectionBehavior,
    NSWindowStyleMask, NSWorkspace,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::geometry::{Point, Rect, Size, follow_cursor};
use kwikpaste_core::window_state::WindowGeometry;

/// 切成不进 Dock、也不出现在 Cmd-Tab 中的辅助应用策略。
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

/// GPUI 创建的 NSView 对应的非激活 NSPanel。
pub struct Panel {
    view: NonNull<c_void>,
    outside_monitor: RefCell<Option<Retained<AnyObject>>>,
    previous_foreground: RefCell<Option<Retained<NSRunningApplication>>>,
}

impl Panel {
    /// 包装 GPUI 窗口的 `NSView`。
    ///
    /// # Safety
    /// `ns_view` 必须仍然存活，并且所有调用都在主线程执行。
    pub unsafe fn from_raw(ns_view: NonNull<c_void>) -> Self {
        Self {
            view: ns_view,
            outside_monitor: RefCell::new(None),
            previous_foreground: RefCell::new(None),
        }
    }

    fn window(&self) -> Option<Retained<NSWindow>> {
        let view = unsafe { self.view.cast::<NSView>().as_ref() };
        view.window()
    }

    /// 补上 GPUI WindowKind::PopUp 缺少的 NSPanel 约束。
    pub fn install(&self, min_size: Size) -> io::Result<()> {
        let window = self
            .window()
            .ok_or_else(|| io::Error::other("the panel view has no window"))?;
        let mask = window.styleMask() | NSWindowStyleMask::NonactivatingPanel;
        window.setStyleMask(mask | NSWindowStyleMask::Resizable);
        window.setLevel(20);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::MoveToActiveSpace
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        window.setContentSize(NSSize::new(min_size.width as f64, min_size.height as f64));
        Ok(())
    }

    /// 配置预览 NSPanel：不允许调整大小，并使用 Status level。
    pub fn install_preview(&self, min_size: Size) -> io::Result<()> {
        let window = self
            .window()
            .ok_or_else(|| io::Error::other("the preview view has no window"))?;
        window.setStyleMask(window.styleMask() | NSWindowStyleMask::NonactivatingPanel);
        window.setLevel(25);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::MoveToActiveSpace
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        window.setContentSize(NSSize::new(min_size.width as f64, min_size.height as f64));
        Ok(())
    }

    pub fn is_visible(&self) -> bool {
        self.window().is_some_and(|window| window.isVisible())
    }

    pub fn is_key(&self) -> bool {
        self.window().is_some_and(|window| window.isKeyWindow())
    }

    pub fn is_panel(&self) -> bool {
        self.window()
            .is_some_and(|window| unsafe { msg_send![&*window, isKindOfClass: class!(NSPanel)] })
    }

    pub fn can_become_main(&self) -> bool {
        self.window()
            .is_some_and(|window| window.canBecomeMainWindow())
    }

    pub fn can_become_key(&self) -> bool {
        self.window()
            .is_some_and(|window| window.canBecomeKeyWindow())
    }

    pub fn style_mask(&self) -> Option<NSWindowStyleMask> {
        self.window().map(|window| window.styleMask())
    }

    pub fn collection_behavior(&self) -> Option<NSWindowCollectionBehavior> {
        self.window().map(|window| window.collectionBehavior())
    }

    pub fn frame(&self) -> Option<NSRect> {
        self.window().map(|window| window.frame())
    }

    pub fn backing_scale(&self) -> f64 {
        self.window()
            .and_then(|window| window.screen().map(|screen| screen.backingScaleFactor()))
            .unwrap_or(1.)
    }

    /// 把面板外框定位到鼠标附近，必要时限制到当前屏幕的可见工作区。
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

    pub fn place_center(&self) -> io::Result<()> {
        let main_thread = main_thread()?;
        let window = self
            .window()
            .ok_or_else(|| io::Error::other("the panel view has no window"))?;
        let screen =
            NSScreen::mainScreen(main_thread).ok_or_else(|| io::Error::other("no main screen"))?;
        let visible = screen.visibleFrame();
        let frame = window.frame();
        let x = visible.origin.x + (visible.size.width - frame.size.width) / 2.;
        let y = visible.origin.y + (visible.size.height - frame.size.height) / 2.;
        window.setFrameOrigin(NSPoint::new(x, y));
        Ok(())
    }

    pub fn place_saved(&self, geometry: WindowGeometry) -> io::Result<()> {
        let main_thread = main_thread()?;
        let window = self
            .window()
            .ok_or_else(|| io::Error::other("the panel view has no window"))?;
        let target_y = -geometry.y - window.frame().size.height;
        let on_screen = NSScreen::screens(main_thread)
            .iter()
            .any(|screen| contains(screen.frame(), NSPoint::new(geometry.x, target_y)));
        if !on_screen {
            return Err(io::Error::other(
                "saved panel position is no longer on a screen",
            ));
        }
        window.setContentSize(NSSize::new(geometry.width.max(1.), geometry.height.max(1.)));
        window.setFrameOrigin(NSPoint::new(geometry.x, target_y));
        Ok(())
    }

    pub fn place_rect(&self, rect: Rect, dpi: u32) -> io::Result<()> {
        let window = self
            .window()
            .ok_or_else(|| io::Error::other("the panel view has no window"))?;
        let scale = f64::from(dpi.max(1)) / 96.;
        window.setContentSize(NSSize::new(
            f64::from(rect.width().max(1)) / scale,
            f64::from(rect.height().max(1)) / scale,
        ));
        window.setFrameOrigin(NSPoint::new(
            f64::from(rect.left) / scale,
            -f64::from(rect.bottom) / scale,
        ));
        Ok(())
    }

    pub fn geometry(&self) -> Option<WindowGeometry> {
        let window = self.window()?;
        let frame = window.frame();
        Some(WindowGeometry {
            x: frame.origin.x,
            y: -(frame.origin.y + frame.size.height),
            width: frame.size.width,
            height: frame.size.height,
            scale: window
                .screen()
                .map_or(1., |screen| screen.backingScaleFactor()),
        })
    }

    /// 不激活应用，但让 NSPanel 成为 key window 以接收编辑键盘。
    pub fn show_without_activating(&self) {
        let Some(window) = self.window() else {
            return;
        };
        *self.previous_foreground.borrow_mut() =
            NSWorkspace::sharedWorkspace().frontmostApplication();
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        window.orderFrontRegardless();
        window.makeKeyWindow();
    }

    pub fn hide(&self) {
        let Some(window) = self.window() else {
            return;
        };
        window.orderOut(None);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::MoveToActiveSpace
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        self.restore_previous_foreground();
    }

    pub fn restore_previous_foreground(&self) {
        if let Some(previous) = self.previous_foreground.borrow_mut().take() {
            #[allow(deprecated)]
            let _ = previous
                .activateWithOptions(NSApplicationActivationOptions::ActivateIgnoringOtherApps);
        }
    }

    pub fn start_global_mouse_monitor(&self, callback: impl Fn() + 'static) -> io::Result<()> {
        self.stop_global_mouse_monitor();
        let Some(window) = self.window() else {
            return Err(io::Error::other("the panel view has no window"));
        };
        let monitor = RcBlock::new(move |event: NonNull<NSEvent>| {
            let point = unsafe { event.as_ref() }.locationInWindow();
            if !contains(window.frame(), point) {
                callback();
            }
        });
        let mask =
            NSEventMask::LeftMouseDown | NSEventMask::RightMouseDown | NSEventMask::OtherMouseDown;
        let token = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &monitor)
            .ok_or_else(|| io::Error::other("global mouse monitor could not be installed"))?;
        *self.outside_monitor.borrow_mut() = Some(token);
        Ok(())
    }

    pub fn stop_global_mouse_monitor(&self) {
        if let Some(token) = self.outside_monitor.borrow_mut().take() {
            unsafe { NSEvent::removeMonitor(&token) };
        }
    }

    pub fn begin_editing(&self) {
        if let Some(window) = self.window() {
            window.makeKeyWindow();
        }
    }

    pub fn activation_policy(&self) -> Option<NSApplicationActivationPolicy> {
        let marker = MainThreadMarker::new()?;
        Some(NSApplication::sharedApplication(marker).activationPolicy())
    }

    pub fn application_is_active(&self) -> bool {
        MainThreadMarker::new()
            .is_some_and(|marker| NSApplication::sharedApplication(marker).isActive())
    }

    pub fn foreground_unchanged(&self) -> Option<bool> {
        let previous = self.previous_foreground.borrow();
        let previous = previous.as_ref()?.processIdentifier();
        let current = NSWorkspace::sharedWorkspace().frontmostApplication();
        Some(current.is_some_and(|application| application.processIdentifier() == previous))
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
