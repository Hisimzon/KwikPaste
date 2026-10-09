//! macOS 窗口材质层：在 GPUI 视图下面放一块系统 `NSVisualEffectView`，映射与 1.x 相同：
//! Mica → `UnderWindowBackground`，Acrylic → `Popover`。
//!
//! 不用 GPUI 的 `Blurred`：它的模糊视图去掉了系统色调和饱和度，较新的 macOS 上偏好设置、预览这类
//! 窗口几乎没有模糊，桌面直接透出来，文字看不清。系统材质自带色调，跟随窗口的深浅色外观。

use std::ffi::c_void;
use std::io;
use std::ptr::NonNull;

use kwikpaste_core::settings::Material;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSView, NSVisualEffectBlendingMode, NSVisualEffectMaterial,
    NSVisualEffectState, NSVisualEffectView, NSWindowOrderingMode,
};

/// 给 GPUI 窗口放上、更新或移除材质层；窗口本身的透明由宿主经 GPUI `Transparent` / `Opaque` 设置。
///
/// # Safety
/// `ns_view` 必须是仍然存活的 GPUI 窗口视图，并且在主线程调用。
pub unsafe fn apply(ns_view: NonNull<c_void>, material: Material) -> io::Result<()> {
    let view = unsafe { ns_view.cast::<NSView>().as_ref() };
    apply_to_view(view, material).map(|_| ())
}

/// 同 [`apply`]，返回放好的材质层；默认材质返回 `None`。
pub(crate) fn apply_to_view(
    view: &NSView,
    material: Material,
) -> io::Result<Option<Retained<NSVisualEffectView>>> {
    let marker = MainThreadMarker::new()
        .ok_or_else(|| io::Error::other("AppKit calls must run on the main thread"))?;
    let window = view
        .window()
        .ok_or_else(|| io::Error::other("the view has no window"))?;
    let content = window
        .contentView()
        .ok_or_else(|| io::Error::other("the window has no content view"))?;
    let existing = effect_view(&content);
    let effect_material = match material {
        Material::Default => {
            if let Some(effect) = existing {
                effect.removeFromSuperview();
            }
            return Ok(None);
        }
        Material::Mica => NSVisualEffectMaterial::UnderWindowBackground,
        Material::Acrylic => NSVisualEffectMaterial::Popover,
    };

    let effect = existing.unwrap_or_else(|| {
        let effect = NSVisualEffectView::initWithFrame(marker.alloc(), content.bounds());
        effect.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        content.addSubview_positioned_relativeTo(&effect, NSWindowOrderingMode::Below, None);
        effect
    });
    effect.setMaterial(effect_material);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::Active);
    effect.setFrame(content.bounds());
    Ok(Some(effect))
}

/// 内容视图下已经放好的材质层。GPUI 换成 `Transparent` / `Opaque` 时会拿掉它自己的模糊视图，
/// 剩下的 `NSVisualEffectView` 只会是这里放的。
fn effect_view(content: &NSView) -> Option<Retained<NSVisualEffectView>> {
    content
        .subviews()
        .iter()
        .find_map(|view| view.downcast::<NSVisualEffectView>().ok())
}
