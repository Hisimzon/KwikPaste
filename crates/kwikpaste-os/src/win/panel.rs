//! Windows 面板的「永不激活」模型：样式补丁、子类过程和只走 Win32 的显示 / 隐藏 / 定位。
//!
//! 面板是 GPUI 的 `WindowKind::PopUp`（已带 `WS_EX_TOOLWINDOW | WS_EX_TOPMOST`），这里再补：
//! - `WS_EX_NOACTIVATE`：点击、显示都不夺走前台，目标应用的焦点和输入法组字保持原样；
//! - `WS_THICKFRAME`：四边可拉伸（不可见的拉伸边，GPUI 去掉了标题栏）；
//! - 子类过程：
//!   - `WM_MOUSEACTIVATE` 由 GPUI 补丁 W0002 回 `MA_NOACTIVATE`，这里校验返回值，补丁失效时兜底并记错；
//!   - 拖动区和拉伸边的按下改走自己的 `SetCapture` 循环：`DefWindowProc` 的模态移动 / 缩放循环
//!     会让窗口在本线程内被激活（前台没变，但 GPUI 以为自己是活动窗口），这种「幽灵激活」清除不掉；
//!   - 吞掉非客户区双击和 `SC_MOVE` / `SC_SIZE` / `SC_MAXIMIZE`，它们同样会幽灵激活；
//!   - `WM_GETMINMAXINFO` 的最小尺寸乘上系统「文本大小」（GPUI 只在建窗时按逻辑像素算一次）；
//!   - 断言：收到激活却不是前台窗口时记一次幽灵激活。
//!
//! 子类过程只改消息、记日志和计数，不调用 GPUI。显示、隐藏、移动一律带 `SWP_NOACTIVATE`，
//! 并且要在 GPUI 的 `App` 借用之外调用：这些调用会同步触发 GPUI 的窗口过程，借用中它只能丢掉回调。

use std::cell::Cell;
use std::ffi::c_void;
use std::io;
use std::sync::atomic::{AtomicU32, Ordering};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetCapture, ReleaseCapture, SetCapture};
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    GWL_EXSTYLE, GWL_STYLE, GetClientRect, GetCursorPos, GetForegroundWindow, GetPropW,
    GetWindowLongPtrW, GetWindowRect, HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTLEFT,
    HTRIGHT, HTTOP, HTTOPLEFT, HTTOPRIGHT, HWND_TOPMOST, IsWindowVisible, MA_NOACTIVATE,
    MA_NOACTIVATEANDEAT, MINMAXINFO, RemovePropW, SC_MAXIMIZE, SC_MOVE, SC_SIZE, SW_HIDE,
    SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SendMessageW, SetPropW, SetWindowLongPtrW, SetWindowPos, ShowWindow, WA_INACTIVE, WM_ACTIVATE,
    WM_CAPTURECHANGED, WM_DPICHANGED, WM_GETMINMAXINFO, WM_LBUTTONUP, WM_MOUSEACTIVATE,
    WM_MOUSEMOVE, WM_NCDESTROY, WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCMOUSEMOVE,
    WM_SETTINGCHANGE, WM_SYSCOLORCHANGE, WM_SYSCOMMAND, WS_EX_APPWINDOW, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_THICKFRAME,
};
use windows::core::{PCWSTR, w};

use super::monitor::BASE_DPI;
use super::rect_from_win32;
use super::system;
use crate::geometry::Rect;

/// 子类 id，ASCII "KPNL"。
const SUBCLASS_ID: usize = 0x4B50_4E4C;
/// 保存子类状态指针的窗口属性名。
const STATE_PROPERTY: PCWSTR = w!("KwikPastePanelState");

static PHANTOM_ACTIVATIONS: AtomicU32 = AtomicU32::new(0);
static ACTIVATIONS: AtomicU32 = AtomicU32::new(0);
static MOUSE_ACTIVATE_REPLIES: [AtomicU32; 5] = [const { AtomicU32::new(0) }; 5];
static MOUSE_ACTIVATE_OVERRIDES: AtomicU32 = AtomicU32::new(0);

