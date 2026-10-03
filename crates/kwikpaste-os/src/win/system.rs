//! 界面要跟随的系统设置：文本大小、高对比度、动画效果，以及它们的变化通知。
//!
//! - 文本大小：「设置 → 辅助功能 → 文本大小」（100%–225%）。优先读 WinRT `UISettings`，
//!   它同时提供变化事件；拿不到时退回注册表，与 1.x 相同。
//! - 高对比度：`SPI_GETHIGHCONTRAST` 的 `HCF_HIGHCONTRASTON`。
//! - 减少动画：`SPI_GETCLIENTAREAANIMATION` 关闭即视为要求减少动画（与 gpui-base 的读法相同）。
//! - 透明效果、系统深色：见 [`super::material`]，材质跟随它们。
//!
//! 变化来源：`UISettings.TextScaleFactorChanged`，以及面板窗口收到的 `WM_SETTINGCHANGE` /
//! `WM_SYSCOLORCHANGE`（面板子类过程转给 [`notify_changed`]）。出口只说“可能变了”，宿主重新 [`read`]。

use std::io;
use std::sync::OnceLock;

use windows::Foundation::TypedEventHandler;
use windows::UI::ViewManagement::UISettings;
use windows::Win32::UI::Accessibility::{HCF_HIGHCONTRASTON, HIGHCONTRASTW};
use windows::Win32::UI::WindowsAndMessaging::{
    SPI_GETCLIENTAREAANIMATION, SPI_GETHIGHCONTRAST, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS,
    SystemParametersInfoW,
};

use super::monitor;

/// 一次读到的系统设置。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SystemSettings {
    /// 1.0–2.25。
    pub text_scale: f64,
    pub high_contrast: bool,
    pub reduce_motion: bool,
    /// 系统「透明效果」打开。
    pub transparency: bool,
    /// 系统应用用深色。
    pub dark: bool,
}

type Sink = Box<dyn Fn() + Send + Sync>;

static SINK: OnceLock<Sink> = OnceLock::new();

/// 读取当前值。
pub fn read() -> SystemSettings {
    SystemSettings {
        text_scale: text_scale_factor(),
        high_contrast: high_contrast(),
        reduce_motion: reduce_motion(),
        transparency: super::material::transparency_enabled(),
        dark: super::material::system_dark(),
    }
}

/// 文本大小系数，钳到 1.0–2.25。
pub fn text_scale_factor() -> f64 {
    UISettings::new()
        .and_then(|settings| settings.TextScaleFactor())
        .map(|factor| factor.clamp(1.0, 2.25))
        .unwrap_or_else(|_| monitor::text_scale_factor())
}

fn high_contrast() -> bool {
    let mut info = HIGHCONTRASTW {
        cbSize: size_of::<HIGHCONTRASTW>() as u32,
        ..Default::default()
    };
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETHIGHCONTRAST,
            info.cbSize,
            Some((&mut info as *mut HIGHCONTRASTW).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_ok() && info.dwFlags.contains(HCF_HIGHCONTRASTON)
}

fn reduce_motion() -> bool {
    let mut animations = windows::core::BOOL(1);
    let read = unsafe {
        SystemParametersInfoW(
            SPI_GETCLIENTAREAANIMATION,
            0,
            Some((&mut animations as *mut windows::core::BOOL).cast()),
            SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
        )
    };
    read.is_ok() && !animations.as_bool()
}

/// 订阅变化。返回的句柄要一直持有，丢弃即退订文本大小事件。
pub struct Watch {
    settings: Option<(UISettings, i64)>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        if let Some((settings, token)) = self.settings.take() {
            let _ = settings.RemoveTextScaleFactorChanged(token);
        }
    }
}

/// 设置变化出口并订阅文本大小事件，进程内只调用一次。出口可能在任意线程调用，不得阻塞。
pub fn watch(sink: impl Fn() + Send + Sync + 'static) -> io::Result<Watch> {
    SINK.set(Box::new(sink))
        .map_err(|_| io::Error::other("the system settings sink is already set"))?;

    let subscribed = UISettings::new().and_then(|settings| {
        let token = settings.TextScaleFactorChanged(&TypedEventHandler::new(|_, _| {
            notify_changed();
            Ok(())
        }))?;
        Ok((settings, token))
    });
    let settings = match subscribed {
        Ok(subscription) => Some(subscription),
        Err(err) => {
            log::warn!("text size changes are not observable ({err}); only WM_SETTINGCHANGE is");
            None
        }
    };

    Ok(Watch { settings })
}

/// 系统设置可能变了：转给 [`watch`] 的出口。
pub fn notify_changed() {
    if let Some(sink) = SINK.get() {
        sink();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_values_in_range() {
        let settings = read();

        assert!((1.0..=2.25).contains(&settings.text_scale));
    }
}
