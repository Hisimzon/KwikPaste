//! macOS 的按键注入（1.x `keystroke/macos.rs`），CGEvent 换成 objc2-core-graphics。

use std::io;

use objc2_app_kit::{NSEvent, NSEventModifierFlags};
use objc2_core_graphics::{
    CGEvent, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation, CGKeyCode,
};

/// kVK_ANSI_V（HIToolbox/Events.h），与键盘布局无关的硬件键码。
const KEY_V: CGKeyCode = 0x09;

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

/// 检测辅助功能授权，未授权时打开系统设置并返回可展示给用户的权限错误。
pub fn ensure_accessibility_trusted() -> io::Result<()> {
    let trusted = unsafe { AXIsProcessTrusted() != 0 };
    if trusted {
        log::debug!("macOS Accessibility permission is available");
        return Ok(());
    }

    log::warn!("macOS Accessibility permission is missing; opening System Settings");
    if let Err(err) = super::permissions::open_accessibility_settings() {
        log::warn!("could not open macOS Accessibility settings: {err}");
    }
    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "Accessibility permission is required to paste; enable KwikPaste in System Settings > Privacy & Security > Accessibility",
    ))
}

pub fn simulate_paste() -> io::Result<()> {
    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .ok_or_else(|| io::Error::other("CGEventSource could not be created"))?;
    for key_down in [true, false] {
        let event = CGEvent::new_keyboard_event(Some(&source), KEY_V, key_down)
            .ok_or_else(|| io::Error::other("the ⌘V keyboard event could not be created"))?;
        CGEvent::set_flags(Some(&event), CGEventFlags::MaskCommand);
        CGEvent::post(CGEventTapLocation::HIDEventTap, Some(&event));
    }

    Ok(())
}

pub fn mask_modifier_release() -> io::Result<()> {
    Ok(())
}

pub fn modifiers_pressed() -> bool {
    let modifiers = NSEventModifierFlags::Command
        | NSEventModifierFlags::Control
        | NSEventModifierFlags::Option
        | NSEventModifierFlags::Shift;

    NSEvent::modifierFlags_class().intersects(modifiers)
}
