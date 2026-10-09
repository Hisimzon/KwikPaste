use std::cell::Cell;
use std::ptr::null_mut;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};

use tauri::{AppHandle, Manager};
use winapi::shared::minwindef::{DWORD, HIWORD, LPARAM, LRESULT, UINT, WPARAM};
use winapi::shared::windef::{HHOOK, POINT};
use winapi::um::processthreadsapi::{GetCurrentProcessId, GetCurrentThreadId};
use winapi::um::winuser::{
    CallNextHookEx, GetAncestor, GetMessageW, GetSystemMetrics, GetWindowThreadProcessId,
    KillTimer, PeekMessageW, PostThreadMessageW, SendInput, SetTimer, SetWindowsHookExW,
    UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, INPUT, INPUT_MOUSE, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEINPUT, MSG, MSLLHOOKSTRUCT, PM_NOREMOVE, SM_CXDRAG, SM_CYDRAG, WH_MOUSE_LL,
    WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MBUTTONUP, WM_MOUSEMOVE, WM_QUIT, WM_RBUTTONDOWN, WM_TIMER,
    WM_XBUTTONDOWN, WM_XBUTTONUP, XBUTTON1, XBUTTON2,
};

use crate::settings::MouseTrigger;
use crate::window::{self, CLIPBOARD_WINDOW_LABEL};

/// 剪贴板窗口可见期间开启：点击窗外即隐藏。
static OUTSIDE_CLICK_HIDE: AtomicBool = AtomicBool::new(false);
/// 「鼠标按键唤起」选中的按键，存 [`TriggerButton`] 的编码，0 表示关闭；开启期间钩子常驻。
static TRIGGER_BUTTON: AtomicU8 = AtomicU8::new(0);
/// 钩子线程 id，`None` 表示当前没有钩子；两项功能都关闭时让线程退出。
static HOOK_THREAD_ID: Mutex<Option<u32>> = Mutex::new(None);
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// 重装钩子的间隔。系统先调用最后装上的低级鼠标钩子，定期重装让快贴一直排在最前，
/// 其它软件后装的钩子最多只能抢先这么久。
const HOOK_REFRESH_MS: UINT = 2000;
/// 补发中键按下时写进 `dwExtraInfo` 的标记，钩子据此认出并放行自己注入的事件。
/// 其它软件注入的按键（如鼠标驱动把别的键映射成中键）照常处理。
const REPLAY_EXTRA_INFO: usize = 0x4B50_4D42;

thread_local! {
    /// 唤起按键按下后的状态；钩子回调总在钩子线程执行，线程退出即重置。
    static PRESS: Cell<Press> = const { Cell::new(Press::Idle) };
}

#[derive(Clone, Copy)]
enum Press {
    Idle,
    /// 按下已吞掉，等它松开；记下按下位置，中键松开前拖出阈值就交还给应用。
    Held(TriggerButton, POINT),
    /// 中键已拖出阈值，按下已补发给应用，松开原样放行。
    MiddleDragging,
}

/// 唤起剪贴板窗口的鼠标按键；discriminant 就是存进 [`TRIGGER_BUTTON`] 的编码。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TriggerButton {
    Middle = 1,
    /// XBUTTON1，多数鼠标上的「后退」侧键。
    Back = 2,
    /// XBUTTON2，多数鼠标上的「前进」侧键。
    Forward = 3,
}

impl TriggerButton {
    fn from_setting(trigger: MouseTrigger) -> Option<Self> {
        match trigger {
            MouseTrigger::Disabled => None,
            MouseTrigger::Middle => Some(Self::Middle),
            MouseTrigger::Back => Some(Self::Back),
            MouseTrigger::Forward => Some(Self::Forward),
        }
    }

    fn current() -> Option<Self> {
        match TRIGGER_BUTTON.load(Ordering::Relaxed) {
            1 => Some(Self::Middle),
            2 => Some(Self::Back),
            3 => Some(Self::Forward),
            _ => None,
        }
    }

