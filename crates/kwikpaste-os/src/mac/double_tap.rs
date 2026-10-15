//! 双击修饰键的 NSEvent 观察器；全局和本地事件共用状态，始终原样放行。

use std::cell::{Cell, RefCell};
use std::io;
use std::ptr::NonNull;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use block2::RcBlock;
use kwikpaste_core::settings::{DoubleTap, DoubleTapKey, KeySide};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask, NSEventType};

use crate::double_tap::{Detector, PhysicalKey};

type Sink = Rc<dyn Fn()>;

thread_local! {
    static SINK: RefCell<Option<Sink>> = const { RefCell::new(None) };
    static GLOBAL: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    static LOCAL: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    static DETECTOR: RefCell<Detector> = const { RefCell::new(Detector::new(DoubleTap {
        key: DoubleTapKey::Disabled, side: KeySide::Any,
    })) };
    static SUSPENDED: Cell<bool> = const { Cell::new(false) };
}

#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrusted() -> u8;
}

/// 主线程注册出口；实际观察器只在设置开启时安装。
pub fn set_sink(sink: impl Fn() + 'static) -> io::Result<()> {
    SINK.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return Err(io::Error::other("the double-tap sink is already set"));
        }
        *slot = Some(Rc::new(sink));
        Ok(())
    })
}

/// 主线程更新配置；不请求权限、不弹窗，缺少辅助功能授权只记录一次。
pub fn set_double_tap(config: DoubleTap) -> io::Result<()> {
    DETECTOR.with(|slot| {
        if config.key == DoubleTapKey::Disabled {
            *slot.borrow_mut() = Detector::new(config);
        } else {
            slot.borrow_mut().configure(config);
        }
    });
    if config.key == DoubleTapKey::Disabled {
        stop();
        return Ok(());
    }
    static WARNED: AtomicBool = AtomicBool::new(false);
    if unsafe { AXIsProcessTrusted() } == 0 && !WARNED.swap(true, Ordering::Relaxed) {
        log::warn!(
            "double-tap keyboard events may not arrive without macOS Accessibility permission"
        );
    }
    if GLOBAL.with(|slot| slot.borrow().is_some()) {
        return Ok(());
    }
    let mask = NSEventMask::FlagsChanged | NSEventMask::KeyDown;
    let global = RcBlock::new(|event: NonNull<NSEvent>| observe(unsafe { event.as_ref() }));
    let global =
        NSEvent::addGlobalMonitorForEventsMatchingMask_handler(mask, &global).ok_or_else(|| {
            io::Error::other("NSEvent global keyboard monitor could not be installed")
        })?;
    let local = RcBlock::new(|event: NonNull<NSEvent>| {
        observe(unsafe { event.as_ref() });
        event.as_ptr()
    });
    let Some(local) =
        (unsafe { NSEvent::addLocalMonitorForEventsMatchingMask_handler(mask, &local) })
    else {
        unsafe { NSEvent::removeMonitor(&global) };
        return Err(io::Error::other(
            "NSEvent local keyboard monitor could not be installed",
        ));
    };
    GLOBAL.with(|slot| *slot.borrow_mut() = Some(global));
    LOCAL.with(|slot| *slot.borrow_mut() = Some(local));
    Ok(())
}

/// 录制快捷键期间停用双击，并丢弃未完成手势。
pub fn suspend_double_tap(suspended: bool) {
    SUSPENDED.with(|slot| slot.set(suspended));
    DETECTOR.with(|slot| slot.borrow_mut().reset());
}

fn stop() {
    for token in [
        GLOBAL.with(|slot| slot.borrow_mut().take()),
        LOCAL.with(|slot| slot.borrow_mut().take()),
    ]
    .into_iter()
    .flatten()
    {
        unsafe { NSEvent::removeMonitor(&token) };
    }
}

fn observe(event: &NSEvent) {
    let suspended = SUSPENDED.with(Cell::get);
    let fire = DETECTOR.with(|slot| {
        let Ok(mut detector) = slot.try_borrow_mut() else {
            return false;
        };
        if event.r#type() == NSEventType::KeyDown {
            detector.reset();
            return false;
        }
        let Some((physical, flag)) = physical_key(event.keyCode()) else {
            detector.reset();
            return false;
        };
        let down = event.modifierFlags().bits() & flag != 0;
        let fire = detector.modifier(physical, down, (event.timestamp() * 1000.0) as u64);
        if suspended || super::trigger_pause::is_paused() {
            detector.reset();
            return false;
        }
        fire
    });
    if fire && !super::trigger_pause::recheck() {
        let sink = SINK.with(|slot| slot.try_borrow().ok().and_then(|sink| sink.clone()));
        if let Some(sink) = sink {
            sink();
        }
    }
}

/// NX_DEVICEL*/NX_DEVICER* 位区分左右；不能用合并的 NSEventModifierFlags 判断松开。
fn physical_key(code: u16) -> Option<(PhysicalKey, usize)> {
    let (key, side, flag) = match code {
        59 => (DoubleTapKey::Control, KeySide::Left, 0x0001),
        62 => (DoubleTapKey::Control, KeySide::Right, 0x2000),
        56 => (DoubleTapKey::Shift, KeySide::Left, 0x0002),
        60 => (DoubleTapKey::Shift, KeySide::Right, 0x0004),
        58 => (DoubleTapKey::Alt, KeySide::Left, 0x0020),
        61 => (DoubleTapKey::Alt, KeySide::Right, 0x0040),
        55 => (DoubleTapKey::Command, KeySide::Left, 0x0008),
        54 => (DoubleTapKey::Command, KeySide::Right, 0x0010),
        _ => return None,
    };
    Some((PhysicalKey { key, side }, flag))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_codes_use_independent_left_and_right_flags() {
        for (left, right, key) in [
            (59, 62, DoubleTapKey::Control),
            (56, 60, DoubleTapKey::Shift),
            (58, 61, DoubleTapKey::Alt),
            (55, 54, DoubleTapKey::Command),
        ] {
            let (l, lb) = physical_key(left).unwrap();
            let (r, rb) = physical_key(right).unwrap();
            assert_eq!(
                l,
                PhysicalKey {
                    key,
                    side: KeySide::Left
                }
            );
            assert_eq!(
                r,
                PhysicalKey {
                    key,
                    side: KeySide::Right
                }
            );
            assert_eq!(lb & rb, 0);
        }
        assert!(physical_key(0).is_none());
    }
}
