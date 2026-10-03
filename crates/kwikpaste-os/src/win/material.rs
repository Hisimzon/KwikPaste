//! 窗口材质（Mica / Acrylic）的 DWM 调用，门槛与 1.x（window-vibrancy）相同，做法按附录 C §7：
//!
//! | 材质 | build ≥ 22523 | 22000–22522 | 17763–21999 | 更早 |
//! |---|---|---|---|---|
//! | mica | `DWMWA_SYSTEMBACKDROP_TYPE(38) = 2` | `DWMWA_MICA_EFFECT(1029) = 1` | 不支持 | 不支持 |
//! | acrylic | `38 = 3`（TRANSIENTWINDOW） | 宿主用 GPUI `Blurred`（accent 4） | 同左 | 不支持 |
//!
//! DWM 背板会被窗口的 accent policy 压掉（GPUI 的 `Transparent` 下的是 accent 2），所以设背板之前
//! 先把 accent 清成 0（[`clear_accent`]）。`DwmGetWindowAttribute(38)` 读回值不可信（属性留着、画面
//! 没了），验收只认截图。
//!
//! 另有：深浅色（`DWMWA_USE_IMMERSIVE_DARK_MODE`，跟随应用主题，GPUI 只会按系统设）、圆角、
//! 系统「透明效果」开关（`EnableTransparency`）和系统深色（`AppsUseLightTheme`）。

use std::ffi::c_void;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DWMSBT_MAINWINDOW, DWMSBT_NONE, DWMSBT_TRANSIENTWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DwmSetWindowAttribute,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::core::{BOOL, s, w};
use windows_registry::{CURRENT_USER, LOCAL_MACHINE};

/// 未公开的 `DWMWA_MICA_EFFECT`（Windows 11 21H2，build 22000–22522）。
const DWMWA_MICA_EFFECT: i32 = 1029;
/// `WCA_ACCENT_POLICY`。
const WCA_ACCENT_POLICY: u32 = 0x13;
const BUILD_MICA: u32 = 22000;
const BUILD_BACKDROP_TYPE: u32 = 22523;
const BUILD_ACRYLIC: u32 = 17763;
const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

/// 当前系统对两种材质的支持（与 1.x 的 `MaterialSupport` 相同的门槛）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterialSupport {
    pub mica: bool,
    pub acrylic: bool,
}

/// 系统 build 号；读不到时为 0（按不支持处理）。
pub fn build() -> u32 {
    LOCAL_MACHINE
        .open(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .and_then(|key| key.get_string("CurrentBuildNumber"))
        .ok()
        .and_then(|build| build.trim().parse().ok())
        .unwrap_or(0)
}

pub fn support(build: u32) -> MaterialSupport {
    MaterialSupport {
        mica: build >= BUILD_MICA,
        acrylic: build >= BUILD_ACRYLIC,
    }
}

/// 系统「设置 → 个性化 → 颜色 → 透明效果」是否打开（读不到时按打开）。
pub fn transparency_enabled() -> bool {
    CURRENT_USER
        .open(PERSONALIZE)
        .and_then(|key| key.get_u32("EnableTransparency"))
        .ok()
        .is_none_or(|value| value != 0)
}

/// 系统应用是否用深色（读不到时按浅色）。
pub fn system_dark() -> bool {
    CURRENT_USER
        .open(PERSONALIZE)
        .and_then(|key| key.get_u32("AppsUseLightTheme"))
        .is_ok_and(|value| value == 0)
}

/// 窗口要的背板。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backdrop {
    /// 不要系统背板（不透明窗口，或用 GPUI `Blurred` 的旧式亚克力）。
    None,
    Mica,
    /// 只在 build ≥ 22523 上走 DWM；更早的系统由宿主改用 GPUI `Blurred`。
    Acrylic,
}

/// Acrylic 在这个 build 上是否走 DWM 背板（否则宿主用 GPUI `Blurred`）。
pub fn acrylic_uses_backdrop(build: u32) -> bool {
    build >= BUILD_BACKDROP_TYPE
}

