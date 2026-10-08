//! 拖动窗口的空白区域。

use gpui::{InteractiveElement, WindowControlArea};

/// 把元素标成拖动窗口的区域。Windows 上 GPUI 按 `WindowControlArea::Drag` 做命中测试（当标题栏）；
/// macOS 的 GPUI 不做这项命中测试，按下时自己交给 AppKit 拖动窗口。
///
/// 只用在不含可点控件的空白处：macOS 上按下就开始拖，子元素收不到这次点击。
pub trait WindowDragArea: InteractiveElement {
    fn window_drag_area(self) -> Self {
        let element = self.window_control_area(WindowControlArea::Drag);
        #[cfg(target_os = "macos")]
        let element = element.on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
            window.start_window_move();
        });
        element
    }
}

impl<E: InteractiveElement> WindowDragArea for E {}
