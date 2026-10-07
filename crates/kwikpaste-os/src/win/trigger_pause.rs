//! 前台应用让出全局触发（设置 `shortcuts.pauseInFullscreen` / `shortcuts.pauseAppIds`）：前台窗口全屏，
//! 或属于列表里的应用时 [`is_paused`] 为真，鼠标唤起和 Win+V 钩子放行按键，宿主注销全局热键。
//!
//! 专用线程用 `EVENT_SYSTEM_FOREGROUND` 的 WinEvent 钩子跟随前台切换。窗口留在前台期间进出全屏没有
//! 便宜的事件可用（`EVENT_OBJECT_LOCATIONCHANGE` 连光标移动都报，每条都要跨进程投递），改由每秒一次
//! 的重查兜住；钩子收到唤起按键时再按窗口几何即时重查一次，所以按键本身不受这一秒的影响。

use std::ffi::c_void;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};

use windows::Win32::Foundation::{HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow,
};
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook, UnhookWinEvent};
use windows::Win32::UI::WindowsAndMessaging::{
    EVENT_SYSTEM_FOREGROUND, GWL_STYLE, GetClassNameW, GetForegroundWindow, GetMessageW,
    GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    IsZoomed, KillTimer, MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, SetTimer,
    WINEVENT_OUTOFCONTEXT, WM_APP, WM_QUIT, WM_TIMER, WS_CAPTION,
};

use super::apps;

/// 设置变化后请监听线程重查一次。
const WM_RECHECK: u32 = WM_APP + 0x51;
/// 前台窗口留在前台期间进出全屏的兜底重查间隔。
const RECHECK_MS: u32 = 1000;

type Sink = Box<dyn Fn() + Send + Sync>;

static SINK: OnceLock<Sink> = OnceLock::new();
static PAUSED: AtomicBool = AtomicBool::new(false);
static FULLSCREEN_ENABLED: AtomicBool = AtomicBool::new(false);
/// 前台应用在列表里；由监听线程算好，钩子只读。
static LISTED: AtomicBool = AtomicBool::new(false);
static APP_IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// 最近一次前台进程的 id 写法，前台不换进程时不再打开进程、访问文件系统。
static LAST_APP: Mutex<Option<(u32, [String; 2])>> = Mutex::new(None);
static THREAD: Mutex<Option<u32>> = Mutex::new(None);

/// 设置暂停状态变化的出口，进程内只设一次。出口可能在钩子线程上调用，不得阻塞；
/// 它只是通知，当前状态以 [`is_paused`] 为准。
pub fn set_sink(sink: impl Fn() + Send + Sync + 'static) -> io::Result<()> {
    SINK.set(Box::new(sink))
        .map_err(|_| io::Error::other("the trigger pause sink is already set"))
}

/// 按设置起停监听线程；两项都关掉时线程退出、立即恢复。
pub fn configure(fullscreen: bool, app_ids: Vec<String>) -> io::Result<()> {
    let mut thread = THREAD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let needed = fullscreen || !app_ids.is_empty();
    FULLSCREEN_ENABLED.store(fullscreen, Ordering::SeqCst);
    *APP_IDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = app_ids;

    match (*thread, needed) {
        (None, true) => *thread = Some(spawn_watch_thread()?),
        (Some(id), true) => {
            let _ = unsafe { PostThreadMessageW(id, WM_RECHECK, WPARAM(0), LPARAM(0)) };
        }
        (Some(id), false) => {
            let _ = unsafe { PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0)) };
            *thread = None;
            LISTED.store(false, Ordering::SeqCst);
            set_paused(false, "");
        }
        (None, false) => {}
    }

    Ok(())
}

pub fn is_paused() -> bool {
    PAUSED.load(Ordering::SeqCst)
}

/// 唤起按键或 Win+V 按下时在钩子里调用，返回这次是否让给前台应用。只读窗口几何与样式，
/// 不打开进程、不碰文件系统；列表命中沿用监听线程算好的结果。
pub fn recheck_for_input() -> bool {
    let window = unsafe { GetForegroundWindow() };
    if is_own_window(window) {
        set_paused(false, "");
        return false;
    }
    let fullscreen = FULLSCREEN_ENABLED.load(Ordering::SeqCst) && is_fullscreen(window);
    update(fullscreen, LISTED.load(Ordering::SeqCst))
}

