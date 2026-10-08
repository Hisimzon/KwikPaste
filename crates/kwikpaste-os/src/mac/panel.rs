//! macOS NSPanel 配置、定位和非激活显示。

use std::cell::RefCell;
use std::ffi::c_void;
use std::io;
use std::ptr::NonNull;

use block2::RcBlock;
use dispatch2::DispatchQueue;
use kwikpaste_core::settings::Material;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationOptions, NSApplicationActivationPolicy,
    NSAutoresizingMaskOptions, NSEvent, NSEventMask, NSRunningApplication, NSScreen, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    NSWindow, NSWindowButton, NSWindowCollectionBehavior, NSWindowOrderingMode, NSWindowStyleMask,
    NSWorkspace,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};

use crate::geometry::{Point, Rect, Size, follow_cursor};
use kwikpaste_core::window_state::WindowGeometry;

/// 材质（透明）面板的圆角，与 1.x 面板相同。
const MATERIAL_CORNER_RADIUS: f64 = 16.;

/// 切成不进 Dock、也不出现在 Cmd-Tab 中的辅助应用策略。
pub fn set_dock_icon_visible(visible: bool) -> io::Result<()> {
    let main_thread = main_thread()?;
    let app = NSApplication::sharedApplication(main_thread);
    let policy = if visible {
        NSApplicationActivationPolicy::Regular
    } else {
        NSApplicationActivationPolicy::Accessory
    };
    if !app.setActivationPolicy(policy) {
        return Err(io::Error::other("NSApp refused the activation policy"));
    }
    Ok(())
}