/// 面板的建窗后参数。
#[derive(Debug, Clone, Copy)]
pub struct PanelOptions {
    /// 最小内容区尺寸（逻辑像素，未乘文本缩放）。
    pub min_logical_size: (f64, f64),
    /// 系统「文本大小」系数，最小尺寸按它放大。
    pub text_scale: f64,
}

/// 子类过程记下的计数，自测和日志用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PanelCounters {
    /// 收到的激活（`WM_ACTIVATE` 非 `WA_INACTIVE`）总数；只有编辑态取前台时才应增加。
    pub activations: u32,
    /// 不在前台却收到激活的次数。正常应始终为 0。
    pub phantom_activations: u32,
    /// GPUI 对 `WM_MOUSEACTIVATE` 的回复，按值计数：下标 1–4 依次是 `MA_ACTIVATE`、
    /// `MA_ACTIVATEANDEAT`、`MA_NOACTIVATE`、`MA_NOACTIVATEANDEAT`，0 是其它值。
    pub mouse_activate_replies: [u32; 5],
    /// 带 `WS_EX_NOACTIVATE` 时 GPUI 仍回激活、由子类改成 `MA_NOACTIVATE` 的次数（补丁 W0002 失效）。
    pub mouse_activate_overrides: u32,
}

/// 当前进程里面板子类过程的计数。
pub fn counters() -> PanelCounters {
    PanelCounters {
        activations: ACTIVATIONS.load(Ordering::Relaxed),
        phantom_activations: PHANTOM_ACTIVATIONS.load(Ordering::Relaxed),
        mouse_activate_replies: std::array::from_fn(|index| {
            MOUSE_ACTIVATE_REPLIES[index].load(Ordering::Relaxed)
        }),
        mouse_activate_overrides: MOUSE_ACTIVATE_OVERRIDES.load(Ordering::Relaxed),
    }
}

/// 面板窗口的原生句柄。只能在创建它的线程（GPUI 主线程）上使用。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Panel {
    hwnd: HWND,
}

impl Panel {
    /// 包装一个已存在的窗口句柄。
    ///
    /// # Safety
    /// `hwnd` 必须是当前线程创建、仍然存活的窗口。
    pub unsafe fn from_raw(hwnd: isize) -> Self {
        Self {
            hwnd: HWND(hwnd as *mut c_void),
        }
    }

    pub fn raw(&self) -> isize {
        self.hwnd.0 as isize
    }

