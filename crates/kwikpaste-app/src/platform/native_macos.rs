//! macOS 面板胶水：从 GPUI 窗口取 `NSView`，交给 `kwikpaste_os::mac::panel`。
//!
//! 最小实现，只靠 CI 编译和冒烟验证；缺的面板配置见 `kwikpaste_os::mac::panel` 的 TODO(macOS)。
//! macOS 不需要键盘、鼠标钩子：非激活 NSPanel 成为 key 窗口就能收键盘，失焦由
//! `windowDidResignKey` 通知（TODO(macOS)：显示时 `makeKeyWindow`，失焦自动隐藏）。

use std::cell::Cell;

use anyhow::{anyhow, bail};
use gpui::Window;
use kwikpaste_os::mac::panel as mac_panel;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use kwikpaste_core::window_state::WindowGeometry;

use super::editing::{EditReport, EditTrigger};
use super::window_state::PanelLayout;

/// macOS 上定位不需要回读校验。
pub struct Placement;

pub struct NativePanel {
    panel: mac_panel::Panel,
    editing: Cell<bool>,
}

impl NativePanel {
    pub fn attach(window: &Window, _text_scale: f64) -> anyhow::Result<Self> {
        let handle = HasWindowHandle::window_handle(window)
            .map_err(|err| anyhow!("panel window handle: {err:?}"))?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            bail!("the panel is not an AppKit window");
        };

        Ok(Self {
            // GPUI 在主线程创建窗口，面板永不销毁。
            panel: unsafe { mac_panel::Panel::from_raw(handle.ns_view) },
            editing: Cell::new(false),
        })
    }

    pub fn install(&self) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn is_visible(&self) -> bool {
        self.panel.is_visible()
    }

    /// 只有 Windows 的自测探针用到窗口句柄。
    pub fn raw_handle(&self) -> isize {
        0
    }

    /// macOS 没有「文本大小」设置。
    pub fn set_text_scale(&self, _text_scale: f64) {}

    /// TODO(macOS)：按 `layout` 的摆放方式和存档尺寸定位；现在总是跟随光标、默认尺寸。
    pub fn place(&self, _layout: &PanelLayout) -> anyhow::Result<Placement> {
        self.panel.place_near_cursor()?;
        Ok(Placement)
    }

    pub fn show(&self) {
        self.panel.show_without_activating();
    }

    pub fn start_hooks(&self) {}

    pub fn stop_hooks(&self) {}

    pub fn verify(&self, _: &Placement) {}

    pub fn raise(&self) {
        self.panel.show_without_activating();
    }

    /// TODO(macOS)：返回隐藏前的几何供存档。
    pub fn hide(&self) -> Option<WindowGeometry> {
        self.panel.hide();
        None
    }

    pub fn is_editing(&self) -> bool {
        self.editing.get()
    }

    /// 非激活面板不需要抢前台；TODO(macOS)：确认面板是 key 窗口后再报告成功。
    pub fn begin_editing(&self, _trigger: EditTrigger) -> anyhow::Result<EditReport> {
        self.editing.set(true);
        Ok(EditReport::default())
    }

    pub fn end_editing(&self, _restore_foreground: bool) {
        self.editing.set(false);
    }

    /// 自测探针只在 Windows 上判定，macOS 不附加原生字段。
    pub fn probe_fields(&self, _: Option<&Placement>) -> String {
        String::new()
    }
}
