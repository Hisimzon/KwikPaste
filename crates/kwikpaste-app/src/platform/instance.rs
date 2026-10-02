//! 单实例的主实例一侧：保管守卫，处理后启动的实例转交来的参数。

use async_channel::{Receiver, Sender};
use gpui::{App, AsyncApp, Global};
use kwikpaste_os::single_instance::{Invocation, PrimaryInstance};

use super::editing::EditTrigger;
use super::panel::{PanelCommand, Trigger, TriggerSource};
use super::probe;
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
        if cx.has_global::<Instance>() {
            cx.global_mut::<Instance>().guard = None;
        }
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
            selftest::QUIT => {
                probe::quitting();
                cx.update(|cx| cx.quit());
            }
            _ => {
                let Some(patch) = arg.strip_prefix(selftest::SETTINGS) else {
                    continue;
                };
                update_settings(patch, cx).await;
            }
        }
        return true;
    }

    false
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