/// GPUI 创建的 NSView 对应的非激活 NSPanel。
pub struct Panel {
    view: NonNull<c_void>,
    effect_view: RefCell<Option<Retained<NSVisualEffectView>>>,
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
            effect_view: RefCell::new(None),
            outside_monitor: RefCell::new(None),
            previous_foreground: RefCell::new(None),
        }
    }

    fn window(&self) -> Option<Retained<NSWindow>> {
        let view = unsafe { self.view.cast::<NSView>().as_ref() };
        view.window()
    }

    pub fn raw_view_handle(&self) -> isize {
        self.view.as_ptr() as isize
    }

    /// 补上 GPUI WindowKind::PopUp 缺少的 NSPanel 约束。
    pub fn install(&self, min_size: Size) -> io::Result<()> {
        let window = self
            .window()
            .ok_or_else(|| io::Error::other("the panel view has no window"))?;
        let mask = window.styleMask() | NSWindowStyleMask::NonactivatingPanel;
        window.setStyleMask(mask | NSWindowStyleMask::Resizable);
        hide_window_buttons(&window);
        window.setLevel(20);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::MoveToActiveSpace
                | NSWindowCollectionBehavior::FullScreenAuxiliary,
        );
        window.setContentSize(NSSize::new(min_size.width as f64, min_size.height as f64));
        self.schedule_unregister_dragged_types();
        Ok(())
    }

    /// GPUI 建窗时可能暂时注册拖放类型；独立的下一轮主队列 turn 清掉它，避免自拖回面板触发投放。
    pub fn schedule_unregister_dragged_types(&self) {
        let view = self.view.as_ptr();
        unsafe {
            DispatchQueue::main().exec_async_f(view, unregister_dragged_types);
        }
    }

    /// 在窗口下放置 AppKit 材质层；默认材质移除它并恢复不透明窗口。
    ///
    /// 不透明窗口由系统按窗口自己的圆角裁切，内容层不再另加圆角：系统圆角比 16 小，
    /// 两道圆角之间会露出 GPUI 给不透明窗口铺的黑底。材质窗口是透明的，圆角由内容层给。
    pub fn set_material(&self, material: Material) -> io::Result<()> {
        let window = self
            .window()
            .ok_or_else(|| io::Error::other("the panel view has no window"))?;
        let content = window
            .contentView()
            .ok_or_else(|| io::Error::other("the panel window has no content view"))?;
        match material {
            Material::Default => {
                set_corner_radius(&content, 0.);
                window.setOpaque(true);
                if let Some(effect) = self.effect_view.borrow_mut().take() {
                    effect.removeFromSuperview();
                }
            }
            Material::Mica | Material::Acrylic => {
                set_corner_radius(&content, MATERIAL_CORNER_RADIUS);
                window.setOpaque(false);
                let effect = if let Some(effect) = self.effect_view.borrow().as_ref() {
                    effect.clone()
                } else {
                    let marker = main_thread()?;
                    let effect =
                        NSVisualEffectView::initWithFrame(marker.alloc(), content.bounds());
                    effect.setAutoresizingMask(
                        NSAutoresizingMaskOptions::ViewWidthSizable
                            | NSAutoresizingMaskOptions::ViewHeightSizable,
                    );
                    content.addSubview_positioned_relativeTo(
                        &effect,
                        NSWindowOrderingMode::Below,
                        None,
                    );
                    *self.effect_view.borrow_mut() = Some(effect.clone());
                    effect
                };
                set_corner_radius(&effect, MATERIAL_CORNER_RADIUS);
                effect.setMaterial(match material {
                    Material::Mica => NSVisualEffectMaterial::UnderWindowBackground,
                    Material::Acrylic => NSVisualEffectMaterial::Popover,
                    Material::Default => unreachable!(),
                });
                effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
                effect.setState(NSVisualEffectState::Active);
                effect.setFrame(content.bounds());
            }
        }
        Ok(())
    }

    /// 配置预览 NSPanel：不允许调整大小，并使用 Status level。
    pub fn install_preview(&self, min_size: Size) -> io::Result<()> {
        let window = self
            .window()
            .ok_or_else(|| io::Error::other("the preview view has no window"))?;
        window.setStyleMask(window.styleMask() | NSWindowStyleMask::NonactivatingPanel);
        hide_window_buttons(&window);
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

    /// 把窗口放到 `rect`：主屏左上角为原点、向下为正（与 [`crate::mac::monitor::screens`] 相同），
    /// 单位是按 `dpi` 放大过的像素，这里除回 point，再翻成 AppKit 以主屏左下角为原点、向上为正的坐标。
    pub fn place_rect(&self, rect: Rect, dpi: u32) -> io::Result<()> {
        let main_thread = main_thread()?;
        let window = self
            .window()
            .ok_or_else(|| io::Error::other("the panel view has no window"))?;
        let primary_height = NSScreen::screens(main_thread)
            .iter()
            .next()
            .map(|screen| screen.frame().size.height)
            .ok_or_else(|| io::Error::other("no screen to place the window on"))?;
        let scale = f64::from(dpi.max(1)) / 96.;
        window.setContentSize(NSSize::new(
            f64::from(rect.width().max(1)) / scale,
            f64::from(rect.height().max(1)) / scale,
        ));
        window.setFrameOrigin(NSPoint::new(
            f64::from(rect.left) / scale,
            primary_height - f64::from(rect.bottom) / scale,
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

fn set_corner_radius(view: &NSView, radius: f64) {
    view.setWantsLayer(true);
    if let Some(layer) = view.layer() {
        layer.setCornerRadius(radius);
    }
}

/// GPUI 的无标题栏窗口仍是 Titled 窗口（能成为 key、有系统阴影和圆角），AppKit 照样放红绿灯，
/// 面板加了 Resizable 后缩放按钮还是可点的：三个按钮都藏起来。
fn hide_window_buttons(window: &NSWindow) {
    for kind in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(button) = window.standardWindowButton(kind) {
            button.setHidden(true);
        }
    }
}

fn contains(rect: NSRect, point: NSPoint) -> bool {
    point.x >= rect.origin.x
        && point.x < rect.origin.x + rect.size.width
        && point.y >= rect.origin.y
        && point.y < rect.origin.y + rect.size.height
}

extern "C" fn unregister_dragged_types(context: *mut c_void) {
    if context.is_null() {
        return;
    }
    let view = unsafe { &*(context.cast::<NSView>()) };
    view.unregisterDraggedTypes();
    crate::mac::drag_out::record_unregistered();
}
