//! macOS 全局鼠标按键监听。
//!
//! `NSEvent` 的 global monitor 只观察其它应用发出的事件，不吞事件，因此不需要把事件重新注入
//! 系统。监听器和 token 都绑定主线程；调用方在主线程启动一次、设置变化时更新按钮即可。

use std::cell::RefCell;
use std::io;
use std::ptr::NonNull;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSEvent, NSEventMask};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerButton {
    Middle,
    Back,
    Forward,
}

pub enum MouseEvent {
    Trigger,
}

thread_local! {
    static MONITOR: RefCell<Option<Retained<AnyObject>>> = const { RefCell::new(None) };
    static BUTTON: RefCell<Option<TriggerButton>> = const { RefCell::new(None) };
}

/// 安装全局观察器；重复安装会先释放旧 token。
pub fn set_sink(sink: impl Fn(MouseEvent) + 'static) -> io::Result<()> {
    stop();
    let monitor = RcBlock::new(move |event: NonNull<NSEvent>| {
        let number = unsafe { event.as_ref() }.buttonNumber();
        let matched = BUTTON.with(|button| {
            button
                .borrow()
                .is_some_and(|button| matches_number(button, number))
        });
        // 前台是设置列表里的应用时让给它（见 `super::trigger_pause`）。
        if matched && !super::trigger_pause::recheck() {
            sink(MouseEvent::Trigger);
        }
    });
    let token = NSEvent::addGlobalMonitorForEventsMatchingMask_handler(
        NSEventMask::OtherMouseDown,
        &monitor,
    )
    .ok_or_else(|| io::Error::other("NSEvent global mouse monitor could not be installed"))?;
    MONITOR.with(|slot| *slot.borrow_mut() = Some(token));
    Ok(())
}

pub fn set_trigger(button: Option<TriggerButton>) {
    BUTTON.with(|slot| *slot.borrow_mut() = button);
}

pub fn stop() {
    if let Some(token) = MONITOR.with(|slot| slot.borrow_mut().take()) {
        unsafe { NSEvent::removeMonitor(&token) };
    }
}

fn matches_number(button: TriggerButton, number: isize) -> bool {
    match button {
        TriggerButton::Middle => number == 2,
        TriggerButton::Back => number == 3,
        TriggerButton::Forward => number == 4,
    }
}