fn spawn_watch_thread() -> io::Result<u32> {
    let (ready, started) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("trigger-pause".to_owned())
        .spawn(move || run_watch_thread(ready))?;

    started
        .recv()
        .map_err(|_| io::Error::other("the trigger pause thread stopped while starting"))?
}

fn run_watch_thread(ready: mpsc::SyncSender<io::Result<u32>>) {
    let mut msg = MSG::default();
    let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
    let hook = unsafe {
        SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            None,
            Some(foreground_changed),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        )
    };
    if hook.is_invalid() {
        let _ = ready.send(Err(io::Error::other(
            "the foreground event hook could not be installed",
        )));
        return;
    }
    let timer = unsafe { SetTimer(None, 0, RECHECK_MS, None) };
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
    recheck();

    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        if matches!(msg.message, WM_RECHECK | WM_TIMER) {
            recheck();
        }
    }

    let _ = unsafe { KillTimer(None, timer) };
    let _ = unsafe { UnhookWinEvent(hook) };
}

/// 进程外 WinEvent 回调在装钩子的线程（监听线程）的消息循环里执行。
unsafe extern "system" fn foreground_changed(
    _: HWINEVENTHOOK,
    _: u32,
    _: HWND,
    _: i32,
    _: i32,
    _: u32,
    _: u32,
) {
    recheck();
}

/// 监听线程上的完整重查：全屏与列表命中都算。
fn recheck() {
    let window = unsafe { GetForegroundWindow() };
    if is_own_window(window) {
        LISTED.store(false, Ordering::SeqCst);
        set_paused(false, "");
        return;
    }
    let fullscreen = FULLSCREEN_ENABLED.load(Ordering::SeqCst) && is_fullscreen(window);
    let listed = is_listed(window);
    LISTED.store(listed, Ordering::SeqCst);
    update(fullscreen, listed);
}

fn update(fullscreen: bool, listed: bool) -> bool {
    let paused = fullscreen || listed;
    set_paused(
        paused,
        if fullscreen {
            "full screen"
        } else {
            "listed app"
        },
    );
    paused
}

fn set_paused(paused: bool, reason: &str) {
    if PAUSED.swap(paused, Ordering::SeqCst) == paused {
        return;
    }
    if paused {
        log::info!("global triggers paused: {reason}");
    } else {
        log::info!("global triggers resumed");
    }
    if let Some(sink) = SINK.get() {
        sink();
    }
}

fn window_process(window: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    pid
}

fn is_own_window(window: HWND) -> bool {
    !window.is_invalid() && window_process(window) == unsafe { GetCurrentProcessId() }
}

/// 前台进程是否在列表里；列表为空时不打开进程。
fn is_listed(window: HWND) -> bool {
    let app_ids = APP_IDS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    if app_ids.is_empty() || window.is_invalid() {
        return false;
    }
    let pid = window_process(window);
    if pid == 0 {
        return false;
    }

    let mut last = LAST_APP
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if last.as_ref().is_none_or(|(cached, _)| *cached != pid) {
        *last = apps::process_app_ids(pid).map(|ids| (pid, ids));
    }
    last.as_ref()
        .is_some_and(|(_, ids)| matches_app_ids(ids, &app_ids))
}

/// Windows 路径不区分大小写；两种 id 写法任一命中即可。
fn matches_app_ids(ids: &[String; 2], app_ids: &[String]) -> bool {
    ids.iter()
        .any(|id| app_ids.iter().any(|wanted| wanted.eq_ignore_ascii_case(id)))
}

/// 判定全屏需要的窗口信息。
#[derive(Debug, Clone, Copy)]
struct WindowFacts {
    rect: RECT,
    monitor: RECT,
    visible: bool,
    minimized: bool,
    cloaked: bool,
    shell: bool,
    maximized: bool,
    captioned: bool,
}

/// 窗口盖住了所在显示器的整个区域（含任务栏）。任务栏自动隐藏时普通窗口最大化也会盖满，
/// 带标题栏的最大化窗口因此不算；无边框全屏和独占全屏的游戏、F11 浏览器都算。
fn judge_fullscreen(facts: WindowFacts) -> bool {
    let WindowFacts { rect, monitor, .. } = facts;
    let covers = rect.left <= monitor.left
        && rect.top <= monitor.top
        && rect.right >= monitor.right
        && rect.bottom >= monitor.bottom;
    let ordinary_maximized = facts.maximized && facts.captioned;

    facts.visible
        && !facts.minimized
        && !facts.cloaked
        && !facts.shell
        && covers
        && !ordinary_maximized
}

