//! Windows 单实例：命名互斥量 + 收 `WM_COPYDATA` 的隐藏窗口，名字与 tauri-plugin-single-instance 一致。

use std::ffi::c_void;
use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex};
use windows::Win32::UI::WindowsAndMessaging::{
    CREATESTRUCTW, ChangeWindowMessageFilterEx, CreateWindowExW, DefWindowProcW, DestroyWindow,
    FindWindowW, GWL_STYLE, GWLP_USERDATA, GetWindowLongPtrW, MSGFLT_ALLOW, RegisterClassExW,
    SMTO_ABORTIFHUNG, SendMessageTimeoutW, SetWindowLongPtrW, WM_COPYDATA, WM_NCCREATE,
    WM_NCDESTROY, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT, WS_OVERLAPPED, WS_POPUP, WS_VISIBLE,
};
use windows::core::{HSTRING, PCWSTR};

use crate::single_instance::{Claim, Invocation, current_invocation, decode_pipe, encode_pipe};

/// `COPYDATASTRUCT::dwData`，与 tauri-plugin-single-instance 相同。
const COPYDATA_TAG: usize = 1542;
/// 互斥量已存在、但对方的窗口还没建好（正在启动或退出）时，最多等这么久。
const FIND_WINDOW_TIMEOUT: Duration = Duration::from_secs(2);
const FIND_WINDOW_INTERVAL: Duration = Duration::from_millis(50);
/// 转交参数时对方无响应的上限，免得卡死在一个挂起的主实例上。
const FORWARD_TIMEOUT_MS: u32 = 5000;

type Sink = Box<dyn Fn(Invocation) + Send>;

/// 主实例守卫：丢弃时释放互斥量、销毁收消息的窗口。必须在主线程丢弃。
pub struct PrimaryInstance {
    mutex: HANDLE,
    window: HWND,
}

impl Drop for PrimaryInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = ReleaseMutex(self.mutex);
            let _ = CloseHandle(self.mutex);
        }
        if let Err(err) = unsafe { DestroyWindow(self.window) } {
            log::warn!("single instance window could not be destroyed: {err}");
        }
    }
}

struct Names {
    class: HSTRING,
    window: HSTRING,
    mutex: HSTRING,
}

impl Names {
    fn new(identifier: &str) -> Self {
        Self {
            class: HSTRING::from(format!("{identifier}-sic")),
            window: HSTRING::from(format!("{identifier}-siw")),
            mutex: HSTRING::from(format!("{identifier}-sim")),
        }
    }
}

pub(crate) fn claim(identifier: &str, sink: Sink) -> io::Result<Claim> {
    let names = Names::new(identifier);
    let mutex = unsafe { CreateMutexW(None, true, &names.mutex) }.map_err(io::Error::other)?;
    let already_running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;

    if already_running {
        if let Some(target) = find_primary_window(&names) {
            let forwarded = forward(target, &current_invocation());
            let _ = unsafe { CloseHandle(mutex) };
            return forwarded.map(|()| Claim::Forwarded);
        }
        log::warn!(
            "another instance holds the single instance mutex but has no message window; starting anyway"
        );
    }

    match create_message_window(&names, sink) {
        Ok(window) => Ok(Claim::Primary(PrimaryInstance { mutex, window })),
        Err(err) => {
            unsafe {
                let _ = ReleaseMutex(mutex);
                let _ = CloseHandle(mutex);
            }
            Err(err)
        }
    }
}

