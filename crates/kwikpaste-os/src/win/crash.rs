//! 原生崩溃（访问冲突等没人处理的 SEH 异常）的最后一道处理：`SetUnhandledExceptionFilter`。
//!
//! 回调里记日志、拉起重启的进程，然后交还系统（`EXCEPTION_CONTINUE_SEARCH`），进程照常结束，
//! 崩溃不会被吞掉。`__fastfail`（含 `panic = "abort"` 的 abort、栈溢出）不经过这里：前者走 panic
//! hook，后者只能在下次启动时由运行标记发现。

use std::sync::OnceLock;

use windows::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, NTSTATUS};
use windows::Win32::System::Diagnostics::Debug::{
    EXCEPTION_POINTERS, RaiseException, SetUnhandledExceptionFilter,
};

/// 回调：异常码和出错地址。在崩溃的线程上调用，只能做少量工作。
type Handler = fn(code: u32, address: usize);

static HANDLER: OnceLock<Handler> = OnceLock::new();
const EXCEPTION_CONTINUE_SEARCH: i32 = 0;
const EXCEPTION_NONCONTINUABLE: u32 = 1;

/// 装上处理器（进程内一次）。
pub fn install(handler: Handler) {
    if HANDLER.set(handler).is_ok() {
        unsafe { SetUnhandledExceptionFilter(Some(filter)) };
    }
}

unsafe extern "system" fn filter(info: *const EXCEPTION_POINTERS) -> i32 {
    let record = unsafe { info.as_ref().and_then(|info| info.ExceptionRecord.as_ref()) };
    if let (Some(handler), Some(record)) = (HANDLER.get(), record) {
        handler(
            record.ExceptionCode.0 as u32,
            record.ExceptionAddress as usize,
        );
    }

    EXCEPTION_CONTINUE_SEARCH
}

/// 自测：在当前线程上抛一个没人处理的访问冲突。
pub fn raise_access_violation() {
    let NTSTATUS(code) = EXCEPTION_ACCESS_VIOLATION;
    unsafe { RaiseException(code as u32, EXCEPTION_NONCONTINUABLE, None) };
}
