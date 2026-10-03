//! 面板可见期间的低级键盘钩子（`WH_KEYBOARD_LL`）。
//!
//! - 非编辑态（导航开启）：按 [`crate::hook_keys`] 吞下面板要处理的键，转成 [`HookEvent`] 交给宿主；
//!   被吞的键松开时一并吞掉，否则目标应用会收到孤立的松开。Ctrl 的按下、松开只通知、不吞。
//! - 编辑态（导航关闭）：面板已是前台窗口，键盘消息直接发给它，钩子只放行。
//! - 取前台：注入一次带 [`OWN_INPUT_MARKER`] 的 Alt，由钩子吞掉，目标应用收不到
//!   （见 [`swallow_marked_alt`]）。
//! - 拖出中（[`crate::drag_out::is_active`]）：吞掉 Esc 并取消拖拽（见 `win::drag_out`）。
//!
//! 钩子跑在专用线程上，回调只查表、改原子量、调用不阻塞的出口。面板隐藏时 [`stop`]；
//! 还有被吞的键没松开时线程等它们松开后再退出。

use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, MAPVK_VK_TO_VSC, MapVirtualKeyW, SendInput,
    VIRTUAL_KEY, VK_CONTROL, VK_ESCAPE, VK_LCONTROL, VK_LMENU, VK_LWIN, VK_MENU, VK_RCONTROL,
    VK_RWIN, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, KBDLLHOOKSTRUCT, MSG, PM_NOREMOVE, PeekMessageW,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

use crate::hook_keys::{self, HookKeyKind};

/// 自己注入的输入写进 `dwExtraInfo` 的标记（ASCII "KPMB"），钩子据此认出并处理。
pub const OWN_INPUT_MARKER: usize = 0x4B50_4D42;

/// 按键的阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPhase {
    Down,
    /// 按住不放时系统自动重复的按下。
    Repeat,
    Up,
}

/// 钩子交给宿主的事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    /// 一个被吞下的键。
    Key {
        /// GPUI `Keystroke` 的 key 名，见 [`crate::hook_keys::HOOK_KEYS`]。
        key: &'static str,
        phase: KeyPhase,
        ctrl: bool,
        shift: bool,
    },
    /// Ctrl 按下或松开（不吞，供界面显示快捷键提示）。
    Control { down: bool },
}

type Sink = Box<dyn Fn(HookEvent) + Send + Sync>;

static SINK: OnceLock<Sink> = OnceLock::new();
/// 面板可见：钩子线程应当存在。
static ACTIVE: AtomicBool = AtomicBool::new(false);
/// 非编辑态：吞表里的键。
static NAVIGATION: AtomicBool = AtomicBool::new(false);
/// 按下被吞、还没松开的键。
static SWALLOWED: [AtomicBool; 256] = [const { AtomicBool::new(false) }; 256];
static CONTROL_DOWN: AtomicBool = AtomicBool::new(false);
static MARKED_ALT_SEEN: AtomicU32 = AtomicU32::new(0);
/// 钩子线程 id；`None` 表示没有线程。
static THREAD: Mutex<Option<u32>> = Mutex::new(None);

/// 设置事件出口，进程内只设一次。出口在钩子线程上调用，不得阻塞（转发到 channel 即可）。
pub fn set_sink(sink: impl Fn(HookEvent) + Send + Sync + 'static) -> io::Result<()> {
    SINK.set(Box::new(sink))
        .map_err(|_| io::Error::other("the keyboard hook sink is already set"))
}

/// 面板显示时调用：装好钩子（线程就绪后才返回），打开导航。
pub fn start() -> io::Result<()> {
    let mut thread = thread_state();
    ACTIVE.store(true, Ordering::SeqCst);
    NAVIGATION.store(true, Ordering::SeqCst);
    if thread.is_some() {
        return Ok(());
    }

    match spawn_hook_thread() {
        Ok(id) => {
            *thread = Some(id);
            Ok(())
        }
        Err(err) => {
            ACTIVE.store(false, Ordering::SeqCst);
            NAVIGATION.store(false, Ordering::SeqCst);
            Err(err)
        }
    }
}

/// 面板隐藏时调用：关导航；没有被吞的键还按着就让钩子线程退出，否则等它们松开。
pub fn stop() {
    let thread = thread_state();
    ACTIVE.store(false, Ordering::SeqCst);
    NAVIGATION.store(false, Ordering::SeqCst);
    CONTROL_DOWN.store(false, Ordering::SeqCst);
    if let Some(id) = *thread
        && !any_swallowed()
    {
        let _ = unsafe { PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0)) };
    }
}

/// 编辑态关导航（键盘消息直接进面板），退出编辑态再打开。钩子线程保留：取前台还要它吞 Alt。
pub fn set_navigation(enabled: bool) {
    NAVIGATION.store(enabled && ACTIVE.load(Ordering::SeqCst), Ordering::SeqCst);
}

pub fn is_running() -> bool {
    thread_state().is_some()
}