fn find_primary_window(names: &Names) -> Option<HWND> {
    let deadline = Instant::now() + FIND_WINDOW_TIMEOUT;
    loop {
        if let Ok(window) = unsafe { FindWindowW(&names.class, &names.window) } {
            return Some(window);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(FIND_WINDOW_INTERVAL);
    }
}

fn forward(target: HWND, invocation: &Invocation) -> io::Result<()> {
    let data = encode_pipe(invocation);
    let copy = COPYDATASTRUCT {
        dwData: COPYDATA_TAG,
        cbData: u32::try_from(data.len()).map_err(io::Error::other)?,
        lpData: data.as_ptr() as *mut c_void,
    };

    let mut reply = 0;
    let sent = unsafe {
        SendMessageTimeoutW(
            target,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&copy as *const COPYDATASTRUCT as isize),
            SMTO_ABORTIFHUNG,
            FORWARD_TIMEOUT_MS,
            Some(&mut reply),
        )
    };
    if sent.0 == 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

fn create_message_window(names: &Names, sink: Sink) -> io::Result<HWND> {
    let instance = unsafe { GetModuleHandleW(None) }.map_err(io::Error::other)?;
    let class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(message_window_proc),
        hInstance: instance.into(),
        lpszClassName: PCWSTR(names.class.as_ptr()),
        ..Default::default()
    };
    // 同一进程里第二次注册同名类会失败，类仍可用，交给 CreateWindowExW 判断。
    unsafe { RegisterClassExW(&class) };

    let sink = Box::into_raw(Box::new(sink));
    let created = unsafe {
        CreateWindowExW(
            WS_EX_NOACTIVATE | WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_TOOLWINDOW,
            &names.class,
            &names.window,
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance.into()),
            Some(sink as *const c_void),
        )
    };
    let window = match created {
        Ok(window) => window,
        Err(err) => {
            drop(unsafe { Box::from_raw(sink) });
            return Err(io::Error::other(err));
        }
    };

    // 与插件相同：可见但 0×0 且分层，用户看不到。
    unsafe { SetWindowLongPtrW(window, GWL_STYLE, (WS_VISIBLE | WS_POPUP).0 as isize) };
    // 以管理员身份运行的主实例也能收到普通权限第二实例发来的参数。
    if let Err(err) =
        unsafe { ChangeWindowMessageFilterEx(window, WM_COPYDATA, MSGFLT_ALLOW, None) }
    {
        log::warn!("single instance window could not allow WM_COPYDATA through UIPI: {err}");
    }

    Ok(window)
}

unsafe extern "system" fn message_window_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_NCCREATE => {
            let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize) };
        }
        WM_COPYDATA => {
            receive(hwnd, lparam);
            return LRESULT(1);
        }
        WM_NCDESTROY => {
            let sink = unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) } as *mut Sink;
            if !sink.is_null() {
                drop(unsafe { Box::from_raw(sink) });
            }
        }
        _ => {}
    }

    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn receive(hwnd: HWND, lparam: LPARAM) {
    let copy = unsafe { &*(lparam.0 as *const COPYDATASTRUCT) };
    let sink = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const Sink;
    if copy.dwData != COPYDATA_TAG || copy.lpData.is_null() || sink.is_null() {
        return;
    }

    let bytes =
        unsafe { std::slice::from_raw_parts(copy.lpData as *const u8, copy.cbData as usize) };
    let invocation = decode_pipe(bytes);
    let sink = unsafe { &*sink };
    // 窗口过程里 panic 会直接终止进程。
    if catch_unwind(AssertUnwindSafe(|| sink(invocation))).is_err() {
        log::error!("single instance handler panicked");
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::*;

    fn unique_identifier(name: &str) -> String {
        format!(
            "com.fastthree.kwikpaste.os-test.{name}.{}",
            std::process::id()
        )
    }

    fn primary(identifier: &str, sender: mpsc::Sender<Invocation>) -> PrimaryInstance {
        let claim = claim(
            identifier,
            Box::new(move |invocation| {
                let _ = sender.send(invocation);
            }),
        )
        .expect("claim");
        match claim {
            Claim::Primary(primary) => primary,
            Claim::Forwarded => panic!("expected to be the primary instance"),
        }
    }

    #[test]
    fn second_claim_forwards_cwd_and_args_to_the_primary() {
        let identifier = unique_identifier("forward");
        let (sender, receiver) = mpsc::channel();
        let guard = primary(&identifier, sender);

        // 主实例窗口属于本线程，SendMessageTimeoutW 会直接调用它的窗口过程。
        let second = claim(&identifier, Box::new(|_| {})).expect("second claim");
        assert!(matches!(second, Claim::Forwarded));

        let received = receiver.try_recv().expect("forwarded invocation");
        assert_eq!(received, current_invocation());

        drop(guard);
    }

    #[test]
    fn name_is_free_again_after_the_primary_is_dropped() {
        let identifier = unique_identifier("release");
        let (sender, _receiver) = mpsc::channel();
        drop(primary(&identifier, sender));

        let (sender, _receiver) = mpsc::channel();
        let again = primary(&identifier, sender);
        drop(again);
    }

    #[test]
    fn production_names_match_the_tauri_plugin() {
        let names = Names::new("com.fastthree.kwikpaste");

        assert_eq!(names.class.to_string(), "com.fastthree.kwikpaste-sic");
        assert_eq!(names.window.to_string(), "com.fastthree.kwikpaste-siw");
        assert_eq!(names.mutex.to_string(), "com.fastthree.kwikpaste-sim");
    }
}