    /// 建窗后调用一次：补样式、挂子类过程。
    pub fn install(&self, options: PanelOptions) -> io::Result<()> {
        let hwnd = self.hwnd;
        let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };
        let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
        let ex_style = (ex_style | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW).0 as isize)
            & !(WS_EX_APPWINDOW.0 as isize);

        unsafe {
            SetWindowLongPtrW(hwnd, GWL_STYLE, style | WS_THICKFRAME.0 as isize);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex_style);
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            )
        }
        .map_err(io::Error::other)?;

        let state = Box::into_raw(Box::new(SubclassState {
            min_logical_size: options.min_logical_size,
            text_scale: Cell::new(options.text_scale),
            drag: Cell::new(None),
        }));
        let installed =
            unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, state as usize) };
        if !installed.as_bool() {
            drop(unsafe { Box::from_raw(state) });
            return Err(io::Error::last_os_error());
        }
        if let Err(err) = unsafe { SetPropW(hwnd, STATE_PROPERTY, Some(HANDLE(state.cast()))) } {
            log::warn!("panel state property could not be set: {err}");
        }

        Ok(())
    }

    pub fn is_visible(&self) -> bool {
        unsafe { IsWindowVisible(self.hwnd) }.as_bool()
    }

    /// 系统「文本大小」变化后更新最小尺寸用的系数（下一次拖动、显示即生效）。
    pub fn set_text_scale(&self, text_scale: f64) {
        // 状态指针另存一份在窗口属性里：GetWindowSubclass 只有 comctl32 v6 按名字导出，
        // 没带清单的进程（单测）加载时会找不到入口。
        let data = unsafe { GetPropW(self.hwnd, STATE_PROPERTY) };
        if !data.is_invalid() {
            let state = unsafe { &*(data.0 as *const SubclassState) };
            state.text_scale.set(text_scale);
        }
    }

    /// 编辑态去掉 `WS_EX_NOACTIVATE`（窗口能被激活、拿到键盘和输入法），退出时加回。
    /// 加回后 GPUI（补丁 W0002）对点击重新回 `MA_NOACTIVATE`。
    pub fn set_activatable(&self, activatable: bool) {
        let ex_style = unsafe { GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) };
        let flag = WS_EX_NOACTIVATE.0 as isize;
        let next = if activatable {
            ex_style & !flag
        } else {
            ex_style | flag
        };
        if next != ex_style {
            unsafe { SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, next) };
        }
    }

    /// 窗口是否带 `WS_EX_NOACTIVATE`。
    pub fn is_non_activating(&self) -> bool {
        let ex_style = unsafe { GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE) };
        ex_style & WS_EX_NOACTIVATE.0 as isize != 0
    }

    /// 窗口当前所在显示器的 DPI。
    pub fn dpi(&self) -> u32 {
        super::monitor::window_dpi(self.hwnd)
    }

    /// 外框矩形（含不可见的拉伸边），屏幕坐标。
    pub fn window_rect(&self) -> io::Result<Rect> {
        let mut rect = RECT::default();
        unsafe { GetWindowRect(self.hwnd, &mut rect) }.map_err(io::Error::other)?;
        Ok(rect_from_win32(rect))
    }

    /// 内容区矩形，屏幕坐标。
    pub fn client_rect(&self) -> io::Result<Rect> {
        client_rect_on_screen(self.hwnd)
    }

    /// 把内容区放到 `client`（屏幕坐标、物理像素），返回写入的外框矩形。不显示、不激活。
    ///
    /// 目标显示器的 DPI 与窗口当前的不同时分两步：先只移位置，让系统按新显示器发 `WM_DPICHANGED`、
    /// GPUI 换算缩放；再按新 DPI 下的拉伸边宽度写最终矩形。
    pub fn place(&self, client: Rect, target_dpi: u32) -> io::Result<Rect> {
        let hwnd = self.hwnd;

        if self.dpi() != target_dpi {
            let insets = self.client_rect()?.insets_within(self.window_rect()?);
            unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    client.left - insets.left,
                    client.top - insets.top,
                    0,
                    0,
                    SWP_NOACTIVATE | SWP_NOSIZE | SWP_NOZORDER,
                )
            }
            .map_err(io::Error::other)?;
        }

        let insets = self.client_rect()?.insets_within(self.window_rect()?);
        let outer = client.outset(insets);
        unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                outer.left,
                outer.top,
                outer.width(),
                outer.height(),
                SWP_NOACTIVATE,
            )
        }
        .map_err(io::Error::other)?;

        Ok(outer)
    }

    /// 不激活地显示。必须用 `ShowWindow`：`SetWindowPos(SWP_SHOWWINDOW)` 不发 `WM_SHOWWINDOW`，
    /// GPUI 收不到可见性变化，补丁 W0001 也不会唤醒停泊的 vsync 线程。
    pub fn show_without_activating(&self) {
        let _ = unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
    }

    pub fn hide(&self) {
        if unsafe { GetCapture() } == self.hwnd {
            let _ = unsafe { ReleaseCapture() };
        }
        let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
    }

    /// 已经可见时压回 topmost 栈顶：之后才置顶的其它窗口（贴图、画中画）可能盖在上面。
    pub fn raise(&self) -> io::Result<()> {
        unsafe {
            SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            )
        }
        .map_err(io::Error::other)
    }
}