    /// 从鼠标消息里认出中键 / 侧键，返回按键和是否为按下；侧键看 `mouseData` 的高位字。
    fn from_message(msg: UINT, mouse_data: DWORD) -> Option<(Self, bool)> {
        match msg {
            WM_MBUTTONDOWN => Some((Self::Middle, true)),
            WM_MBUTTONUP => Some((Self::Middle, false)),
            WM_XBUTTONDOWN | WM_XBUTTONUP => {
                let button = match HIWORD(mouse_data) {
                    XBUTTON1 => Self::Back,
                    XBUTTON2 => Self::Forward,
                    _ => return None,
                };
                Some((button, msg == WM_XBUTTONDOWN))
            }
            _ => None,
        }
    }
}

pub fn enable_outside_click_hide(app: &AppHandle) {
    let _ = APP_HANDLE.set(app.clone());
    update_hook(|| OUTSIDE_CLICK_HIDE.store(true, Ordering::Relaxed));
}

pub fn disable_outside_click_hide() {
    update_hook(|| OUTSIDE_CLICK_HIDE.store(false, Ordering::Relaxed));
}

/// 按「鼠标按键唤起」设置切换唤起按键，关闭时传 `Disabled`；幂等，可重复调用。
pub fn set_mouse_trigger(app: &AppHandle, trigger: MouseTrigger) {
    let _ = APP_HANDLE.set(app.clone());
    let code = TriggerButton::from_setting(trigger).map_or(0, |button| button as u8);
    update_hook(|| TRIGGER_BUTTON.store(code, Ordering::Relaxed));
}

/// 在锁内改一项功能的开关，再按是否还有功能开着起停钩子线程，并发启停不会留下多余线程。
fn update_hook(change: impl FnOnce()) {
    let mut thread_id = HOOK_THREAD_ID
        .lock()
        .expect("mouse hook thread id poisoned");
    change();

    let needed =
        OUTSIDE_CLICK_HIDE.load(Ordering::Relaxed) || TRIGGER_BUTTON.load(Ordering::Relaxed) != 0;
    match (*thread_id, needed) {
        (None, true) => *thread_id = spawn_hook_thread(),
        (Some(tid), false) => {
            unsafe {
                PostThreadMessageW(tid, WM_QUIT, 0, 0);
            }
            *thread_id = None;
        }
        _ => {}
    }
}

/// 起钩子线程，等它装好钩子、建好消息队列后返回线程 id；装钩子失败返回 `None`，下次启用时重试。
fn spawn_hook_thread() -> Option<u32> {
    let (sender, receiver) = mpsc::sync_channel(1);

    std::thread::spawn(move || unsafe {
        let mut hook = SetWindowsHookExW(WH_MOUSE_LL, Some(hook_proc), null_mut(), 0);
        if hook.is_null() {
            log::error!("SetWindowsHookExW(WH_MOUSE_LL) failed");
            let _ = sender.send(None);
            return;
        }

        // 先建好消息队列再交出线程 id，否则紧随其后的 WM_QUIT 会投递失败。
        let mut msg: MSG = std::mem::zeroed();
        PeekMessageW(&mut msg, null_mut(), 0, 0, PM_NOREMOVE);
        let _ = sender.send(Some(GetCurrentThreadId()));

        let timer = SetTimer(null_mut(), 0, HOOK_REFRESH_MS, None);

        // GetMessageW 收到 WM_QUIT 返回 0 → 消息泵自然退出。
        while GetMessageW(&mut msg, null_mut(), 0, 0) > 0 {
            if msg.message == WM_TIMER {
                hook = reinstall_hook(hook);
            }
        }

        KillTimer(null_mut(), timer);
        UnhookWindowsHookEx(hook);
    });

    receiver.recv().ok().flatten()
}

