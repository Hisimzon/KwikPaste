//! Windows 窗口管理：剪贴板窗口始终不可聚焦，避免破坏外部粘贴目标；
//! 弹层窗口按系统圆角裁剪。

use std::ffi::c_void;
use std::iter;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Manager, WebviewWindow};
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
    DWM_WINDOW_CORNER_PREFERENCE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowLongPtrW, IsWindow, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, GWL_EXSTYLE, HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SW_HIDE, SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE,
};

use super::{get_window, CLIPBOARD_PREVIEW_WINDOW_LABEL, CLIPBOARD_WINDOW_LABEL};
use crate::core::Result;
use crate::menu::context_window::{CONTEXT_MENU_WINDOW_LABEL, CONTEXT_SUBMENU_WINDOW_LABEL};
use crate::{keyboard, mouse};

static CLIPBOARD_PASTE_TARGET: AtomicIsize = AtomicIsize::new(0);
static CLIPBOARD_WINDOW_VISIBLE: AtomicBool = AtomicBool::new(false);
static CLIPBOARD_VISIBILITY_REQUEST: AtomicU64 = AtomicU64::new(0);
static PRE_EDIT_FOREGROUND_HWND: Mutex<Option<isize>> = Mutex::new(None);

/// 返回最新请求的剪贴板窗口可见状态。
///
/// 显隐由原生 `ShowWindow` 完成，tao 的内部 VISIBLE 标志不会同步；WebView 显隐又是异步排队的，
/// 所以 toggle 不能问 `window.is_visible()`。
pub fn is_clipboard_window_visible(_app_handle: &AppHandle) -> bool {
    CLIPBOARD_WINDOW_VISIBLE.load(Ordering::Acquire)
}

pub fn show_window(app_handle: &AppHandle, label: &str) -> Result<()> {
    let window = get_window(app_handle, label)?;
    if label == CLIPBOARD_WINDOW_LABEL {
        remember_clipboard_paste_target(&window)?;
        apply_no_activate(&window, true)?;
        show_without_activation(app_handle, &window)?;
        raise_clipboard_window(app_handle, &window);
    } else {
        window.show().map_err(|e| anyhow::anyhow!(e))?;
        window.unminimize().map_err(|e| anyhow::anyhow!(e))?;
        window.set_focus().map_err(|e| anyhow::anyhow!(e))?;
    }

    Ok(())
}

/// 剪贴板窗口用 `SW_SHOWNOACTIVATE` 显示，Z 序原地不动：在它之后置顶的其它应用窗口（贴图、画中画等）
/// 会一直盖在上面，所以每次显示都压回 topmost 栈顶。已可见时重复显示会反过来盖住还开着的预览和
/// 右键菜单，随后按从下到上的顺序把它们再压回去。
fn raise_clipboard_window(app_handle: &AppHandle, window: &WebviewWindow) {
    let popups = [
        CLIPBOARD_PREVIEW_WINDOW_LABEL,
        CONTEXT_MENU_WINDOW_LABEL,
        CONTEXT_SUBMENU_WINDOW_LABEL,
    ]
    .into_iter()
    .filter_map(|label| app_handle.get_webview_window(label))
    .filter(|popup| popup.is_visible().unwrap_or(false));

    for target in iter::once(window.clone()).chain(popups) {
        if let Err(err) = raise_topmost(&target, false) {
            log::warn!("raise {} window failed: {err}", target.label());
        }
    }
}

/// 把窗口压到 Windows topmost 栈顶，不激活、不动位置尺寸；`show` 为 true 时顺带显示。
/// 同为置顶的窗口之间谁最后压栈谁在上。
pub fn raise_topmost(window: &WebviewWindow, show: bool) -> Result<()> {
    let raw_hwnd = window.hwnd().map_err(|e| anyhow::anyhow!(e))?;
    let hwnd = HWND(raw_hwnd.0 as isize);
    let mut flags = SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE;

    if show {
        flags |= SWP_SHOWWINDOW;
    }

    unsafe {
        SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, flags).map_err(|e| anyhow::anyhow!(e))?;
    }

    Ok(())
}

/// 读取「设置 → 辅助功能 → 文本大小」（100%–225%），没设置过时是 1.0。
pub fn text_scale_factor() -> f64 {
    windows_registry::CURRENT_USER
        .open(r"Software\Microsoft\Accessibility")
        .and_then(|key| key.get_u32("TextScaleFactor"))
        .map(|percent| (f64::from(percent) / 100.0).clamp(1.0, 2.25))
        .unwrap_or(1.0)
}

