//! 原生弹出菜单（托盘右键菜单）跟随系统深浅色。
//!
//! Win32 弹出菜单只有在进程声明 `PreferredAppMode = AllowDark` 后才会按系统主题绘制；
//! 1.x 由窗口库在启动时做了这件事，2.0 没有，菜单就一直是浅色。

use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::core::{PCSTR, w};

const SET_PREFERRED_APP_MODE_ORDINAL: usize = 135;
const FLUSH_MENU_THEMES_ORDINAL: usize = 136;
const APP_MODE_ALLOW_DARK: i32 = 1;

type SetPreferredAppMode = unsafe extern "system" fn(i32) -> i32;
type FlushMenuThemes = unsafe extern "system" fn();

/// 让本进程的原生弹出菜单跟随系统深浅色。要在第一个菜单弹出前调用；
/// 取不到未公开的 uxtheme 导出时保持浅色，只记日志。
pub fn allow_dark_menus() {
    let Ok(uxtheme) = (unsafe { LoadLibraryW(w!("uxtheme.dll")) }) else {
        log::warn!("uxtheme.dll could not be loaded, native menus stay light");
        return;
    };
    let Some(set_mode) =
        (unsafe { GetProcAddress(uxtheme, PCSTR(SET_PREFERRED_APP_MODE_ORDINAL as *const u8)) })
    else {
        log::warn!("SetPreferredAppMode is unavailable, native menus stay light");
        return;
    };
    let set_mode: SetPreferredAppMode =
        unsafe { std::mem::transmute::<_, SetPreferredAppMode>(set_mode) };
    unsafe { set_mode(APP_MODE_ALLOW_DARK) };

    if let Some(flush) =
        unsafe { GetProcAddress(uxtheme, PCSTR(FLUSH_MENU_THEMES_ORDINAL as *const u8)) }
    {
        let flush: FlushMenuThemes = unsafe { std::mem::transmute::<_, FlushMenuThemes>(flush) };
        unsafe { flush() };
    }
}