/// 设背板。Mica / DWM Acrylic 先清 accent（否则被 GPUI 的 accent 2 压掉）；`None` 撤掉之前设过的背板。
pub fn set_backdrop(hwnd: isize, backdrop: Backdrop, build: u32) {
    let window = HWND(hwnd as *mut c_void);
    match backdrop {
        Backdrop::None => {
            if build >= BUILD_BACKDROP_TYPE {
                set_u32(window, DWMWA_SYSTEMBACKDROP_TYPE.0, DWMSBT_NONE.0 as u32);
            } else if build >= BUILD_MICA {
                set_u32(window, DWMWA_MICA_EFFECT, 0);
            }
        }
        Backdrop::Mica => {
            clear_accent(window);
            if build >= BUILD_BACKDROP_TYPE {
                set_u32(
                    window,
                    DWMWA_SYSTEMBACKDROP_TYPE.0,
                    DWMSBT_MAINWINDOW.0 as u32,
                );
            } else {
                set_u32(window, DWMWA_MICA_EFFECT, 1);
            }
        }
        Backdrop::Acrylic => {
            clear_accent(window);
            set_u32(
                window,
                DWMWA_SYSTEMBACKDROP_TYPE.0,
                DWMSBT_TRANSIENTWINDOW.0 as u32,
            );
        }
    }
}

/// 窗口的深浅色（影响 Mica / Acrylic 的底色和系统画的边框）。
pub fn set_dark_mode(hwnd: isize, dark: bool) {
    set_u32(
        HWND(hwnd as *mut c_void),
        DWMWA_USE_IMMERSIVE_DARK_MODE.0,
        u32::from(dark),
    );
}

/// 请求圆角（Windows 11；更早的系统上调用失败，无害）。
pub fn set_round_corners(hwnd: isize) {
    set_u32(
        HWND(hwnd as *mut c_void),
        DWMWA_WINDOW_CORNER_PREFERENCE.0,
        DWMWCP_ROUND.0 as u32,
    );
}

fn set_u32(window: HWND, attribute: i32, value: u32) {
    let result = unsafe {
        DwmSetWindowAttribute(
            window,
            windows::Win32::Graphics::Dwm::DWMWINDOWATTRIBUTE(attribute),
            (&raw const value).cast(),
            size_of::<u32>() as u32,
        )
    };
    if let Err(err) = result {
        log::debug!("DwmSetWindowAttribute({attribute}, {value}) failed: {err}");
    }
}

#[repr(C)]
struct AccentPolicy {
    state: u32,
    flags: u32,
    gradient: u32,
    animation: u32,
}

#[repr(C)]
struct CompositionAttributeData {
    attribute: u32,
    data: *mut c_void,
    size: usize,
}

type SetWindowCompositionAttribute =
    unsafe extern "system" fn(HWND, *mut CompositionAttributeData) -> BOOL;

/// 把 accent policy 清成 `ACCENT_DISABLED`（未公开的 `SetWindowCompositionAttribute`）。
fn clear_accent(window: HWND) {
    let Ok(user32) = (unsafe { GetModuleHandleW(w!("user32.dll")) }) else {
        return;
    };
    let Some(proc) = (unsafe { GetProcAddress(user32, s!("SetWindowCompositionAttribute")) })
    else {
        return;
    };
    let set: SetWindowCompositionAttribute = unsafe { std::mem::transmute(proc) };
    let mut accent = AccentPolicy {
        state: 0,
        flags: 2,
        gradient: 0,
        animation: 0,
    };
    let mut data = CompositionAttributeData {
        attribute: WCA_ACCENT_POLICY,
        data: (&raw mut accent).cast(),
        size: size_of::<AccentPolicy>(),
    };
    if !unsafe { set(window, &mut data) }.as_bool() {
        log::debug!("SetWindowCompositionAttribute(accent 0) failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thresholds_match_1x() {
        assert_eq!(
            support(19045),
            MaterialSupport {
                mica: false,
                acrylic: true
            }
        );
        assert_eq!(
            support(22000),
            MaterialSupport {
                mica: true,
                acrylic: true
            }
        );
        assert_eq!(
            support(17134),
            MaterialSupport {
                mica: false,
                acrylic: false
            }
        );
        assert!(!acrylic_uses_backdrop(22000) && acrylic_uses_backdrop(26100));
    }

    #[test]
    fn this_machine_has_a_build_number() {
        assert!(build() >= 10240);
    }
}
