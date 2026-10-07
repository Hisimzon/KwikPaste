//! macOS 前台应用让出全局触发：前台应用在设置 `shortcuts.pauseAppIds` 列表里时 [`is_paused`] 为真，
//! 宿主注销全局热键，鼠标按键唤起不响应。跟随 NSWorkspace 的应用激活通知（只在 NSWorkspace 自己的
//! 通知中心发出，默认通知中心收不到）。
//!
//! TODO: 全屏判断（`shortcuts.pauseInFullscreen`）尚未实现，偏好设置里暂不显示这一项；实现时按
//! `CGWindowListCopyWindowInfo` 比对前台应用的 layer 0 窗口与所在屏幕，并跟随
//! `NSWorkspaceActiveSpaceDidChangeNotification`。

use std::cell::RefCell;
use std::io;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use block2::RcBlock;
use kwikpaste_core::app_ids::contains_app;
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{NSWorkspace, NSWorkspaceDidActivateApplicationNotification};
use objc2_foundation::NSNotification;

type Sink = Box<dyn Fn() + Send + Sync>;

static SINK: OnceLock<Sink> = OnceLock::new();
static PAUSED: AtomicBool = AtomicBool::new(false);
static APP_IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

thread_local! {
    static OBSERVER: RefCell<Option<Retained<ProtocolObject<dyn NSObjectProtocol>>>> =
        const { RefCell::new(None) };
}

/// 设置暂停状态变化的出口，进程内只设一次；它只是通知，当前状态以 [`is_paused`] 为准。
pub fn set_sink(sink: impl Fn() + Send + Sync + 'static) -> io::Result<()> {
    SINK.set(Box::new(sink))
        .map_err(|_| io::Error::other("the trigger pause sink is already set"))
}

/// 按设置更新应用列表并立即重查。在主线程调用，首次调用时装上应用激活通知。
pub fn configure(_fullscreen: bool, app_ids: Vec<String>) -> io::Result<()> {
    *APP_IDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = app_ids;
    install_observer();
    recheck();

    Ok(())
}

pub fn is_paused() -> bool {
    PAUSED.load(Ordering::SeqCst)
}

/// 重查前台应用，返回这次是否让给它；鼠标按键唤起到达时也调用。
pub fn recheck() -> bool {
    let app_ids = APP_IDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let listed = !app_ids.is_empty() && frontmost_is_listed(&app_ids);
    set_paused(listed);

    listed
}

fn install_observer() {
    OBSERVER.with(|slot| {
        if slot.borrow().is_some() {
            return;
        }
        let block = RcBlock::new(|_notification: NonNull<NSNotification>| {
            recheck();
        });
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let observer = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidActivateApplicationNotification),
                None,
                None,
                &block,
            )
        };
        *slot.borrow_mut() = Some(observer);
    });
}

fn frontmost_is_listed(app_ids: &[String]) -> bool {
    autoreleasepool(|_| {
        let Some(app) = NSWorkspace::sharedWorkspace().frontmostApplication() else {
            return false;
        };
        if i64::from(app.processIdentifier()) == i64::from(std::process::id()) {
            return false;
        }
        app.bundleIdentifier()
            .map(|id| id.to_string())
            .is_some_and(|id| contains_app(app_ids, &id))
    })
}

fn set_paused(paused: bool) {
    if PAUSED.swap(paused, Ordering::SeqCst) == paused {
        return;
    }
    if paused {
        log::info!("global triggers paused: listed app");
    } else {
        log::info!("global triggers resumed");
    }
    if let Some(sink) = SINK.get() {
        sink();
    }
}