/// 重装钩子，排到钩子链最前面，先于之后才装钩子的其它软件拿到鼠标事件；
/// 钩子因回调超时被系统悄悄摘掉时也能借此找回。先装新的再卸旧的，中间不留空档。
fn reinstall_hook(old: HHOOK) -> HHOOK {
    unsafe {
        let hook = SetWindowsHookExW(WH_MOUSE_LL, Some(hook_proc), null_mut(), 0);
        if hook.is_null() {
            return old;
        }

        UnhookWindowsHookEx(old);
        hook
    }
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return CallNextHookEx(null_mut(), code, wparam, lparam);
    }

    let msg = wparam as UINT;
    let data = &*(lparam as *const MSLLHOOKSTRUCT);

    if let Some(trigger) = TriggerButton::current() {
        match judge_trigger_button(trigger, msg, data) {
            TriggerVerdict::Pass => {}
            TriggerVerdict::Swallow => return 1,
            TriggerVerdict::Toggle => {
                schedule_toggle();
                return 1;
            }
            TriggerVerdict::ReplayMiddleDown => replay_middle_down(),
        }
    }

    if OUTSIDE_CLICK_HIDE.load(Ordering::Relaxed)
        && matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN)
    {
        hide_on_outside_click(data.pt);
    }

    // 其余事件不吞：用户的点击应该正常落到目标窗口。
    CallNextHookEx(null_mut(), code, wparam, lparam)
}

/// 钩子对唤起按键事件的处理结果。
#[derive(Debug, PartialEq, Eq)]
enum TriggerVerdict {
    /// 原样放行。
    Pass,
    /// 吞掉，不交给其它应用。
    Swallow,
    /// 吞掉，并开合剪贴板窗口。
    Toggle,
    /// 放行这次移动，并把吞掉的中键按下补发给应用。
    ReplayMiddleDown,
}

/// 判定鼠标按键唤起怎么处理这个事件；只更新按下状态，开合窗口与补发按下留给调用方。
///
/// 选定按键的单击整个交给快贴：按下和松开都吞掉，其它应用收不到，原有的单击功能
/// （开新标签页、后退等）随之停用，松开时开合剪贴板窗口。侧键按住时鼠标移动多远都算单击；
/// 中键按住拖出阈值就补发按下、交还给应用，自动滚动、平移画布照常。快贴窗口里的中键照常交给窗口，
/// 列表项有自己的中键动作；侧键在快贴窗口里没有用途，同样接管，窗口弹在光标下时再按一次也能收起。
/// 松开按按下时记下的按键配对，按下后改了设置也不会漏掉。
fn judge_trigger_button(
    trigger: TriggerButton,
    msg: UINT,
    data: &MSLLHOOKSTRUCT,
) -> TriggerVerdict {
    if data.dwExtraInfo == REPLAY_EXTRA_INFO {
        return TriggerVerdict::Pass;
    }

    if msg == WM_MOUSEMOVE {
        if let Press::Held(TriggerButton::Middle, origin) = PRESS.get() {
            if beyond_drag_threshold(origin, data.pt) {
                PRESS.set(Press::MiddleDragging);
                return TriggerVerdict::ReplayMiddleDown;
            }
        }

        return TriggerVerdict::Pass;
    }

    let Some((button, down)) = TriggerButton::from_message(msg, data.mouseData) else {
        return TriggerVerdict::Pass;
    };

    if down {
        if button != trigger {
            return TriggerVerdict::Pass;
        }

        if button == TriggerButton::Middle && is_own_window_at(data.pt) {
            PRESS.set(Press::Idle);
            return TriggerVerdict::Pass;
        }

        PRESS.set(Press::Held(button, data.pt));
        return TriggerVerdict::Swallow;
    }

    match PRESS.get() {
        Press::Held(held, _) if held == button => {
            PRESS.set(Press::Idle);
            TriggerVerdict::Toggle
        }
        Press::MiddleDragging if button == TriggerButton::Middle => {
            PRESS.set(Press::Idle);
            TriggerVerdict::Pass
        }
        _ => TriggerVerdict::Pass,
    }
}

/// 偏移超过拖动阈值才算拖动。`SM_CXDRAG` / `SM_CYDRAG` 是不随显示缩放变化的像素值（默认 4），
/// 高缩放屏上按下滚轮带出的抖动就可能超过它，所以放宽到两倍。
fn beyond_drag_threshold(origin: POINT, point: POINT) -> bool {
    let (width, height) = unsafe { (GetSystemMetrics(SM_CXDRAG), GetSystemMetrics(SM_CYDRAG)) };

    (point.x - origin.x).abs() > width * 2 || (point.y - origin.y).abs() > height * 2
}

