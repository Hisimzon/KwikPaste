//! 面板可见期间的低级鼠标钩子（`WH_MOUSE_LL`）：在本进程窗口以外按下鼠标就通知宿主隐藏面板。
//!
//! 面板从不激活，系统不会给它失焦通知，只能在全局监听按下。点击本身不吞，照常落到目标窗口。
//! 光标下的顶层窗口属于本进程（面板、以后的预览和菜单、托盘菜单）时不算窗外。
//! 钩子每 2 秒重装一次排到钩子链最前面：其它软件后装的钩子最多只能抢先这么久，
//! 钩子因回调超时被系统悄悄摘掉时也能借此找回。

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};

use windows::Win32::Foundation::{LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GA_ROOT, GetAncestor, GetMessageW, GetWindowThreadProcessId, HHOOK, KillTimer,
    MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetTimer,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_MOUSE_LL, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_QUIT,
    WM_RBUTTONDOWN, WM_TIMER, WindowFromPoint,
};

use crate::geometry::Point;

/// 重装钩子的间隔。
const HOOK_REFRESH_MS: u32 = 2000;

/// 钩子交给宿主的事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEvent {
    /// 在本进程窗口以外按下了鼠标（左、右、中键），坐标是物理像素。
    OutsideClick(Point),
}

type Sink = Box<dyn Fn(MouseEvent) + Send + Sync>;

static SINK: OnceLock<Sink> = OnceLock::new();
static OUTSIDE_CLICK: AtomicBool = AtomicBool::new(false);
static THREAD: Mutex<Option<u32>> = Mutex::new(None);

/// 设置事件出口，进程内只设一次。出口在钩子线程上调用，不得阻塞。
pub fn set_sink(sink: impl Fn(MouseEvent) + Send + Sync + 'static) -> io::Result<()> {
    SINK.set(Box::new(sink))
        .map_err(|_| io::Error::other("the mouse hook sink is already set"))
}

/// 面板显示时调用：开始监听窗外点击，钩子就绪后才返回。
pub fn start_outside_click() -> io::Result<()> {
    let mut thread = thread_state();
    OUTSIDE_CLICK.store(true, Ordering::SeqCst);
    if thread.is_some() {
        return Ok(());
    }

    match spawn_hook_thread() {
        Ok(id) => {
            *thread = Some(id);
            Ok(())
        }
        Err(err) => {
            OUTSIDE_CLICK.store(false, Ordering::SeqCst);
            Err(err)
        }
    }
}

/// 面板隐藏时调用：停止监听并卸下钩子。
pub fn stop_outside_click() {
    let mut thread = thread_state();
    OUTSIDE_CLICK.store(false, Ordering::SeqCst);
    if let Some(id) = thread.take() {
        let _ = unsafe { PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0)) };
    }
}

fn thread_state() -> std::sync::MutexGuard<'static, Option<u32>> {
    THREAD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn spawn_hook_thread() -> io::Result<u32> {
    let (ready, started) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("mouse-hook".to_owned())
        .spawn(move || run_hook_thread(ready))?;

    started
        .recv()
        .map_err(|_| io::Error::other("the mouse hook thread stopped while starting"))?
}

fn install() -> windows::core::Result<HHOOK> {
    let module = unsafe { GetModuleHandleW(None) }.ok().map(Into::into);
    unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(hook_proc), module, 0) }
}

fn run_hook_thread(ready: mpsc::SyncSender<io::Result<u32>>) {
    let mut hook = match install() {
        Ok(hook) => hook,
        Err(err) => {
            let _ = ready.send(Err(io::Error::other(err)));
            return;
        }
    };

    let mut msg = MSG::default();
    let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
    let timer = unsafe { SetTimer(None, 0, HOOK_REFRESH_MS, None) };

    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        if msg.message == WM_TIMER {
            // 先装新的再卸旧的，中间不留空档。
            if let Ok(fresh) = install() {
                let _ = unsafe { UnhookWindowsHookEx(hook) };
                hook = fresh;
            }
        }
    }

    let _ = unsafe { KillTimer(None, timer) };
    let _ = unsafe { UnhookWindowsHookEx(hook) };
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // 拖出期间不算窗外点击：拖拽结束前不能隐藏源窗口。
    if code >= 0
        && OUTSIDE_CLICK.load(Ordering::SeqCst)
        && !crate::drag_out::is_active()
        && matches!(
            wparam.0 as u32,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN
        )
    {
        let event = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if !is_own_window_at(event.pt)
            && let Some(sink) = SINK.get()
        {
            sink(MouseEvent::OutsideClick(Point {
                x: event.pt.x,
                y: event.pt.y,
            }));
        }
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// 光标下的顶层窗口是否属于本进程。`WindowFromPoint` 只给调用线程自己的窗口发 `WM_NCHITTEST`，
/// 钩子线程没有窗口，不会被别的应用卡住。
fn is_own_window_at(point: POINT) -> bool {
    let window = unsafe { WindowFromPoint(point) };
    if window.is_invalid() {
        return false;
    }

    let mut process = 0;
    unsafe { GetWindowThreadProcessId(GetAncestor(window, GA_ROOT), Some(&mut process)) };
    process == unsafe { GetCurrentProcessId() }
}
