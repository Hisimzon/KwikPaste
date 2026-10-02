//! Windows 面板胶水：从 GPUI 窗口取 HWND，几何计算和原生调用交给 `kwikpaste_os::win`。

use std::cell::Cell;

use anyhow::{Context as _, anyhow, bail};
use gpui::Window;
use kwikpaste_os::geometry::{Rect, follow_cursor, scale_size};
use kwikpaste_os::win::monitor::{self, BASE_DPI, MonitorInfo};
use kwikpaste_os::win::panel::{self as win_panel, PanelOptions};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::panel::PANEL_SIZE;

/// 一次显示算出并写入的几何。
pub struct Placement {
    pub client: Rect,
    pub outer: Rect,
    pub dpi: u32,
}

pub struct NativePanel {
    panel: win_panel::Panel,
    /// 上次隐藏时的内容区尺寸（逻辑像素，已含文本缩放），用户拉伸过就沿用。
    logical_size: Cell<Option<(f64, f64)>>,
}

impl NativePanel {
    pub fn attach(window: &Window) -> anyhow::Result<Self> {
        let handle = HasWindowHandle::window_handle(window)
            .map_err(|err| anyhow!("panel window handle: {err:?}"))?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            bail!("the panel is not a Win32 window");
        };

        Ok(Self {
            // GPUI 在主线程创建窗口，面板永不销毁。
            panel: unsafe { win_panel::Panel::from_raw(handle.hwnd.get()) },
            logical_size: Cell::new(None),
        })
    }

    pub fn install(&self) -> anyhow::Result<()> {
        self.panel.install(PanelOptions {
            min_logical_size: PANEL_SIZE,
        })?;
        Ok(())
    }

    pub fn is_visible(&self) -> bool {
        self.panel.is_visible()
    }

    /// 光标所在显示器、光标附近；取不到光标时放到主显示器中央。
    pub fn place_near_cursor(&self) -> anyhow::Result<Placement> {
        let monitor = monitor::at_cursor()
            .or_else(|err| {
                log::warn!("cursor monitor unavailable ({err}); using the primary monitor");
                monitor::primary()
            })
            .context("no monitor to show the panel on")?;
        let client = self.target_client_rect(&monitor);
        let outer = self.panel.place(client, monitor.dpi)?;

        Ok(Placement {
            client,
            outer,
            dpi: monitor.dpi,
        })
    }

    fn target_client_rect(&self, monitor: &MonitorInfo) -> Rect {
        let logical = self.logical_size.get().unwrap_or_else(|| {
            let text_scale = monitor::text_scale_factor();
            (PANEL_SIZE.0 * text_scale, PANEL_SIZE.1 * text_scale)
        });
        let size = scale_size(logical, monitor.scale());

        follow_cursor(monitor.cursor, monitor.work_area, size)
    }

    pub fn show(&self) {
        self.panel.show_without_activating();
    }

    /// 显示后外框必须等于写入的矩形；不等（例如 DPI 变化改了尺寸）就记错误并重放一次。
    pub fn verify(&self, placement: &Placement) {
        match self.panel.window_rect() {
            Ok(rect) if rect == placement.outer => {}
            Ok(rect) => {
                log::error!(
                    "panel rect {rect:?} differs from the target {:?}; placing it again",
                    placement.outer
                );
                if let Err(err) = self.panel.place(placement.client, placement.dpi) {
                    log::error!("panel could not be placed again: {err}");
                }
            }
            Err(err) => log::warn!("panel rect is unreadable after show: {err}"),
        }
    }

    pub fn raise(&self) {
        if let Err(err) = self.panel.raise() {
            log::warn!("panel could not be raised: {err}");
        }
    }

    pub fn hide(&self) {
        if let Ok(client) = self.panel.client_rect() {
            let scale = f64::from(self.panel.dpi()) / f64::from(BASE_DPI);
            self.logical_size.set(Some((
                f64::from(client.width()) / scale,
                f64::from(client.height()) / scale,
            )));
        }
        self.panel.hide();
    }

    /// 自测探针用的原生状态（JSON 对象的字段片段，以逗号开头）。
    pub fn probe_fields(&self, placement: Option<&Placement>) -> String {
        let counters = win_panel::counters();
        let mut fields = format!(
            r#","hwnd":{},"visible":{},"foreground":{},"dpi":{},"phantom_activations":{},"mouse_activate_replies":{:?},"mouse_activate_overrides":{}"#,
            self.panel.raw(),
            self.panel.is_visible(),
            kwikpaste_os::win::foreground_window(),
            self.panel.dpi(),
            counters.phantom_activations,
            counters.mouse_activate_replies,
            counters.mouse_activate_overrides,
        );
        if let Ok(rect) = self.panel.window_rect() {
            fields.push_str(&format!(r#","window_rect":{}"#, json_rect(rect)));
        }
        if let Ok(rect) = self.panel.client_rect() {
            fields.push_str(&format!(r#","client_rect":{}"#, json_rect(rect)));
        }
        if let Some(placement) = placement {
            fields.push_str(&format!(
                r#","target_client":{},"target_outer":{},"target_dpi":{}"#,
                json_rect(placement.client),
                json_rect(placement.outer),
                placement.dpi,
            ));
        }

        fields
    }
}

fn json_rect(rect: Rect) -> String {
    format!(
        "[{},{},{},{}]",
        rect.left, rect.top, rect.right, rect.bottom
    )
}