/// 让弹层窗口跟随 Windows 11 的窗口圆角。
///
/// 系统只给带标题栏或 `WS_THICKFRAME` 的窗口自动圆角；预览、右键菜单这类不要系统阴影的
/// 无边框窗口拿不到，而原生材质会把整个窗口矩形填满，页面里的 CSS 圆角就被四个直角的
/// 背板露了馅。DWM 的圆角半径与 CSS 的 `rounded-2`（8px）一致，1px 边框仍贴着圆角走。
/// Windows 10 不认这个属性，直接跳过。
pub fn round_corners(window: &WebviewWindow) {
    if windows_version::OsVersion::current().build < 22000 {
        return;
    }
    let Ok(raw_hwnd) = window.hwnd() else {
        return;
    };
    let hwnd = HWND(raw_hwnd.0 as isize);
    let preference = DWMWCP_ROUND;

    let result = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &preference as *const DWM_WINDOW_CORNER_PREFERENCE as *const c_void,
            size_of::<DWM_WINDOW_CORNER_PREFERENCE>() as u32,
        )
    };
    if let Err(err) = result {
        log::warn!("round window corners failed for {}: {err}", window.label());
    }
}

pub fn set_clipboard_window_editing(app_handle: &AppHandle, editing: bool) -> Result<()> {
    let window = get_window(app_handle, CLIPBOARD_WINDOW_LABEL)?;

    if editing {
        remember_clipboard_paste_target(&window)?;
    }

    // 窗口显隐由原生 ShowWindow 管理，tao 的内部 VISIBLE flag 不会同步。
    // 此处若调用 set_focusable，tao 会因内部仍是隐藏态而执行 SW_HIDE。
    apply_no_activate(&window, !editing)?;

    if editing {
        let hwnd = window_hwnd(&window)?;
        remember_pre_edit_foreground(hwnd);
        let foreground_hwnd = unsafe { GetForegroundWindow() };
        if foreground_hwnd != hwnd && !unsafe { SetForegroundWindow(hwnd) }.as_bool() {
            apply_no_activate(&window, true)?;
            clear_pre_edit_foreground();
            return Err(anyhow::anyhow!("activate clipboard window for editing").into());
        }

        keyboard::disable_navigation_keys();
        return Ok(());
    }

    let hwnd = window_hwnd(&window)?;
    let should_restore_foreground = unsafe { GetForegroundWindow() == hwnd };

    if is_clipboard_window_visible(app_handle) {
        keyboard::enable_navigation_keys(app_handle);
        mouse::enable_outside_click_hide(app_handle);
    }

    if should_restore_foreground {
        restore_pre_edit_foreground(hwnd);
    } else {
        clear_pre_edit_foreground();
    }

    Ok(())
}

pub fn hide_window(app_handle: &AppHandle, label: &str) -> Result<()> {
    let window = get_window(app_handle, label)?;
    if label == CLIPBOARD_WINDOW_LABEL {
        hide_without_activation(app_handle, &window)?;
        if let Err(err) = apply_no_activate(&window, true) {
            log::warn!("reset clipboard window no-activate style on hide failed: {err:?}");
        }
        clear_pre_edit_foreground();
        crate::menu::context_window::hide(app_handle);
    } else {
        window.hide().map_err(|e| anyhow::anyhow!(e))?;
    }

    Ok(())
}

/// 手动编辑曾让剪贴板窗口进入前台时，粘贴前把前台还给进入编辑前的外部窗口。
pub fn restore_clipboard_paste_target(app_handle: &AppHandle) -> Result<()> {
    let window = get_window(app_handle, CLIPBOARD_WINDOW_LABEL)?;
    let clipboard_hwnd = window_hwnd(&window)?;
    let foreground_hwnd = unsafe { GetForegroundWindow() };

    if foreground_hwnd.0 != 0 && foreground_hwnd != clipboard_hwnd {
        CLIPBOARD_PASTE_TARGET.store(foreground_hwnd.0, Ordering::Relaxed);
        return Ok(());
    }

    let target_hwnd = HWND(CLIPBOARD_PASTE_TARGET.load(Ordering::Relaxed));
    if target_hwnd.0 == 0
        || target_hwnd == clipboard_hwnd
        || !unsafe { IsWindow(target_hwnd) }.as_bool()
    {
        return Ok(());
    }

    if !unsafe { SetForegroundWindow(target_hwnd) }.as_bool() {
        return Err(anyhow::anyhow!("restore clipboard paste target window").into());
    }

    Ok(())
}

fn remember_clipboard_paste_target(window: &WebviewWindow) -> Result<()> {
    let clipboard_hwnd = window_hwnd(window)?;
    let foreground_hwnd = unsafe { GetForegroundWindow() };

    if foreground_hwnd.0 != 0 && foreground_hwnd != clipboard_hwnd {
        CLIPBOARD_PASTE_TARGET.store(foreground_hwnd.0, Ordering::Relaxed);
    }

    Ok(())
}

fn window_hwnd(window: &WebviewWindow) -> Result<HWND> {
    let raw_hwnd = window.hwnd().map_err(|e| anyhow::anyhow!(e))?;
    Ok(HWND(raw_hwnd.0 as isize))
}

