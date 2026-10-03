//! 单实例的主实例一侧：保管守卫，处理后启动的实例转交来的参数。

use async_channel::{Receiver, Sender};
use gpui::{App, AsyncApp, Global};
use kwikpaste_os::single_instance::{Invocation, PrimaryInstance};

use super::editing::EditTrigger;
use super::panel::{PanelCommand, Trigger, TriggerSource};
use super::{paste, probe, updater, watchdog};
use crate::{core_host, selftest};

/// 开机自启带的参数：第二实例带它时静默退出，主实例什么也不做（与 1.x 相同）。
const AUTO_LAUNCH: &str = "--auto-launch";

/// 持有主实例守卫：退出前丢弃，释放单实例名字。
struct Instance {
    guard: Option<PrimaryInstance>,
}

impl Global for Instance {}

pub fn serve(
    cx: &mut App,
    guard: PrimaryInstance,
    invocations: Receiver<Invocation>,
    commands: Sender<PanelCommand>,
) {
    cx.set_global(Instance { guard: Some(guard) });
    cx.on_app_quit(|cx| {
        release(cx);
        async {}
    })
    .detach();

    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(invocation) = invocations.recv().await {
            handle(&invocation, &commands, cx).await;
        }
    })
    .detach();
}

/// 释放单实例（关互斥体、销毁消息窗口）：退出前、更新交接拉起新进程之前调用。必须在主线程上：
/// 消息窗口只能由创建它的线程销毁。
pub fn release(cx: &mut App) {
    if cx.has_global::<Instance>() {
        cx.global_mut::<Instance>().guard = None;
    }
}

async fn handle(invocation: &Invocation, commands: &Sender<PanelCommand>, cx: &mut AsyncApp) {
    let args = invocation.args.get(1..).unwrap_or_default();
    log::info!("another launch handed over {args:?}");

    if selftest::enabled(selftest::PLATFORM) && handle_selftest(args, commands, cx).await {
        return;
    }
    if args.iter().any(|arg| arg == AUTO_LAUNCH) {
        return;
    }

    // TODO：未完成引导开引导窗、否则开偏好窗（与 1.x 相同）；这两个窗口建好之前先唤起面板。
    let trigger = Trigger::now(TriggerSource::SecondInstance);
    let _ = commands.try_send(PanelCommand::Show(trigger));
}

/// 平台自测的远程命令（主实例本身也处于 `--selftest-platform` 时才接受）；处理了返回 `true`。
async fn handle_selftest(
    args: &[String],
    commands: &Sender<PanelCommand>,
    cx: &mut AsyncApp,
) -> bool {
    let trigger = Trigger::now(TriggerSource::SecondInstance);
    for arg in args {
        let command = match arg.as_str() {
            selftest::SHOW => Some(PanelCommand::Show(trigger)),
            selftest::HIDE => Some(PanelCommand::Hide(trigger)),
            selftest::TOGGLE => Some(PanelCommand::Toggle(trigger)),
            selftest::EDIT => Some(PanelCommand::BeginEditing(EditTrigger::Keyboard)),
            selftest::END_EDIT => Some(PanelCommand::EndEditing),
            _ => None,
        };
        if let Some(command) = command {
            let _ = commands.try_send(command);
            return true;
        }

        match arg.as_str() {
            selftest::IME_STATE => probe::ime_state(),
            selftest::IME_NATIVE => probe::set_ime_native_mode(),
            selftest::READ_NOW => read_now(cx).await,
            selftest::COUNT => count(cx).await,
            selftest::QUIT => {
                probe::quitting();
                cx.update(|cx| cx.quit());
            }
            selftest::VSYNC_DEAD => watchdog::simulate_dead_render_thread(),
            _ => {
                if let Some(place) = arg.strip_prefix(selftest::PANIC) {
                    selftest_panic(place);
                } else if let Some(patch) = arg.strip_prefix(selftest::SETTINGS) {
                    update_settings(patch, cx).await;
                } else if let Some(code) = arg.strip_prefix(selftest::HANDOFF) {
                    let code = code.parse().unwrap_or(0);
                    cx.update(|cx| updater::rehearse_handoff(cx, code));
                } else if let Some(id) = arg.strip_prefix(selftest::COPY_ITEM) {
                    let copied = cx.update(|cx| paste::copy(cx, id.to_owned(), false, true));
                    if let Err(err) = copied.await {
                        log::error!("selftest copy of {id} failed: {err}");
                    }
                } else {
                    continue;
                }
            }
        }
        return true;
    }

    false
}

/// `--selftest-panic=main|thread`：故意 panic，验证 panic hook 和崩溃重启。`main` 在这个前台任务里
/// （主线程的窗口过程内，进程随即 abort），`thread` 在一个新线程上（进程活着，走有序重启）。
fn selftest_panic(place: &str) {
    log::warn!("selftest: panicking on {place}");
    if place == "thread" {
        let spawned = std::thread::Builder::new()
            .name("selftest-panic".to_owned())
            .spawn(|| panic!("selftest panic on a worker thread"));
        if let Err(err) = spawned {
            log::error!("selftest panic thread did not start: {err}");
        }
        return;
    }
    panic!("selftest panic on the main thread");
}

/// `--selftest-read-now`：手动读取一次剪贴板，结果写进探针日志。
async fn read_now(cx: &mut AsyncApp) {
    let Some(core) = cx.update(|cx| core_host::core(cx).cloned()) else {
        return;
    };
    probe::read_now(&core.read_clipboard_now().await);
}

/// `--selftest-count`：历史记录总数写进探针日志。
async fn count(cx: &mut AsyncApp) {
    let Some(core) = cx.update(|cx| core_host::core(cx).cloned()) else {
        return;
    };
    let query = kwikpaste_core::db::models::ClipboardItemQuery {
        limit: 1,
        ..Default::default()
    };
    match core.list_items(query).await {
        Ok(page) => probe::count(page.total),
        Err(err) => log::error!("selftest count failed: {err}"),
    }
}

/// `--selftest-settings=<JSON patch>`：经 core 更新设置，走与偏好页相同的 `SettingsUpdated` 路径。
async fn update_settings(patch: &str, cx: &mut AsyncApp) {
    let patch = match serde_json::from_str::<serde_json::Value>(patch) {
        Ok(patch) => patch,
        Err(err) => {
            log::error!("selftest settings patch is not JSON: {err}");
            return;
        }
    };
    let Some(core) = cx.update(|cx| core_host::core(cx).cloned()) else {
        return;
    };
    if let Err(err) = core.update_settings(patch).await {
        log::error!("selftest settings patch was rejected: {err}");
    }
}
