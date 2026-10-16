//! 窗口材质（Mica / 磨砂）的 DWM 调用，门槛与 1.x（window-vibrancy）相同，做法按附录 C §7：
//!
//! | 材质 | build ≥ 22523 | 22000–22522 | 17763–21999 | 更早 |
//! |---|---|---|---|---|
//! | mica | `DWMWA_SYSTEMBACKDROP_TYPE(38) = 2` | `DWMWA_MICA_EFFECT(1029) = 1` | 不支持 | 不支持 |
//! | 磨砂 | accent 4（`ACCENT_ENABLE_ACRYLICBLURBEHIND`） | 同左 | 同左 | 不支持 |
//!
//! 磨砂用旧式的 accent 亚克力（与 QcClip 相同），不用系统亚克力背板（`DWMSBT_TRANSIENTWINDOW`）：
//! 后者的亮度层和噪点调不了，整体发灰。accent 4 的色调可以自己给：浅色给一层白抵消灰雾，深色几乎
//! 不加色调。纯高斯模糊（accent 3）在浅色下颜色太艳、次要文字看不清，也不用。窗口尺寸变化后
//! accent 4 的模糊区域不会跟着更新，所以缩放后要重设（[`set_backdrop`] 装的子类负责）。
//!
//! DWM 背板会被窗口的 accent policy 压掉（GPUI 的 `Transparent` 下的是 accent 2），所以设 Mica 之前
//! 先把 accent 清成 0（[`clear_accent`]）。`DwmGetWindowAttribute(38)` 读回值不可信（属性留着、画面
//! 没了），验收只认截图。
//!
//! DWM 只给「框架活动」的窗口画 Mica，非活动窗口只画一块平涂底色。
//! 面板和预览这类从不激活的浮层用 [`keep_popup_frame_active`] 把框架状态一直留在活动。
//!
//! 另有：深浅色（`DWMWA_USE_IMMERSIVE_DARK_MODE`，跟随应用主题，GPUI 只会按系统设）、圆角、
//! 系统「透明效果」开关（`EnableTransparency`）和系统深色（`AppsUseLightTheme`）。

use std::ffi::c_void;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMSBT_MAINWINDOW, DWMSBT_NONE, DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
};
use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GetWindowLongPtrW, PostMessageW, SIZE_MINIMIZED, SendMessageW, WM_APP,
    WM_NCACTIVATE, WM_NCDESTROY, WM_SHOWWINDOW, WM_SIZE, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, s, w};
use windows_registry::{CURRENT_USER, LOCAL_MACHINE};

/// 未公开的 `DWMWA_MICA_EFFECT`（Windows 11 21H2，build 22000–22522）。
const DWMWA_MICA_EFFECT: i32 = 1029;
/// `WCA_ACCENT_POLICY`。
const WCA_ACCENT_POLICY: u32 = 0x13;
const BUILD_MICA: u32 = 22000;
const BUILD_BACKDROP_TYPE: u32 = 22523;
const BUILD_FROSTED: u32 = 17763;
const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";
/// [`keep_popup_frame_active`] 的子类 id（"KPMA"）。
const ACTIVE_FRAME_SUBCLASS: usize = 0x4b50_4d41;
/// 磨砂窗口缩放后重设 accent 的子类 id（"KPFR"）。
const FROSTED_SUBCLASS: usize = 0x4b50_4652;
/// `ACCENT_ENABLE_ACRYLICBLURBEHIND`。
const ACCENT_ACRYLIC: u32 = 4;
/// 浅色磨砂的色调：55% 白（ABGR），抵消 accent 亚克力的灰雾。
const FROSTED_TINT_LIGHT: u32 = (140 << 24) | 0x00ff_ffff;
/// 深色磨砂几乎不加色调；alpha 为 0 时 accent 亚克力不生效，所以给 1。
const FROSTED_TINT_DARK: u32 = 1 << 24;
/// 失活走完之后把框架重新置为活动的私有消息，只投递给窗口自己。
const WM_REASSERT_ACTIVE_FRAME: u32 = WM_APP + 0x4d;

/// 当前系统对两种材质的支持（与 1.x 的 `MaterialSupport` 相同的门槛）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MaterialSupport {
    pub mica: bool,
    pub frosted: bool,
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
        frosted: build >= BUILD_FROSTED,
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
    /// 不要系统背板（不透明窗口）。
    None,
    Mica,
    /// 磨砂：accent 亚克力，背后是模糊的窗口内容；`dark` 决定色调。
    Frosted {
        dark: bool,
    },
}

/// 设背板，在 GPUI 设完 accent 之后调用。Mica 先清 accent（否则被 GPUI 的 accent 2 压掉）；
/// 磨砂先撤掉 DWM 背板再设 accent，并装上缩放后重设的子类；`None` 撤掉之前设过的背板。
pub fn set_backdrop(hwnd: isize, backdrop: Backdrop, build: u32) {
    let window = HWND(hwnd as *mut c_void);
    if !matches!(backdrop, Backdrop::Frosted { .. }) {
        let _ = unsafe { RemoveWindowSubclass(window, Some(frosted_proc), FROSTED_SUBCLASS) };
    }
    match backdrop {
        Backdrop::None => clear_system_backdrop(window, build),
        Backdrop::Frosted { dark } => {
            clear_system_backdrop(window, build);
            let tint = if dark {
                FROSTED_TINT_DARK
            } else {
                FROSTED_TINT_LIGHT
            };
            set_accent(window, ACCENT_ACRYLIC, 0, tint);
            // 子类的数据就是色调；重复安装只更新数据。
            let installed = unsafe {
                SetWindowSubclass(window, Some(frosted_proc), FROSTED_SUBCLASS, tint as usize)
            };
            if !installed.as_bool() {
                log::debug!("frosted resize subclass could not be installed");
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
    }
}

/// 磨砂窗口的子类：尺寸变化（最小化除外）后按存下的色调重设 accent。
unsafe extern "system" fn frosted_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    tint: usize,
) -> LRESULT {
    let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    match msg {
        WM_SIZE if wparam.0 != SIZE_MINIMIZED as usize => {
            set_accent(hwnd, ACCENT_ACRYLIC, 0, tint as u32);
        }
        WM_NCDESTROY => {
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(frosted_proc), FROSTED_SUBCLASS) };
        }
        _ => {}
    }
    result
}