/// 注入一次带标记的 Alt 按下、松开，等钩子把两次都吞掉后返回 `true`（最多等 `timeout`）。
///
/// 之后紧接着调用 `SetForegroundWindow` 就能成功：本进程刚产生了输入。目标应用收不到这次 Alt，
/// 不会进入菜单模式。钩子没在运行时直接返回 `false`，绝不在没有钩子的情况下注入 Alt。
pub fn swallow_marked_alt(timeout: Duration) -> bool {
    if !is_running() {
        return false;
    }

    let seen_before = MARKED_ALT_SEEN.load(Ordering::SeqCst);
    let scan = unsafe { MapVirtualKeyW(u32::from(VK_MENU.0), MAPVK_VK_TO_VSC) } as u16;
    let inputs = [
        marked_key(VK_LMENU, scan, KEYEVENTF_EXTENDEDKEY),
        marked_key(VK_LMENU, scan, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP),
    ];
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };

    let deadline = Instant::now() + timeout;
    let mut swallowed = false;
    while sent == 2 && Instant::now() < deadline {
        if MARKED_ALT_SEEN.load(Ordering::SeqCst) >= seen_before + 2 {
            swallowed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    if !swallowed {
        log::warn!("the marked Alt was not confirmed by the keyboard hook (sent {sent})");
    }
    swallowed
}

fn marked_key(vk: VIRTUAL_KEY, scan: u16, flags: KEYBD_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: OWN_INPUT_MARKER,
            },
        },
    }
}

fn thread_state() -> std::sync::MutexGuard<'static, Option<u32>> {
    THREAD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn any_swallowed() -> bool {
    SWALLOWED.iter().any(|key| key.load(Ordering::SeqCst))
}

fn spawn_hook_thread() -> io::Result<u32> {
    let (ready, started) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("keyboard-hook".to_owned())
        .spawn(move || run_hook_thread(ready))?;

    started
        .recv()
        .map_err(|_| io::Error::other("the keyboard hook thread stopped while starting"))?
}

fn run_hook_thread(ready: mpsc::SyncSender<io::Result<u32>>) {
    let module = unsafe { GetModuleHandleW(None) }.ok().map(Into::into);
    let hook = match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module, 0) } {
        Ok(hook) => hook,
        Err(err) => {
            let _ = ready.send(Err(io::Error::other(err)));
            return;
        }
    };

    // 先建好消息队列再交出线程 id，否则紧随其后的 WM_QUIT 投递会失败。
    let mut msg = MSG::default();
    let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
    let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));

    loop {
        let result = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        if result.0 > 0 {
            continue;
        }
        // WM_QUIT 或出错：面板又显示了（start 抢在退出前）就继续跑。
        let mut thread = thread_state();
        if ACTIVE.load(Ordering::SeqCst) && result.0 == 0 {
            continue;
        }
        *thread = None;
        break;
    }

    let _ = unsafe { UnhookWindowsHookEx(hook) };
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if handle_key(wparam.0 as u32, event) {
            return LRESULT(1);
        }
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// 处理一个按键事件，返回是否吞掉。
fn handle_key(message: u32, event: &KBDLLHOOKSTRUCT) -> bool {
    if event.dwExtraInfo == OWN_INPUT_MARKER {
        // 只有 swallow_marked_alt 会注入带标记的按键；超时之后才到的也照样吞掉，绝不漏给目标应用。
        MARKED_ALT_SEEN.fetch_add(1, Ordering::SeqCst);
        return true;
    }

    let vk = (event.vkCode & 0xFF) as u16;
    let down = matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN);
    let up = matches!(message, WM_KEYUP | WM_SYSKEYUP);

    // 拖出中：面板不是前台，ole32 看不到 Esc。吞掉它（连同松开）并请求取消拖拽。
    if down && vk == VK_ESCAPE.0 && super::drag_out::request_cancel() {
        SWALLOWED[usize::from(vk)].store(true, Ordering::SeqCst);
        return true;
    }

    if up && SWALLOWED[usize::from(vk)].swap(false, Ordering::SeqCst) {
        if let Some(entry) = hook_keys::HOOK_KEYS.iter().find(|entry| entry.vk == vk)
            && entry.kind == HookKeyKind::Hold
            && NAVIGATION.load(Ordering::SeqCst)
        {
            emit_key(entry.key, KeyPhase::Up);
        }
        if !ACTIVE.load(Ordering::SeqCst) && !any_swallowed() {
            let _ =
                unsafe { PostThreadMessageW(GetCurrentThreadId(), WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
        return true;
    }

    if !NAVIGATION.load(Ordering::SeqCst) || !(down || up) {
        return false;
    }

    if is_control(vk) {
        if CONTROL_DOWN.swap(down, Ordering::SeqCst) != down {
            emit(HookEvent::Control { down });
        }
        return false;
    }

    if !down || key_down(VK_MENU) || key_down(VK_LWIN) || key_down(VK_RWIN) {
        return false;
    }

    let Some(entry) = hook_keys::lookup(vk, key_down(VK_CONTROL)) else {
        return false;
    };

    let repeat = SWALLOWED[usize::from(vk)].swap(true, Ordering::SeqCst);
    match (entry.kind, repeat) {
        (HookKeyKind::Hold, true) => {}
        (_, true) => emit_key(entry.key, KeyPhase::Repeat),
        (_, false) => emit_key(entry.key, KeyPhase::Down),
    }

    true
}

fn emit_key(key: &'static str, phase: KeyPhase) {
    emit(HookEvent::Key {
        key,
        phase,
        ctrl: key_down(VK_CONTROL),
        shift: key_down(VK_SHIFT),
    });
}

fn emit(event: HookEvent) {
    if let Some(sink) = SINK.get() {
        sink(event);
    }
}

fn is_control(vk: u16) -> bool {
    vk == VK_CONTROL.0 || vk == VK_LCONTROL.0 || vk == VK_RCONTROL.0
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    let state = unsafe { GetAsyncKeyState(i32::from(vk.0)) };
    state < 0
}
