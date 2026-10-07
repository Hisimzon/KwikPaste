//! 接管 `Win+V`（设置 `shortcuts.winV`，与 1.x `src-tauri/src/shortcut/win_v.rs` 相同）。
//!
//! `Win+V` 是系统保留热键，`RegisterHotKey` 拦不住，也挡不住系统剪贴板历史面板弹出。所以开启期间
//! 常驻一颗 `WH_KEYBOARD_LL` 钩子：V 按下时 Win 键按着就吞掉这次按下（自动重复也吞）和配对的松开，
//! 并通知宿主开合面板；同时注入一次占位键（未分配的 VK `0xE8`），打断「单击 Win」，Win 松开时不弹
//! 开始菜单。系统设置不动：关掉之后钩子线程退出，`Win+V` 立刻回到系统剪贴板历史。
//!
//! 别的程序注入的按键（`LLKHF_INJECTED`）原样放行（1.x 同样如此），自己注入的占位键也因此放行。
//! 自测可以打开 [`accept_probe_input`]：带 [`PROBE_INPUT_MARKER`] 的注入按键按真实按键处理。

use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock, mpsc};

use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT,
    KEYEVENTF_KEYUP, SendInput, VIRTUAL_KEY, VK_LWIN, VK_RWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, KBDLLHOOKSTRUCT, LLKHF_INJECTED, MSG, PM_NOREMOVE, PeekMessageW,
    PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN,
    WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

const VK_V: u32 = 0x56;
/// 未分配的 VK（AutoHotkey 的 menu-mask key 同款）：注入它没有副作用，却让系统认为 Win 按住期间
/// 有过别的输入，于是不弹开始菜单。
const VK_DUMMY: u16 = 0xE8;
/// 自测探针注入按键时写进 `dwExtraInfo` 的标记（ASCII "KPPR"）。
pub const PROBE_INPUT_MARKER: usize = 0x4B50_5052;

type Sink = Box<dyn Fn() + Send + Sync>;

static SINK: OnceLock<Sink> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);
static ACCEPT_PROBE: AtomicBool = AtomicBool::new(false);
static V_CONSUMED: AtomicBool = AtomicBool::new(false);
static THREAD: Mutex<Option<u32>> = Mutex::new(None);

/// 设置出口（在钩子线程上调用，不得阻塞），进程内只设一次。
pub fn set_sink(sink: impl Fn() + Send + Sync + 'static) -> io::Result<()> {
    SINK.set(Box::new(sink))
        .map_err(|_| io::Error::other("the Win+V sink is already set"))
}

/// 自测：把带 [`PROBE_INPUT_MARKER`] 的注入按键当成真实按键。
pub fn accept_probe_input(accept: bool) {
    ACCEPT_PROBE.store(accept, Ordering::SeqCst);
}

/// 按设置开关接管；幂等。
pub fn set_enabled(enabled: bool) -> io::Result<()> {
    let mut thread = THREAD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ENABLED.store(enabled, Ordering::SeqCst);
    match (*thread, enabled) {
        (None, true) => match spawn_hook_thread() {
            Ok(id) => *thread = Some(id),
            Err(err) => {
                ENABLED.store(false, Ordering::SeqCst);
                return Err(err);
            }
        },
        (Some(id), false) => {
            let _ = unsafe { PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0)) };
            *thread = None;
            V_CONSUMED.store(false, Ordering::SeqCst);
        }
        _ => {}
    }

    Ok(())
}

/// 钩子是否在运行（自测核对开关生效用）。
pub fn is_active() -> bool {
    THREAD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .is_some()
}

fn spawn_hook_thread() -> io::Result<u32> {
    let (ready, started) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("win-v-hook".to_owned())
        .spawn(move || {
            let module = unsafe { GetModuleHandleW(None) }.ok().map(Into::into);
            let hook =
                match unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook_proc), module, 0) } {
                    Ok(hook) => hook,
                    Err(err) => {
                        let _ = ready.send(Err(io::Error::other(err)));
                        return;
                    }
                };
            let mut msg = MSG::default();
            let _ = unsafe { PeekMessageW(&mut msg, None, 0, 0, PM_NOREMOVE) };
            let _ = ready.send(Ok(unsafe { GetCurrentThreadId() }));
            while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {}
            let _ = unsafe { UnhookWindowsHookEx(hook) };
        })?;

    started
        .recv()
        .map_err(|_| io::Error::other("the Win+V hook thread stopped while starting"))?
}

unsafe extern "system" fn hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && ENABLED.load(Ordering::SeqCst) {
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if handle(wparam.0 as u32, event) {
            return LRESULT(1);
        }
    }

    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// 处理一个按键事件，返回是否吞掉。
fn handle(message: u32, event: &KBDLLHOOKSTRUCT) -> bool {
    let injected = event.flags.0 & LLKHF_INJECTED.0 != 0;
    let probe = ACCEPT_PROBE.load(Ordering::SeqCst) && event.dwExtraInfo == PROBE_INPUT_MARKER;
    if (injected && !probe) || event.vkCode != VK_V {
        return false;
    }

    if matches!(message, WM_KEYDOWN | WM_SYSKEYDOWN) {
        if !key_down(VK_LWIN) && !key_down(VK_RWIN) {
            return false;
        }
        // 按住 V 会重复触发按下：只在第一次开合，重复期间仍然吞掉。
        if V_CONSUMED.load(Ordering::SeqCst) {
            return true;
        }
        // 让给前台应用时按下不吞，松开随之放行，系统照常处理 Win+V。
        if super::trigger_pause::recheck_for_input() {
            return false;
        }
        V_CONSUMED.store(true, Ordering::SeqCst);
        suppress_start_menu();
        if let Some(sink) = SINK.get() {
            sink();
        }
        return true;
    }

    // 配对吞掉 V 的松开：按下已被拦截，放行松开会让前台应用收到孤立的 KEYUP。
    matches!(message, WM_KEYUP | WM_SYSKEYUP) && V_CONSUMED.swap(false, Ordering::SeqCst)
}

fn key_down(vk: VIRTUAL_KEY) -> bool {
    unsafe { GetAsyncKeyState(i32::from(vk.0)) < 0 }
}

/// 注入一次占位键的按下、松开，阻止系统在 Win 松开时弹开始菜单。
fn suppress_start_menu() {
    let key = |flags: KEYBD_EVENT_FLAGS| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(VK_DUMMY),
                dwFlags: flags,
                ..Default::default()
            },
        },
    };
    let inputs = [key(KEYBD_EVENT_FLAGS(0)), key(KEYEVENTF_KEYUP)];
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        log::warn!("start menu suppress key sent {sent}/{}", inputs.len());
    }
}