/// 子类过程的私有状态，指针存在子类的 `dwRefData` 里，`WM_NCDESTROY` 时释放。
struct SubclassState {
    min_logical_size: (f64, f64),
    /// 系统「文本大小」系数，由宿主经 [`Panel::set_text_scale`] 更新。
    text_scale: Cell<f64>,
    drag: Cell<Option<Drag>>,
}

/// 自己的移动 / 缩放循环：按下时的命中区、光标和外框。
#[derive(Clone, Copy)]
struct Drag {
    hit: u32,
    cursor: POINT,
    rect: RECT,
}

unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    let state = unsafe { &*(data as *const SubclassState) };

    match msg {
        WM_MOUSEACTIVATE => return mouse_activate(hwnd, msg, wparam, lparam),
        WM_ACTIVATE => {
            let activated = (wparam.0 & 0xFFFF) as u32 != WA_INACTIVE;
            if activated {
                ACTIVATIONS.fetch_add(1, Ordering::Relaxed);
            }
            if activated && unsafe { GetForegroundWindow() } != hwnd {
                let count = PHANTOM_ACTIVATIONS.fetch_add(1, Ordering::Relaxed) + 1;
                log::error!(
                    "panel was activated without being the foreground window (phantom activation #{count})"
                );
            }
        }
        WM_NCLBUTTONDOWN if is_drag_hit(wparam.0 as u32) => {
            begin_drag(hwnd, state, wparam.0 as u32);
            return LRESULT(0);
        }
        WM_MOUSEMOVE | WM_NCMOUSEMOVE if state.drag.get().is_some() => {
            update_drag(hwnd, state);
            return LRESULT(0);
        }
        WM_LBUTTONUP if state.drag.take().is_some() => {
            let _ = unsafe { ReleaseCapture() };
            return LRESULT(0);
        }
        WM_CAPTURECHANGED if state.drag.take().is_some() => return LRESULT(0),
        WM_NCLBUTTONDBLCLK => return LRESULT(0),
        WM_SYSCOMMAND => {
            let command = (wparam.0 & 0xFFF0) as u32;
            if matches!(command, SC_MOVE | SC_SIZE | SC_MAXIMIZE) {
                return LRESULT(0);
            }
        }
        WM_GETMINMAXINFO => {
            let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            apply_min_size(hwnd, state, lparam);
            return result;
        }
        WM_DPICHANGED if state.drag.get().is_some() => {
            // 拖过不同 DPI 的显示器时系统按建议矩形改了尺寸，以当前位置重新起算，免得下一次移动又改回去。
            let result = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
            if let Some(drag) = state.drag.get() {
                restart_drag(hwnd, state, drag.hit);
            }
            return result;
        }
        WM_SETTINGCHANGE | WM_SYSCOLORCHANGE => system::notify_changed(),
        WM_NCDESTROY => {
            let _ = unsafe { RemovePropW(hwnd, STATE_PROPERTY) };
            let _ = unsafe { RemoveWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID) };
            drop(unsafe { Box::from_raw(data as *mut SubclassState) });
        }
        _ => {}
    }

    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

/// 交给 GPUI（补丁 W0002）回答，记下回复；带 `WS_EX_NOACTIVATE` 却回了激活时兜底改成不激活。
fn mouse_activate(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let reply = unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) };
    let index = usize::try_from(reply.0)
        .ok()
        .filter(|value| (1..=4).contains(value));
    MOUSE_ACTIVATE_REPLIES[index.unwrap_or(0)].fetch_add(1, Ordering::Relaxed);

    let ex_style = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) };
    let non_activating = ex_style & WS_EX_NOACTIVATE.0 as isize != 0;
    let declined = reply.0 == MA_NOACTIVATE as isize || reply.0 == MA_NOACTIVATEANDEAT as isize;
    if !non_activating || declined {
        return reply;
    }

    MOUSE_ACTIVATE_OVERRIDES.fetch_add(1, Ordering::Relaxed);
    log::error!(
        "GPUI answered WM_MOUSEACTIVATE with {} on a WS_EX_NOACTIVATE panel; patch W0002 is not in effect",
        reply.0
    );
    LRESULT(MA_NOACTIVATE as isize)
}