fn is_fullscreen(window: HWND) -> bool {
    if window.is_invalid() {
        return false;
    }
    let mut rect = RECT::default();
    if unsafe { GetWindowRect(window, &mut rect) }.is_err() {
        return false;
    }
    let monitor = unsafe { MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if monitor.is_invalid() || !unsafe { GetMonitorInfoW(monitor, &mut info) }.as_bool() {
        return false;
    }
    let mut cloaked = 0u32;
    let _ = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast::<c_void>(),
            size_of::<u32>() as u32,
        )
    };
    let style = unsafe { GetWindowLongPtrW(window, GWL_STYLE) } as u32;

    judge_fullscreen(WindowFacts {
        rect,
        monitor: info.rcMonitor,
        visible: unsafe { IsWindowVisible(window) }.as_bool(),
        minimized: unsafe { IsIconic(window) }.as_bool(),
        cloaked: cloaked != 0,
        shell: is_shell_window(window),
        maximized: unsafe { IsZoomed(window) }.as_bool(),
        captioned: style & WS_CAPTION.0 == WS_CAPTION.0,
    })
}

/// 桌面与任务栏本身盖满屏幕，但不是全屏应用。
fn is_shell_window(window: HWND) -> bool {
    let mut buffer = [0u16; 32];
    let len = unsafe { GetClassNameW(window, &mut buffer) }.max(0) as usize;

    matches!(
        String::from_utf16_lossy(&buffer[..len]).as_str(),
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONITOR: RECT = RECT {
        left: 0,
        top: 0,
        right: 1920,
        bottom: 1080,
    };

    fn facts(rect: RECT) -> WindowFacts {
        WindowFacts {
            rect,
            monitor: MONITOR,
            visible: true,
            minimized: false,
            cloaked: false,
            shell: false,
            maximized: false,
            captioned: false,
        }
    }

    #[test]
    fn a_borderless_window_covering_the_monitor_is_fullscreen() {
        assert!(judge_fullscreen(facts(MONITOR)));
        let overhanging = RECT {
            left: -1,
            top: -1,
            right: 1921,
            bottom: 1081,
        };
        assert!(judge_fullscreen(facts(overhanging)));
    }

    #[test]
    fn a_maximized_captioned_window_is_not_fullscreen() {
        // 任务栏自动隐藏时，最大化窗口连边框一起超出显示器。
        let rect = RECT {
            left: -8,
            top: -8,
            right: 1928,
            bottom: 1088,
        };
        let maximized = WindowFacts {
            maximized: true,
            captioned: true,
            ..facts(rect)
        };
        assert!(!judge_fullscreen(maximized));
    }

    #[test]
    fn smaller_hidden_or_shell_windows_are_not_fullscreen() {
        let smaller = RECT {
            right: 1920,
            bottom: 1040,
            ..MONITOR
        };
        assert!(!judge_fullscreen(facts(smaller)));
        assert!(!judge_fullscreen(WindowFacts {
            shell: true,
            ..facts(MONITOR)
        }));
        assert!(!judge_fullscreen(WindowFacts {
            minimized: true,
            ..facts(MONITOR)
        }));
        assert!(!judge_fullscreen(WindowFacts {
            cloaked: true,
            ..facts(MONITOR)
        }));
    }

    #[test]
    fn a_window_is_judged_against_its_own_monitor() {
        let second = RECT {
            left: 1920,
            right: 3840,
            ..MONITOR
        };
        assert!(judge_fullscreen(WindowFacts {
            monitor: second,
            ..facts(second)
        }));
        assert!(!judge_fullscreen(WindowFacts {
            monitor: second,
            ..facts(MONITOR)
        }));
    }

    #[test]
    fn listed_apps_match_either_id_spelling_ignoring_case() {
        let ids = [
            r"C:\Games\GAME~1\Game.exe".to_owned(),
            r"C:\Games\Big Game\Game.exe".to_owned(),
        ];
        assert!(matches_app_ids(
            &ids,
            &[r"c:\games\big game\game.exe".to_owned()]
        ));
        assert!(matches_app_ids(
            &ids,
            &[r"C:\Games\GAME~1\Game.exe".to_owned()]
        ));
        assert!(!matches_app_ids(&ids, &[r"C:\Games\Other.exe".to_owned()]));
        assert!(!matches_app_ids(&ids, &[]));
    }
}
