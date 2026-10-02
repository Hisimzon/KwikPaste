//! macOS 的显示器几何。

use objc2::MainThreadMarker;
use objc2_app_kit::NSScreen;

/// 一块显示器的范围（point，左上角为原点、向下为正，与 `CGDisplayBounds` 相同）与缩放。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScreenFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    /// `backingScaleFactor`。
    pub scale: f64,
}

/// 全部显示器。`NSScreen` 只能在主线程上读；不在主线程时返回空。
pub fn screens() -> Vec<ScreenFrame> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Vec::new();
    };
    let screens = NSScreen::screens(mtm);
    // NSScreen 的 frame 以主屏左下角为原点、向上为正；按主屏高度翻成左上角原点。
    let Some(primary_height) = screens
        .iter()
        .next()
        .map(|screen| screen.frame().size.height)
    else {
        return Vec::new();
    };

    screens
        .iter()
        .map(|screen| {
            let frame = screen.frame();
            ScreenFrame {
                x: frame.origin.x,
                y: primary_height - frame.origin.y - frame.size.height,
                width: frame.size.width,
                height: frame.size.height,
                scale: screen.backingScaleFactor(),
            }
        })
        .collect()
}