fn is_drag_hit(hit: u32) -> bool {
    hit == HTCAPTION || (HTLEFT..=HTBOTTOMRIGHT).contains(&hit)
}

fn begin_drag(hwnd: HWND, state: &SubclassState, hit: u32) {
    restart_drag(hwnd, state, hit);
    unsafe { SetCapture(hwnd) };
}

fn restart_drag(hwnd: HWND, state: &SubclassState, hit: u32) {
    let mut cursor = POINT::default();
    let mut rect = RECT::default();
    let measured = unsafe { GetCursorPos(&mut cursor) }
        .and_then(|()| unsafe { GetWindowRect(hwnd, &mut rect) });
    if let Err(err) = measured {
        log::warn!("panel drag could not read the cursor or window rect: {err}");
        state.drag.set(None);
        return;
    }

    state.drag.set(Some(Drag { hit, cursor, rect }));
}

fn update_drag(hwnd: HWND, state: &SubclassState) {
    let Some(drag) = state.drag.get() else {
        return;
    };
    let mut cursor = POINT::default();
    if unsafe { GetCursorPos(&mut cursor) }.is_err() {
        return;
    }

    let mut info = MINMAXINFO::default();
    unsafe {
        SendMessageW(
            hwnd,
            WM_GETMINMAXINFO,
            None,
            Some(LPARAM(&mut info as *mut MINMAXINFO as isize)),
        )
    };
    let min = (info.ptMinTrackSize.x.max(1), info.ptMinTrackSize.y.max(1));
    let rect = dragged_rect(
        drag.hit,
        rect_from_win32(drag.rect),
        cursor.x - drag.cursor.x,
        cursor.y - drag.cursor.y,
        min,
    );

    let flags = if drag.hit == HTCAPTION {
        SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOSIZE
    } else {
        SWP_NOACTIVATE | SWP_NOZORDER
    };
    let moved = unsafe {
        SetWindowPos(
            hwnd,
            None,
            rect.left,
            rect.top,
            rect.width(),
            rect.height(),
            flags,
        )
    };
    if let Err(err) = moved {
        log::warn!("panel drag could not move the window: {err}");
    }
}

/// 按命中区和光标位移算出拖动后的外框；缩放时不小于 `min`（外框尺寸）。
fn dragged_rect(hit: u32, start: Rect, dx: i32, dy: i32, min: (i32, i32)) -> Rect {
    let mut rect = start;
    if hit == HTCAPTION {
        rect.left += dx;
        rect.right += dx;
        rect.top += dy;
        rect.bottom += dy;
        return rect;
    }

    if matches!(hit, HTLEFT | HTTOPLEFT | HTBOTTOMLEFT) {
        rect.left = (start.left + dx).min(start.right - min.0);
    }
    if matches!(hit, HTRIGHT | HTTOPRIGHT | HTBOTTOMRIGHT) {
        rect.right = (start.right + dx).max(start.left + min.0);
    }
    if matches!(hit, HTTOP | HTTOPLEFT | HTTOPRIGHT) {
        rect.top = (start.top + dy).min(start.bottom - min.1);
    }
    if matches!(hit, HTBOTTOM | HTBOTTOMLEFT | HTBOTTOMRIGHT) {
        rect.bottom = (start.bottom + dy).max(start.top + min.1);
    }

    rect
}

