//! core 事件进 GPUI，以及平台层跟随设置：热键、托盘、界面语言。
//!
//! core 在自己的 runtime 线程上发事件，这里转进 channel，由主线程任务从 [`CoreEvents`] 实体
//! 发出；平台层和 UI 都订阅这个实体。

use async_channel::Receiver;
use gpui::{App, AppContext as _, Entity, EventEmitter, Global};
use kwikpaste_core::CoreEvent;
use kwikpaste_core::settings::Language;

use super::{hotkey, tray};
use crate::core_host;

/// core 事件的发送者：订阅它即可收到 [`CoreEvent`]（设置变更、记录入库、清理……）。
pub struct CoreEvents;

impl EventEmitter<CoreEvent> for CoreEvents {}

struct CoreEventsHub(Entity<CoreEvents>);

impl Global for CoreEventsHub {}

/// core 事件实体；平台层启动之前（或展示窗模式下）为 `None`。
pub fn core_events(cx: &App) -> Option<Entity<CoreEvents>> {
    cx.try_global::<CoreEventsHub>().map(|hub| hub.0.clone())
}

/// 把 core 的事件流接到 [`CoreEvents`] 实体上，并在退出前关闭 core（数据库借此做 WAL checkpoint）。
pub fn serve(cx: &mut App, events: Receiver<CoreEvent>) {
    let hub = cx.new(|_| CoreEvents);
    cx.set_global(CoreEventsHub(hub.clone()));

    let emitter = hub.clone();
    cx.spawn(async move |cx| {
        while let Ok(event) = events.recv().await {
            cx.update(|cx| emitter.update(cx, |_, cx| cx.emit(event)));
        }
    })
    .detach();

    cx.on_app_quit(|cx| {
        let core = core_host::core(cx).cloned();
        async move {
            if let Some(core) = core
                && let Err(err) = core.shutdown().await
            {
                log::warn!("core did not shut down cleanly: {err}");
            }
        }
    })
    .detach();
}

/// 平台层跟随设置变更：快捷键重新注册，语言变了重建托盘菜单，托盘显隐。
pub fn follow(cx: &mut App) {
    let Some(hub) = core_events(cx) else {
        return;
    };
    cx.subscribe(&hub, |_, event: &CoreEvent, cx| {
        let CoreEvent::SettingsUpdated { settings, delta } = event else {
            return;
        };
        if delta.touches("shortcuts.openClipboard") {
            hotkey::apply(&settings.shortcuts, cx);
        }
        let language_changed = delta.touches("appearance.language");
        if language_changed {
            apply_language(cx);
        }
        if language_changed || delta.touches("general.trayIcon") {
            tray::apply(settings, cx);
        }
    })
    .detach();
}

/// 界面语言跟随设置 `appearance.language`。
pub fn apply_language(cx: &mut App) {
    let Some(core) = core_host::core(cx) else {
        return;
    };
    let language = match core.language() {
        Language::ZhCN => crate::i18n::Language::ZhCn,
        Language::EnUS => crate::i18n::Language::EnUs,
    };
    if crate::i18n::language() != language {
        crate::i18n::set_language(language, cx);
    }
}
