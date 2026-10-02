//! macOS 面板胶水：从 GPUI 窗口取 `NSView`，交给 `kwikpaste_os::mac::panel`。
//!
//! 最小实现，只靠 CI 编译和冒烟验证；缺的面板配置见 `kwikpaste_os::mac::panel` 的 TODO(macOS)。

use anyhow::{anyhow, bail};
use gpui::Window;
use kwikpaste_os::mac::panel as mac_panel;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// macOS 上定位不需要回读校验。
pub struct Placement;

pub struct NativePanel {
    panel: mac_panel::Panel,
}

impl NativePanel {
    pub fn attach(window: &Window) -> anyhow::Result<Self> {
        let handle = HasWindowHandle::window_handle(window)
            .map_err(|err| anyhow!("panel window handle: {err:?}"))?;
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            bail!("the panel is not an AppKit window");
        };

        Ok(Self {
            // GPUI 在主线程创建窗口，面板永不销毁。
            panel: unsafe { mac_panel::Panel::from_raw(handle.ns_view) },
        })
    }

    pub fn install(&self) -> anyhow::Result<()> {
        Ok(())
    }

    pub fn is_visible(&self) -> bool {
        self.panel.is_visible()
    }

    pub fn place_near_cursor(&self) -> anyhow::Result<Placement> {
        self.panel.place_near_cursor()?;
        Ok(Placement)
    }

    pub fn show(&self) {
        self.panel.show_without_activating();
    }

    pub fn verify(&self, _: &Placement) {}

    pub fn raise(&self) {
        self.panel.show_without_activating();
    }

    pub fn hide(&self) {
        self.panel.hide();
    }

    /// 自测探针只在 Windows 上判定，macOS 不附加原生字段。
    pub fn probe_fields(&self, _: Option<&Placement>) -> String {
        String::new()
    }
}