/// 最小外框 = 最小内容区（逻辑像素 × DPI × 文本缩放）+ 当前拉伸边宽度。
fn apply_min_size(hwnd: HWND, state: &SubclassState, lparam: LPARAM) {
    let (Ok(outer), Ok(client)) = (window_rect_of(hwnd), client_rect_on_screen(hwnd)) else {
        return;
    };
    let insets = client.insets_within(outer);
    let scale =
        f64::from(super::monitor::window_dpi(hwnd)) / f64::from(BASE_DPI) * state.text_scale.get();
    let mut width = (state.min_logical_size.0 * scale).round() as i32;
    let mut height = (state.min_logical_size.1 * scale).round() as i32;
    // 文本放得很大、屏幕又小时，最小尺寸不能超过所在显示器的工作区，否则窗口放不下。
    if let Some(work_area) = work_area_of(hwnd) {
        width = width
            .min(work_area.width() - insets.left - insets.right)
            .max(1);
        height = height
            .min(work_area.height() - insets.top - insets.bottom)
            .max(1);
    }

    let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
    info.ptMinTrackSize.x = width + insets.left + insets.right;
    info.ptMinTrackSize.y = height + insets.top + insets.bottom;
}

fn work_area_of(hwnd: HWND) -> Option<Rect> {
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetMonitorInfoW(monitor, &mut info) }
        .as_bool()
        .then(|| rect_from_win32(info.rcWork))
}

fn window_rect_of(hwnd: HWND) -> io::Result<Rect> {
    let mut rect = RECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.map_err(io::Error::other)?;
    Ok(rect_from_win32(rect))
}

fn client_rect_on_screen(hwnd: HWND) -> io::Result<Rect> {
    let mut rect = RECT::default();
    unsafe { GetClientRect(hwnd, &mut rect) }.map_err(io::Error::other)?;

    let mut origin = POINT::default();
    if !unsafe { ClientToScreen(hwnd, &mut origin) }.as_bool() {
        return Err(io::Error::last_os_error());
    }

    Ok(Rect {
        left: origin.x,
        top: origin.y,
        right: origin.x + rect.right,
        bottom: origin.y + rect.bottom,
    })
}

