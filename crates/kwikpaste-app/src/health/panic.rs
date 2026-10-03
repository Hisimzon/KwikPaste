//! panic hook：记录、按重启策略处理，再交给原来的 hook。不 catch、不改变 panic 的去向：
//! 主线程上的 panic 照样让进程退出（窗口过程里是 `0xC0000409`），其它线程上的照样结束该线程。

use std::any::Any;
use std::backtrace::Backtrace;
use std::cell::Cell;
use std::panic::PanicHookInfo;

use super::{crash, logger};

thread_local! {
    /// hook 里再次 panic 时不再进重启逻辑。
    static IN_HOOK: Cell<bool> = const { Cell::new(false) };
}

pub fn install() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        handle(info);
        previous(info);
    }));
}

fn handle(info: &PanicHookInfo<'_>) {
    if IN_HOOK.with(|flag| flag.replace(true)) {
        return;
    }

    let thread = std::thread::current();
    let thread = thread.name().unwrap_or("<unnamed>").to_owned();
    let location = info.location().map(ToString::to_string).unwrap_or_default();
    let message = payload_text(info.payload());
    logger::write_crash(&format!(
        "panic on thread '{thread}' at {location}: {message}\n{}",
        Backtrace::force_capture()
    ));

    let on_main_thread = thread == "main";
    crash::on_panic(
        crash::CrashInfo {
            thread,
            location,
            message,
        },
        on_main_thread,
    );

    IN_HOOK.with(|flag| flag.set(false));
}

fn payload_text(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        return (*text).to_owned();
    }
    if let Some(text) = payload.downcast_ref::<String>() {
        return text.clone();
    }

    "<non-string panic payload>".to_owned()
}