fn apply_no_activate(window: &WebviewWindow, no_activate: bool) -> Result<()> {
    let hwnd = window_hwnd(window)?;

    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let no_activate_style = WS_EX_NOACTIVATE.0 as isize;
        let next_style = if no_activate {
            style | no_activate_style
        } else {
            style & !no_activate_style
        };
        if next_style != style {
            let _ = SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next_style);
        }

        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )
        .map_err(|e| anyhow::anyhow!(e))?;
    }

    Ok(())
}

/// 在无激活样式已经生效后显示主窗口，并同步 WebView2 可见状态，避免快捷键呼出期间抢占前台窗口。
fn show_without_activation(app_handle: &AppHandle, window: &WebviewWindow) -> Result<()> {
    let hwnd = window_hwnd(window)?;

    set_clipboard_visibility_on_ui_thread(app_handle, window, hwnd, true)
}

fn hide_without_activation(app_handle: &AppHandle, window: &WebviewWindow) -> Result<()> {
    let hwnd = window_hwnd(window)?;

    set_clipboard_visibility_on_ui_thread(app_handle, window, hwnd, false)
}

/// `with_webview` 可能把 closure 排队到 Tauri UI event loop。快捷键和托盘回调也可能运行
/// 在该线程，故这里只提交显隐任务，绝不能同步等待 closure，否则主线程会等待自身而死锁。
fn set_clipboard_visibility_on_ui_thread(
    app_handle: &AppHandle,
    window: &WebviewWindow,
    hwnd: HWND,
    visible: bool,
) -> Result<()> {
    let request = CLIPBOARD_VISIBILITY_REQUEST.fetch_add(1, Ordering::AcqRel) + 1;
    CLIPBOARD_WINDOW_VISIBLE.store(visible, Ordering::Release);

    if !visible {
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
        keyboard::disable_navigation_keys();
        mouse::disable_outside_click_hide();
    }

    let app_handle = app_handle.clone();

    let schedule_result = window.with_webview(move |webview| unsafe {
        if CLIPBOARD_VISIBILITY_REQUEST.load(Ordering::Acquire) != request {
            return;
        }

        if CLIPBOARD_WINDOW_VISIBLE.load(Ordering::Acquire) {
            if let Err(err) = webview.controller().SetIsVisible(true) {
                log::warn!("show clipboard webview controller failed: {err:?}");
            }

            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            keyboard::enable_navigation_keys(&app_handle);
            mouse::enable_outside_click_hide(&app_handle);
        } else if let Err(err) = webview.controller().SetIsVisible(false) {
            log::warn!("hide clipboard webview controller failed after native hide: {err:?}");
        }
    });

    if let Err(err) = schedule_result {
        if !visible {
            log::warn!("sync hidden clipboard webview state failed: {err:?}");
            return Ok(());
        }

        if CLIPBOARD_VISIBILITY_REQUEST.load(Ordering::Acquire) == request {
            CLIPBOARD_WINDOW_VISIBLE.store(false, Ordering::Release);
        }
        unsafe {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
        keyboard::disable_navigation_keys();
        mouse::disable_outside_click_hide();
        return Err(anyhow::anyhow!(err).into());
    }

    Ok(())
}

fn remember_pre_edit_foreground(clipboard_hwnd: HWND) {
    let mut guard = PRE_EDIT_FOREGROUND_HWND
        .lock()
        .expect("pre edit foreground hwnd poisoned");
    if guard.is_some() {
        return;
    }

    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0 == 0 || foreground == clipboard_hwnd {
        return;
    }

    *guard = Some(foreground.0);
}

fn restore_pre_edit_foreground(clipboard_hwnd: HWND) {
    let previous = PRE_EDIT_FOREGROUND_HWND
        .lock()
        .expect("pre edit foreground hwnd poisoned")
        .take();
    let Some(previous) = previous else {
        return;
    };

    let previous_hwnd = HWND(previous);
    if previous_hwnd == clipboard_hwnd || !unsafe { IsWindow(previous_hwnd).as_bool() } {
        return;
    }

    if !unsafe { SetForegroundWindow(previous_hwnd).as_bool() } {
        log::debug!("restore pre-edit foreground window was rejected by Windows");
    }
}

fn clear_pre_edit_foreground() {
    PRE_EDIT_FOREGROUND_HWND
        .lock()
        .expect("pre edit foreground hwnd poisoned")
        .take();
}

pub fn show_taskbar_icon(app_handle: &AppHandle, visible: bool) -> Result<()> {
    let window = get_window(app_handle, CLIPBOARD_WINDOW_LABEL)?;
    window
        .set_skip_taskbar(!visible)
        .map_err(|e| anyhow::anyhow!(e))?;
    Ok(())
}