#[cfg(test)]
mod tests {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, HTCLIENT, IsZoomed, MA_ACTIVATE,
        RegisterClassExW, WA_ACTIVE, WM_LBUTTONDOWN, WNDCLASSEXW, WS_EX_TOPMOST, WS_POPUP,
    };
    use windows::core::w;

    use super::*;
    use crate::geometry::Point;

    const START: Rect = Rect {
        left: 100,
        top: 100,
        right: 662,
        bottom: 1011,
    };
    const MIN: (i32, i32) = (562, 911);
    const OPTIONS: PanelOptions = PanelOptions {
        min_logical_size: (360.0, 600.0),
        text_scale: 1.0,
    };

    unsafe extern "system" fn plain_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// 与 GPUI 的 PopUp 同样式的隐藏窗口；不显示，测试期间不会出现在桌面上。
    fn hidden_popup() -> Panel {
        let instance = unsafe { GetModuleHandleW(None) }.expect("module handle");
        let class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(plain_proc),
            hInstance: instance.into(),
            lpszClassName: w!("KwikPasteOsPanelTest"),
            ..Default::default()
        };
        unsafe { RegisterClassExW(&class) };

        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_TOOLWINDOW | WS_EX_TOPMOST,
                w!("KwikPasteOsPanelTest"),
                w!("panel test"),
                WS_POPUP,
                0,
                0,
                400,
                700,
                None,
                None,
                Some(instance.into()),
                None,
            )
        }
        .expect("a hidden test window");

        unsafe { Panel::from_raw(hwnd.0 as isize) }
    }

    fn destroy(panel: Panel) {
        unsafe { DestroyWindow(panel.hwnd) }.expect("destroy the test window");
    }

    #[test]
    fn install_adds_the_non_activating_styles() {
        let panel = hidden_popup();
        panel.install(OPTIONS).expect("install");

        let style = unsafe { GetWindowLongPtrW(panel.hwnd, GWL_STYLE) };
        let ex_style = unsafe { GetWindowLongPtrW(panel.hwnd, GWL_EXSTYLE) };
        assert_ne!(style & WS_THICKFRAME.0 as isize, 0);
        assert_ne!(ex_style & WS_EX_NOACTIVATE.0 as isize, 0);
        assert_ne!(ex_style & WS_EX_TOOLWINDOW.0 as isize, 0);
        assert_ne!(ex_style & WS_EX_TOPMOST.0 as isize, 0);
        assert!(!panel.is_visible());

        destroy(panel);
    }

    #[test]
    fn mouse_activate_falls_back_to_no_activate_and_counts_the_override() {
        let panel = hidden_popup();
        panel.install(OPTIONS).expect("install");
        let before = counters();

        let click = (WM_LBUTTONDOWN << 16) | HTCLIENT;
        let reply = unsafe {
            SendMessageW(
                panel.hwnd,
                WM_MOUSEACTIVATE,
                Some(WPARAM(panel.hwnd.0 as usize)),
                Some(LPARAM(click as isize)),
            )
        };

        let after = counters();
        assert_eq!(reply.0, MA_NOACTIVATE as isize);
        assert!(after.mouse_activate_overrides > before.mouse_activate_overrides);
        assert!(
            after.mouse_activate_replies[MA_ACTIVATE as usize]
                > before.mouse_activate_replies[MA_ACTIVATE as usize]
        );

        destroy(panel);
    }

    #[test]
    fn activation_while_not_foreground_counts_as_phantom() {
        let panel = hidden_popup();
        panel.install(OPTIONS).expect("install");
        let before = counters().phantom_activations;

        unsafe {
            SendMessageW(
                panel.hwnd,
                WM_ACTIVATE,
                Some(WPARAM(WA_ACTIVE as usize)),
                None,
            )
        };

        assert!(counters().phantom_activations > before);
        destroy(panel);
    }

    #[test]
    fn system_move_size_and_maximize_are_swallowed() {
        let panel = hidden_popup();
        panel.install(OPTIONS).expect("install");

        for command in [SC_MAXIMIZE, SC_MOVE, SC_SIZE] {
            let reply = unsafe {
                SendMessageW(
                    panel.hwnd,
                    WM_SYSCOMMAND,
                    Some(WPARAM(command as usize)),
                    None,
                )
            };
            assert_eq!(reply.0, 0);
        }
        assert!(!unsafe { IsZoomed(panel.hwnd) }.as_bool());

        destroy(panel);
    }

    #[test]
    fn min_track_size_covers_the_minimum_content_plus_frame() {
        let panel = hidden_popup();
        panel.install(OPTIONS).expect("install");

        let mut info = MINMAXINFO::default();
        unsafe {
            SendMessageW(
                panel.hwnd,
                WM_GETMINMAXINFO,
                None,
                Some(LPARAM(&mut info as *mut MINMAXINFO as isize)),
            )
        };

        let insets = panel
            .client_rect()
            .expect("client")
            .insets_within(panel.window_rect().expect("window"));
        let scale = f64::from(panel.dpi()) / f64::from(BASE_DPI);
        let work_area = work_area_of(panel.hwnd).expect("work area");
        let width =
            ((360.0 * scale).round() as i32).min(work_area.width() - insets.left - insets.right);
        let height =
            ((600.0 * scale).round() as i32).min(work_area.height() - insets.top - insets.bottom);
        assert_eq!(info.ptMinTrackSize.x, width + insets.left + insets.right);
        assert_eq!(info.ptMinTrackSize.y, height + insets.top + insets.bottom);

        destroy(panel);
    }

    #[test]
    fn min_track_size_follows_the_text_scale_and_stays_inside_the_work_area() {
        let panel = hidden_popup();
        panel.install(OPTIONS).expect("install");
        let min_track = || {
            let mut info = MINMAXINFO::default();
            unsafe {
                SendMessageW(
                    panel.hwnd,
                    WM_GETMINMAXINFO,
                    None,
                    Some(LPARAM(&mut info as *mut MINMAXINFO as isize)),
                )
            };
            (info.ptMinTrackSize.x, info.ptMinTrackSize.y)
        };

        let normal = min_track();
        panel.set_text_scale(1.5);
        let larger = min_track();
        panel.set_text_scale(100.0);
        let huge = min_track();

        assert!(larger.0 > normal.0 && larger.1 > normal.1);
        let work_area = work_area_of(panel.hwnd).expect("work area");
        assert!(huge.0 <= work_area.width() && huge.1 <= work_area.height());
        destroy(panel);
    }

    #[test]
    fn activatable_toggles_only_the_no_activate_bit() {
        let panel = hidden_popup();
        panel.install(OPTIONS).expect("install");
        let others =
            unsafe { GetWindowLongPtrW(panel.hwnd, GWL_EXSTYLE) } & !(WS_EX_NOACTIVATE.0 as isize);

        panel.set_activatable(true);
        assert!(!panel.is_non_activating());
        panel.set_activatable(false);
        assert!(panel.is_non_activating());
        assert_eq!(
            unsafe { GetWindowLongPtrW(panel.hwnd, GWL_EXSTYLE) } & !(WS_EX_NOACTIVATE.0 as isize),
            others
        );
        destroy(panel);
    }

    #[test]
    fn place_puts_the_client_area_exactly_on_the_target() {
        let panel = hidden_popup();
        panel.install(OPTIONS).expect("install");
        // 最小内容区（360×600 × 当前 DPI），放在测试窗所在显示器的工作区里：系统会把超过屏幕的
        // 窗口压到默认的最大跟踪尺寸（CI 的 1024×768 屏放不下 900 高的内容区）。
        let size = crate::geometry::scale_size(
            OPTIONS.min_logical_size,
            f64::from(panel.dpi()) / f64::from(BASE_DPI),
        );
        let work_area = work_area_of(panel.hwnd).expect("work area");
        let insets = panel
            .client_rect()
            .expect("client")
            .insets_within(panel.window_rect().expect("window"));
        let origin = Point {
            x: work_area.left + insets.left + 20,
            y: work_area.top + insets.top + 20,
        };
        let target = Rect::from_origin_size(origin, size);
        if target.outset(insets).right > work_area.right
            || target.outset(insets).bottom > work_area.bottom
        {
            eprintln!(
                "skipped: a {size:?} panel does not fit the work area {work_area:?} of this screen"
            );
            destroy(panel);
            return;
        }

        let outer = panel.place(target, panel.dpi()).expect("place");

        assert_eq!(panel.client_rect().expect("client"), target);
        assert_eq!(panel.window_rect().expect("window"), outer);
        assert!(!panel.is_visible());

        destroy(panel);
    }

    #[test]
    fn caption_drag_moves_without_resizing() {
        let rect = dragged_rect(HTCAPTION, START, 60, -30, MIN);

        assert_eq!(rect.size(), START.size());
        assert_eq!((rect.left, rect.top), (160, 70));
    }

    #[test]
    fn corner_drag_resizes_and_respects_the_minimum() {
        let grown = dragged_rect(HTBOTTOMRIGHT, START, 60, 40, MIN);
        let shrunk = dragged_rect(HTBOTTOMRIGHT, START, -500, -600, MIN);
        let from_top_left = dragged_rect(HTTOPLEFT, START, 400, 400, MIN);

        assert_eq!((grown.width(), grown.height()), (622, 951));
        assert_eq!((shrunk.width(), shrunk.height()), MIN);
        assert_eq!(
            (from_top_left.right, from_top_left.bottom),
            (START.right, START.bottom)
        );
        assert_eq!((from_top_left.width(), from_top_left.height()), MIN);
    }
}
