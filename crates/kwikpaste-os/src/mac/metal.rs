//! Metal 设备移除监听；回调只置状态并通知宿主，不碰 GPUI。

use std::cell::RefCell;
use std::io;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_metal::{
    MTLCopyAllDevicesWithObserver, MTLDevice, MTLDeviceNotificationName,
    MTLDeviceWasRemovedNotification, MTLRemoveDeviceObserver,
};

static DEVICE_REMOVED: AtomicBool = AtomicBool::new(false);

thread_local! {
    static DEVICE_OBSERVER: RefCell<Option<DeviceObserver>> = const { RefCell::new(None) };
}

struct DeviceObserver(Retained<ProtocolObject<dyn NSObjectProtocol>>);

impl Drop for DeviceObserver {
    fn drop(&mut self) {
        // SAFETY: Metal returned this observer; unregister before releasing our retained token.
        unsafe { MTLRemoveDeviceObserver(&self.0) };
    }
}

/// 注册一次 Metal 观察者；通知可能来自任意线程，宿主回调必须仅发送信号且不得 panic。
pub fn watch_device_removed(on_removed: impl Fn() + Send + Sync + 'static) -> io::Result<()> {
    DEVICE_OBSERVER.with(|slot| {
        if slot.borrow().is_some() {
            return Ok(());
        }

        let block = RcBlock::new(
            move |_device: NonNull<ProtocolObject<dyn MTLDevice>>,
                  name: NonNull<MTLDeviceNotificationName>| {
                // SAFETY: Metal keeps both callback arguments alive for the duration of the call.
                if unsafe {
                    name.as_ref()
                        .isEqualToString(MTLDeviceWasRemovedNotification)
                } {
                    DEVICE_REMOVED.store(true, Ordering::Release);
                    on_removed();
                }
            },
        );
        let mut observer = std::ptr::null_mut();
        // SAFETY: The out pointer lives for the call and Metal copies the sendable block.
        let _devices = unsafe {
            MTLCopyAllDevicesWithObserver(NonNull::from(&mut observer), RcBlock::as_ptr(&block))
        };
        let observer = unsafe { Retained::from_raw(observer) }
            .ok_or_else(|| io::Error::other("Metal did not return a device observer"))?;
        *slot.borrow_mut() = Some(DeviceObserver(observer));
        Ok(())
    })
}

/// 设备一旦移除，就要求宿主有序重启；当前 GPUI 没有公开 Metal renderer 重建接口。
pub fn device_removed() -> bool {
    DEVICE_REMOVED.load(Ordering::Acquire)
}

/// Request the next health tick to recheck the current Metal renderer/device.
pub fn request_device_recheck() {
    DEVICE_REMOVED.store(true, Ordering::Release);
}

/// 自测状态注入；宿主仍必须同时检查自测参数与环境变量。
pub fn simulate_device_removed() {
    request_device_recheck();
}