/// 把吞掉的中键按下补发给光标下的应用，打上标记让钩子放行。
fn replay_middle_down() {
    let mut input: INPUT = unsafe { std::mem::zeroed() };
    input.type_ = INPUT_MOUSE;
    unsafe {
        *input.u.mi_mut() = MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: 0,
            dwFlags: MOUSEEVENTF_MIDDLEDOWN,
            time: 0,
            dwExtraInfo: REPLAY_EXTRA_INFO,
        };
    }

    let sent = unsafe { SendInput(1, &mut input, std::mem::size_of::<INPUT>() as i32) };
    if sent != 1 {
        log::warn!("replay middle button down was not sent");
    }
}

/// 光标下的顶层窗口是否属于快贴自己。WebView2 的子窗口可能属于浏览器进程，所以按根窗口判断。
/// `WindowFromPoint` 只给调用线程自己的窗口发 `WM_NCHITTEST`，钩子线程没有窗口，不会被别的应用卡住。
fn is_own_window_at(point: POINT) -> bool {
    unsafe {
        let hwnd = WindowFromPoint(point);
        if hwnd.is_null() {
            return false;
        }

        let mut process_id: DWORD = 0;
        GetWindowThreadProcessId(GetAncestor(hwnd, GA_ROOT), &mut process_id);

        process_id == GetCurrentProcessId()
    }
}

/// 钩子线程不能直接操作窗口，回到主线程开合剪贴板窗口。
fn schedule_toggle() {
    let Some(app) = APP_HANDLE.get() else {
        return;
    };

    let handle = app.clone();
    if let Err(err) = app.run_on_main_thread(move || {
        if let Err(err) = window::toggle_window(&handle, CLIPBOARD_WINDOW_LABEL) {
            log::warn!("toggle clipboard window via mouse button failed: {err}");
        }
    }) {
        log::warn!("schedule mouse button toggle failed: {err}");
    }
}

/// 失焦隐藏：右键菜单可见时只处理菜单，否则点在剪贴板窗口和预览面板之外就隐藏剪贴板窗口。
fn hide_on_outside_click(cursor: POINT) {
    let Some(app) = APP_HANDLE.get() else {
        return;
    };

    // 右键菜单优先：菜单可见时，光标在菜单矩形外 → 关菜单（剪贴板窗口不连带关，
    // 避免「打开菜单后误点窗内空白处」直接收掉整个面板）。
    if crate::menu::context_window::is_visible(app) {
        if cursor_outside_context_menu(app, cursor) {
            schedule_hide_context_menu(app);
        }
        return;
    }

    // 预览面板可以点选词语，点在面板上等同点在剪贴板窗口内。
    if window::should_auto_hide_clipboard_window()
        && cursor_outside_clipboard_window(app, cursor)
        && !window::preview::contains_physical_point(app, cursor.x, cursor.y)
    {
        schedule_hide(app);
    }
}

fn cursor_outside_clipboard_window(app: &AppHandle, cursor: POINT) -> bool {
    let Some(window) = app.get_webview_window(CLIPBOARD_WINDOW_LABEL) else {
        return false;
    };
    if !window::is_clipboard_window_visible(app) {
        return false;
    }

    let Ok(position) = window.outer_position() else {
        return false;
    };
    let Ok(size) = window.outer_size() else {
        return false;
    };

    cursor.x < position.x
        || cursor.x >= position.x + size.width as i32
        || cursor.y < position.y
        || cursor.y >= position.y + size.height as i32
}

/// 钩子收到的 `cursor` 是 physical 坐标，菜单矩形也用 physical 比对，
/// 不走 logical 换算（避免边缘 1px 舍入误判）。
fn cursor_outside_context_menu(app: &AppHandle, cursor: POINT) -> bool {
    !crate::menu::context_window::contains_physical_point(app, cursor.x, cursor.y)
}

fn schedule_hide(app: &AppHandle) {
    let handle = app.clone();
    if let Err(err) = app.run_on_main_thread(move || {
        if let Err(err) = window::hide_window(&handle, CLIPBOARD_WINDOW_LABEL) {
            log::warn!("auto-hide clipboard window failed: {err}");
        }
    }) {
        log::warn!("schedule auto-hide failed: {err}");
    }
}