fn clear_system_backdrop(window: HWND, build: u32) {
    if build >= BUILD_BACKDROP_TYPE {
        set_u32(window, DWMWA_SYSTEMBACKDROP_TYPE.0, DWMSBT_NONE.0 as u32);
    } else if build >= BUILD_MICA {
        set_u32(window, DWMWA_MICA_EFFECT, 0);
    }
}

/// 让从不激活的浮层窗口（`WS_EX_TOOLWINDOW`：面板、预览）一直按活动窗口画背板：子类把
/// `WM_NCACTIVATE(FALSE)` 改成 TRUE，并立即发一次 TRUE。普通窗口（偏好设置等）不处理，失焦时照系统
/// 惯例回退成纯色。可重复调用：同一个子类只装一次。
///
/// 真被激活过的浮层（面板编辑态）失活时，系统在 `WM_NCACTIVATE(FALSE)` 处理完之后仍会把框架清成
/// 非活动，改写挡不住；子类因此在失活后投递一条消息，等这次失活走完再置回活动，每次显示前也再置一次。
pub fn keep_popup_frame_active(hwnd: isize) {
    let window = HWND(hwnd as *mut c_void);
    let ex_style = unsafe { GetWindowLongPtrW(window, GWL_EXSTYLE) };
    if ex_style & WS_EX_TOOLWINDOW.0 as isize == 0 {
        return;
    }
    let installed =
        unsafe { SetWindowSubclass(window, Some(active_frame_proc), ACTIVE_FRAME_SUBCLASS, 0) };
    if !installed.as_bool() {
        log::debug!("active frame subclass could not be installed");
        return;
    }
    // lParam = -1：只改框架状态，不重画非客户区。
    unsafe { SendMessageW(window, WM_NCACTIVATE, Some(WPARAM(1)), Some(LPARAM(-1))) };
}

unsafe extern "system" fn active_frame_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    match msg {
        WM_NCACTIVATE if wparam.0 == 0 => {
            if let Err(err) =
                unsafe { PostMessageW(Some(hwnd), WM_REASSERT_ACTIVE_FRAME, WPARAM(0), LPARAM(0)) }
            {
                log::debug!("active frame could not be reasserted after deactivation: {err}");
            }
            unsafe { DefSubclassProc(hwnd, msg, WPARAM(1), LPARAM(-1)) }
        }
        WM_REASSERT_ACTIVE_FRAME => {
            unsafe { SendMessageW(hwnd, WM_NCACTIVATE, Some(WPARAM(1)), Some(LPARAM(-1))) };
            LRESULT(0)
        }
        WM_SHOWWINDOW if wparam.0 != 0 => {
            unsafe { SendMessageW(hwnd, WM_NCACTIVATE, Some(WPARAM(1)), Some(LPARAM(-1))) };
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        WM_NCDESTROY => {
            let _ = unsafe {
                RemoveWindowSubclass(hwnd, Some(active_frame_proc), ACTIVE_FRAME_SUBCLASS)
            };
            unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
        }
        _ => unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) },
    }
}

/// 窗口的深浅色（影响 Mica 的底色和系统画的边框）。
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

/// 把 accent policy 清成 `ACCENT_DISABLED`。
fn clear_accent(window: HWND) {
    set_accent(window, 0, 2, 0);
}

/// 设 accent policy（未公开的 `SetWindowCompositionAttribute`），`gradient` 是 ABGR 色调。
fn set_accent(window: HWND, state: u32, flags: u32, gradient: u32) {
    let Ok(user32) = (unsafe { GetModuleHandleW(w!("user32.dll")) }) else {
        return;
    };
    let Some(proc) = (unsafe { GetProcAddress(user32, s!("SetWindowCompositionAttribute")) })
    else {
        return;
    };
    let set: SetWindowCompositionAttribute = unsafe { std::mem::transmute(proc) };
    let mut accent = AccentPolicy {
        state,
        flags,
        gradient,
        animation: 0,
    };
    let mut data = CompositionAttributeData {
        attribute: WCA_ACCENT_POLICY,
        data: (&raw mut accent).cast(),
        size: size_of::<AccentPolicy>(),
    };
    if !unsafe { set(window, &mut data) }.as_bool() {
        log::debug!("SetWindowCompositionAttribute(accent {state}) failed");
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
                frosted: true
            }
        );
        assert_eq!(
            support(22000),
            MaterialSupport {
                mica: true,
                frosted: true
            }
        );
        assert_eq!(
            support(17134),
            MaterialSupport {
                mica: false,
                frosted: false
            }
        );
    }

    #[test]
    fn this_machine_has_a_build_number() {
        assert!(build() >= 10240);
    }
}