fn schedule_hide_context_menu(app: &AppHandle) {
    let handle = app.clone();
    if let Err(err) = app.run_on_main_thread(move || {
        crate::menu::context_window::hide(&handle);
    }) {
        log::warn!("schedule auto-hide context menu failed: {err}");
    }
}

#[cfg(test)]
mod tests {
    use winapi::shared::minwindef::WORD;

    use super::*;

    #[test]
    fn side_buttons_are_told_apart_by_the_high_word() {
        let x1 = DWORD::from(XBUTTON1) << 16;
        let x2 = DWORD::from(XBUTTON2) << 16;

        assert_eq!(
            TriggerButton::from_message(WM_XBUTTONDOWN, x1),
            Some((TriggerButton::Back, true))
        );
        assert_eq!(
            TriggerButton::from_message(WM_XBUTTONUP, x2),
            Some((TriggerButton::Forward, false))
        );
        assert_eq!(
            TriggerButton::from_message(WM_MBUTTONUP, 0),
            Some((TriggerButton::Middle, false))
        );
        assert_eq!(TriggerButton::from_message(WM_XBUTTONDOWN, 0), None);
        assert_eq!(TriggerButton::from_message(WM_LBUTTONDOWN, 0), None);
    }

    fn event(side_button: WORD, x: i32, y: i32) -> MSLLHOOKSTRUCT {
        MSLLHOOKSTRUCT {
            pt: POINT { x, y },
            mouseData: DWORD::from(side_button) << 16,
            flags: 0,
            time: 0,
            dwExtraInfo: 0,
        }
    }

    #[test]
    fn bound_side_button_is_taken_over_even_while_the_mouse_moves() {
        let back = event(XBUTTON1, 100, 100);
        let forward = event(XBUTTON2, 100, 100);
        let judge =
            |msg, data: &MSLLHOOKSTRUCT| judge_trigger_button(TriggerButton::Back, msg, data);

        assert_eq!(judge(WM_XBUTTONDOWN, &back), TriggerVerdict::Swallow);
        assert_eq!(
            judge(WM_MOUSEMOVE, &event(0, 400, 100)),
            TriggerVerdict::Pass
        );
        assert_eq!(judge(WM_XBUTTONDOWN, &forward), TriggerVerdict::Pass);
        assert_eq!(judge(WM_XBUTTONUP, &forward), TriggerVerdict::Pass);
        assert_eq!(judge(WM_XBUTTONUP, &back), TriggerVerdict::Toggle);

        // 没有对应按下的松开（比如按下时还没开启）原样放行。
        assert_eq!(judge(WM_XBUTTONUP, &back), TriggerVerdict::Pass);
    }

    #[test]
    fn middle_drag_goes_back_to_the_app_while_a_click_opens_the_window() {
        let judge =
            |msg, data: &MSLLHOOKSTRUCT| judge_trigger_button(TriggerButton::Middle, msg, data);
        // 测试进程没有窗口，光标下不会是快贴自己的窗口。
        let at = |x, y| event(0, x, y);
        let mut replayed = at(400, 100);
        replayed.dwExtraInfo = REPLAY_EXTRA_INFO;

        assert_eq!(
            judge(WM_MBUTTONDOWN, &at(100, 100)),
            TriggerVerdict::Swallow
        );
        assert_eq!(judge(WM_MOUSEMOVE, &at(101, 101)), TriggerVerdict::Pass);
        assert_eq!(
            judge(WM_MOUSEMOVE, &at(400, 100)),
            TriggerVerdict::ReplayMiddleDown
        );
        assert_eq!(judge(WM_MBUTTONDOWN, &replayed), TriggerVerdict::Pass);
        assert_eq!(judge(WM_MOUSEMOVE, &at(500, 100)), TriggerVerdict::Pass);
        assert_eq!(judge(WM_MBUTTONUP, &at(500, 100)), TriggerVerdict::Pass);

        assert_eq!(
            judge(WM_MBUTTONDOWN, &at(100, 100)),
            TriggerVerdict::Swallow
        );
        assert_eq!(judge(WM_MOUSEMOVE, &at(101, 101)), TriggerVerdict::Pass);
        assert_eq!(judge(WM_MBUTTONUP, &at(101, 101)), TriggerVerdict::Toggle);
    }
}
