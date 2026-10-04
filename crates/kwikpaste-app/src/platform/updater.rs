//! 更新器的宿主：建 [`Updater`]、开始后台调度，实现安装交接要的 [`HandoffHost`]。
//!
//! 交接的每一步由更新器在 core 的 runtime 上按顺序 await；这里把请求转进主线程，各占一个事件循环
//! `UpdaterUi` 把更新状态送进 GPUI 更新窗，把公告交给原生对话框。
//! 链接只有在更新器收到 action 结果之后才打开。
//!
//! 开发构建（`target\…` 里的 exe）判定为 `InstallKind::Unmanaged`、`AppEnv::Dev`：不检查、不下载、
//! 不安装，也不上报统计、不拉公告，只记下检查时间。

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use async_channel::Sender;
use futures::channel::oneshot;
use gpui::{
    App, AppContext as _, AsyncApp, Context, Entity, Global, IntoElement, ParentElement as _,
    Render, Styled as _, TitlebarOptions, Window, WindowBounds, WindowOptions, div,
    prelude::FluentBuilder as _, px, size,
};
use kwikpaste_ui::theme::TextSize;
use kwikpaste_ui::{Button, KpStyled as _, theme};
use kwikpaste_updater::{
    AnnouncementOutcome, AnnouncementPrompt, DownloadProgress, HandoffHost, HostFuture,
    UpdateMetadata, UpdateStatus, Updater, UpdaterUi,
};
#[cfg(target_os = "windows")]
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::panel::{PanelCommand, Trigger, TriggerSource};
use super::{hotkey, instance, tray};
use crate::{
    core_host,
    i18n::{t, t_args},
};

/// 交接要求的退出码；`main` 在 GPUI 的循环结束后据此退出。
static EXIT_CODE: AtomicI32 = AtomicI32::new(0);

/// 运行中的更新器，UI 的更新窗、偏好页经 [`updater`] 取用。
pub struct UpdaterHost(Updater);

impl Global for UpdaterHost {}

/// 当前的更新器；没建起来时为 `None`。
#[allow(
    dead_code,
    reason = "UI 接线用的接口：更新窗与偏好页经它检查、下载、安装"
)]
pub fn updater(cx: &App) -> Option<&Updater> {
    cx.try_global::<UpdaterHost>().map(|host| &host.0)
}

/// 交接退出时要求的退出码（没有交接时为 0）。
pub fn exit_code() -> i32 {
    EXIT_CODE.load(Ordering::SeqCst)
}

/// 建更新器并开始调度。失败只记日志：更新器不可用不影响应用运行。
pub fn start(cx: &mut App) {
    let Some(core) = core_host::core(cx).cloned() else {
        return;
    };
    let (requests, receiver) = async_channel::unbounded();
    let (ui_requests, ui_receiver) = async_channel::unbounded();
    cx.set_global(UiBus {
        requests: ui_requests.clone(),
    });
    cx.set_global(HandoffRequests(requests.clone()));
    let host = Arc::new(Handoff { requests });

    let created = match Updater::new(
        core,
        Arc::new(Ui {
            requests: ui_requests.clone(),
        }),
        host,
    ) {
        Ok(created) => created,
        Err(err) => {
            log::error!("the updater is unavailable: {err}");
            return;
        }
    };
    created.start();
    cx.set_global(UpdaterHost(created));
    cx.on_app_quit(|cx| {
        if let Some(updater) = updater(cx) {
            updater.stop();
        }
        async {}
    })
    .detach();

    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(request) = receiver.recv().await {
            cx.update(|cx| serve(request, cx));
        }
    })
    .detach();

    cx.spawn(async move |cx: &mut AsyncApp| {
        while let Ok(request) = ui_receiver.recv().await {
            cx.update(|cx| serve_ui(request, cx));
        }
    })
    .detach();
}

/// 开发自测：不请求网络，直接以一个固定候选版本展示更新窗。
pub fn selftest_update_window(cx: &mut App) {
    let Some(updater) = updater(cx) else {
        return;
    };
    let mut status = updater.status();
    status.update = Some(UpdateMetadata {
        current_version: status.current_version.clone(),
        version: "9.9.9-selftest".to_owned(),
        channel: "stable",
        date: Some("2026-10-03".to_owned()),
        body: Some("Self-test release notes\n\nThis window is a local UI preview.".to_owned()),
        target: "selftest".to_owned(),
        download_url: "https://example.invalid/selftest".to_owned(),
        downloaded: false,
        release_notes_url: "https://example.invalid/selftest".to_owned(),
    });
    serve_ui(UiRequest::Update(Box::new(status)), cx);
}

/// 开发自测：展示公告按钮顺序，不打开真实链接。
pub fn selftest_announcement() {
    let prompt = AnnouncementPrompt {
        id: "selftest-announcement".to_owned(),
        important: true,
        title: "KwikPaste announcement".to_owned(),
        body: "This is a native announcement dialog self-test.".to_owned(),
        buttons: vec![
            kwikpaste_updater::AnnouncementButton {
                role: kwikpaste_updater::ButtonRole::Action,
                label: "Open details".to_owned(),
            },
            kwikpaste_updater::AnnouncementButton {
                role: kwikpaste_updater::ButtonRole::Close,
                label: "Later".to_owned(),
            },
        ],
    };
    log::info!(
        "announcement self-test result: {:?}",
        show_announcement(prompt)
    );
}

/// 交接请求的入口，自测演练交接时用。
struct HandoffRequests(Sender<Request>);

impl Global for HandoffRequests {}

/// 自测（`--selftest-handoff=<code>`）：按更新器的顺序走一遍交接的宿主步骤（停输入、删托盘、
/// 释放单实例），停 2 秒让探针检查状态，再以 `code` 退出。走的是与 [`HandoffHost`] 完全相同的
/// 主线程通道；不下载、不安装、不启动任何程序。
pub fn rehearse_handoff(cx: &mut App, code: i32) {
    let Some(requests) = cx.try_global::<HandoffRequests>() else {
        log::warn!("the updater is not running; no handoff to rehearse");
        return;
    };
    let host = Handoff {
        requests: requests.0.clone(),
    };
    let pause = cx.background_executor().clone();
    cx.background_executor()
        .spawn(async move {
            host.stop_input().await;
            host.remove_tray().await;
            host.release_single_instance().await;
            super::probe::handoff_rehearsed();
            pause.timer(std::time::Duration::from_secs(2)).await;
            host.exit(code);
        })
        .detach();
}

/// 交接请求，在主线程上执行。
enum Request {
    StopInput(oneshot::Sender<()>),
    RemoveTray(oneshot::Sender<()>),
    ReleaseSingleInstance(oneshot::Sender<()>),
    Exit(i32),
}

fn serve(request: Request, cx: &mut App) {
    match request {
        Request::StopInput(done) => {
            log::info!("update handoff: stopping input");
            hotkey::unregister_all(cx);
            super::request(cx, PanelCommand::Hide(Trigger::now(TriggerSource::Ui)));
            #[cfg(target_os = "windows")]
            {
                kwikpaste_os::win::keyboard::stop();
                kwikpaste_os::win::mouse::stop_outside_click();
            }
            let _ = done.send(());
        }
        Request::RemoveTray(done) => {
            log::info!("update handoff: removing the tray icon");
            tray::remove(cx);
            let _ = done.send(());
        }
        Request::ReleaseSingleInstance(done) => {
            log::info!("update handoff: releasing the single instance");
            instance::release(cx);
            let _ = done.send(());
        }
        Request::Exit(code) => {
            log::info!("update handoff: exiting with {code}");
            // The installer intentionally terminates this process after the handoff.  Tell the
            // external watchdog before releasing the app so that the termination is not treated
            // as an unclean crash and relaunched during replacement.
            crate::health::suppress_watchdog_restart();
            EXIT_CODE.store(code, Ordering::SeqCst);
            core_host::mark_shut_down();
            cx.quit();
        }
    }
}

/// [`HandoffHost`] 的实现：把每一步转进主线程并等它做完。
struct Handoff {
    requests: Sender<Request>,
}

impl Handoff {
    fn round_trip(
        &self,
        request: impl FnOnce(oneshot::Sender<()>) -> Request,
    ) -> HostFuture<'_, ()> {
        let (done, finished) = oneshot::channel();
        let request = request(done);
        Box::pin(async move {
            if self.requests.send(request).await.is_ok() {
                let _ = finished.await;
            }
        })
    }
}

impl HandoffHost for Handoff {
    fn stop_input(&self) -> HostFuture<'_, ()> {
        self.round_trip(Request::StopInput)
    }

    fn remove_tray(&self) -> HostFuture<'_, ()> {
        self.round_trip(Request::RemoveTray)
    }

    fn release_single_instance(&self) -> HostFuture<'_, ()> {
        self.round_trip(Request::ReleaseSingleInstance)
    }

    fn exit(&self, code: i32) {
        if self.requests.try_send(Request::Exit(code)).is_err() {
            log::error!("update handoff could not reach the main thread; exiting directly");
            std::process::exit(code);
        }
    }
}

/// 后台更新器和 GPUI 之间的 UI 适配。
#[derive(Clone)]
struct Ui {
    requests: Sender<UiRequest>,
}

enum UiRequest {
    Update(Box<UpdateStatus>),
    Progress(DownloadProgress),
}

impl UpdaterUi for Ui {
    fn update_available(&self, status: UpdateStatus) {
        if self
            .requests
            .try_send(UiRequest::Update(Box::new(status)))
            .is_err()
        {
            log::debug!("the updater window is no longer available");
        }
    }

    fn show_announcement(&self, prompt: AnnouncementPrompt) -> HostFuture<'_, AnnouncementOutcome> {
        let outcome = show_announcement(prompt);
        Box::pin(async move { outcome })
    }

    fn open_url(&self, url: &str) {
        if let Err(err) = kwikpaste_os::dialogs::open_url(url) {
            log::warn!("announcement link could not be opened: {err}");
        }
    }
}

fn show_announcement(prompt: AnnouncementPrompt) -> AnnouncementOutcome {
    let buttons: Vec<kwikpaste_os::dialogs::DialogButton> = prompt
        .buttons
        .iter()
        .map(|button| kwikpaste_os::dialogs::DialogButton::new(button.label.clone()))
        .collect();
    let selected = kwikpaste_os::dialogs::show(&prompt.title, &prompt.body, &buttons);
    let Some(index) = selected else {
        return AnnouncementOutcome::Closed;
    };
    prompt
        .buttons
        .get(index)
        .map_or(AnnouncementOutcome::Closed, |button| button.role.outcome())
}

struct UpdateWindowHost {
    handle: gpui::AnyWindowHandle,
    view: Entity<UpdateWindow>,
}

impl Global for UpdateWindowHost {}

fn serve_ui(request: UiRequest, cx: &mut App) {
    match request {
        UiRequest::Update(status) => open_update_window(*status, cx),
        UiRequest::Progress(progress) => {
            if let Some((view, _handle)) = cx
                .try_global::<UpdateWindowHost>()
                .map(|host| (host.view.clone(), host.handle))
            {
                view.update(cx, |view, cx| {
                    view.progress = Some(progress);
                    cx.notify();
                });
            }
        }
    }
}

fn open_update_window(status: UpdateStatus, cx: &mut App) {
    if let Some((view, handle)) = cx
        .try_global::<UpdateWindowHost>()
        .map(|host| (host.view.clone(), host.handle))
    {
        view.update(cx, |view, cx| view.set_status(status, cx));
        raise_update_window(handle, cx);
        return;
    }
    let Some(updater) = updater(cx).cloned() else {
        return;
    };
    let Some(requests) = cx.try_global::<UiBus>().map(|bus| bus.requests.clone()) else {
        return;
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(560.), px(420.)), cx)),
        titlebar: Some(TitlebarOptions {
            title: Some(t("common:update.title")),
            ..Default::default()
        }),
        focus: false,
        ..Default::default()
    };
    match crate::platform::open_window(options, cx, |_, cx| {
        cx.new(|_| UpdateWindow::new(updater.clone(), status, requests))
    }) {
        Ok((handle, view)) => {
            raise_update_window(handle, cx);
            cx.set_global(UpdateWindowHost { handle, view });
            let window_id = handle.window_id();
            cx.on_window_closed(move |cx, closed_id| {
                if closed_id != window_id {
                    return;
                }
                let is_update_window = cx
                    .try_global::<UpdateWindowHost>()
                    .is_some_and(|host| host.handle.window_id() == closed_id);
                if is_update_window {
                    let _ = cx.remove_global::<UpdateWindowHost>();
                }
            })
            .detach();
        }
        Err(err) => log::warn!("update window could not be opened: {err:#}"),
    }
}

fn raise_update_window(handle: gpui::AnyWindowHandle, cx: &mut App) {
    #[cfg(target_os = "windows")]
    {
        if let Err(err) = handle.update(cx, |_, window, _| {
            let Ok(handle) = HasWindowHandle::window_handle(window) else {
                return;
            };
            let RawWindowHandle::Win32(handle) = handle.as_raw() else {
                return;
            };
            if !kwikpaste_os::win::keyboard::swallow_marked_alt(std::time::Duration::from_millis(
                50,
            )) {
                log::debug!(
                    "the keyboard hook did not confirm the marked Alt for the update window"
                );
                return;
            }
            if !kwikpaste_os::win::set_foreground(handle.hwnd.get()) {
                log::debug!("SetForegroundWindow did not accept the update window");
            }
        }) {
            log::debug!("the update window could not be raised: {err:#}");
        }
    }
    #[cfg(target_os = "macos")]
    let _ = (handle, cx);
}

struct UpdateWindow {
    updater: Updater,
    requests: Sender<UiRequest>,
    status: UpdateStatus,
    progress: Option<DownloadProgress>,
    downloaded: Option<UpdateMetadata>,
    downloading: bool,
    installing: bool,
    error: Option<String>,
}

impl UpdateWindow {
    fn new(updater: Updater, status: UpdateStatus, requests: Sender<UiRequest>) -> Self {
        Self {
            updater,
            requests,
            status,
            progress: None,
            downloaded: None,
            downloading: false,
            installing: false,
            error: None,
        }
    }

    fn set_status(&mut self, status: UpdateStatus, cx: &mut Context<Self>) {
        self.status = status;
        self.error = None;
        cx.notify();
    }

    fn download(&mut self, cx: &mut Context<Self>) {
        if self.downloading || self.installing {
            return;
        }
        let Some(update) = self.status.update.clone() else {
            return;
        };
        let version = update.version.clone();
        let updater = self.updater.clone();
        let requests = self.requests.clone();
        self.downloading = true;
        self.progress = Some(DownloadProgress {
            downloaded: 0,
            total: None,
            progress: Some(0.),
        });
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = updater
                .download(version, move |progress| {
                    let _ = requests.try_send(UiRequest::Progress(progress));
                })
                .await;
            if let Err(err) = view.update(cx, |view, cx| {
                view.downloading = false;
                match result {
                    Ok(metadata) => view.downloaded = Some(metadata),
                    Err(err) => view.error = Some(err.to_string()),
                }
                cx.notify();
            }) {
                log::debug!("the update window closed before download completed: {err:#}");
            }
        })
        .detach();
    }

    fn install(&mut self, cx: &mut Context<Self>) {
        if self.installing {
            return;
        }
        let Some(update) = self.downloaded.as_ref().or(self.status.update.as_ref()) else {
            return;
        };
        if !update.downloaded && self.downloaded.is_none() {
            return;
        }
        let version = update.version.clone();
        let updater = self.updater.clone();
        self.installing = true;
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = updater.install(version).await;
            if let Err(err) = view.update(cx, |view, cx| {
                view.installing = false;
                if let Err(err) = result {
                    view.error = Some(err.to_string());
                }
                cx.notify();
            }) {
                log::debug!("the update window closed before installation completed: {err:#}");
            }
        })
        .detach();
    }

    fn skip(&mut self, cx: &mut Context<Self>) {
        let Some(update) = self.status.update.clone() else {
            return;
        };
        let updater = self.updater.clone();
        cx.spawn(async move |view, cx| {
            let result = updater.skip(update.version).await;
            if let Err(err) = view.update(cx, |view, cx| {
                if let Ok(status) = result {
                    view.status = status;
                }
                cx.notify();
            }) {
                log::debug!("the update window closed before skip completed: {err:#}");
            }
        })
        .detach();
    }
}

impl Render for UpdateWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx);
        let update = self.status.update.as_ref();
        let title = update
            .map(|update| t_args("common:update.available", &[("version", &update.version)]))
            .unwrap_or_else(|| t("common:update.title"));
        let notes = update
            .and_then(|update| update.body.as_deref())
            .map(str::to_owned)
            .unwrap_or_else(|| t("common:update.unsupported").to_string());
        let progress = self.progress.and_then(|progress| progress.progress);
        let progress_text = progress.map(|value| format!("{:.0}", value * 100.));
        let downloaded =
            self.downloaded.is_some() || update.is_some_and(|update| update.downloaded);
        let state_text = if self.installing {
            Some(t("common:update.installing"))
        } else if downloaded {
            Some(t("common:update.downloaded"))
        } else {
            None
        };
        div()
            .flex()
            .flex_col()
            .gap(px(16.))
            .p(px(24.))
            .bg(crate::platform::material::surface_tint(
                cx,
                tokens.bg_container,
                0.58,
                0.34,
            ))
            .text_color(tokens.text)
            .child(div().kp_text(TextSize::Lg).child(title))
            .child(
                div()
                    .kp_text(TextSize::Sm)
                    .text_color(tokens.secondary)
                    .child(t_args(
                        "common:update.current",
                        &[("version", &self.status.current_version)],
                    )),
            )
            .child(div().kp_text(TextSize::Sm).child(t("common:update.notes")))
            .child(div().flex_1().child(notes))
            .when_some(state_text, |element, state| {
                element.child(div().kp_text(TextSize::Sm).child(state))
            })
            .when_some(self.error.as_deref(), |element, error| {
                element.child(
                    div()
                        .kp_text(TextSize::Sm)
                        .text_color(tokens.error)
                        .child(t_args("common:update.error", &[("message", error)])),
                )
            })
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .when_some(progress_text, |element, progress| {
                        element.child(div().flex_1().kp_text(TextSize::Sm).child(t_args(
                            "common:update.downloading",
                            &[("progress", &progress)],
                        )))
                    })
                    .child(
                        Button::new("update-later", t("common:update.later"))
                            .on_click(|_, window, _| window.remove_window()),
                    )
                    .when(update.is_some_and(|update| !update.downloaded), |element| {
                        element.child(
                            Button::new("update-skip", t("common:update.skip"))
                                .on_click(cx.listener(|view, _, _, cx| view.skip(cx))),
                        )
                    })
                    .when(
                        self.downloaded.is_some() || update.is_some_and(|update| update.downloaded),
                        |element| {
                            element.child(
                                Button::new("update-install", t("common:update.install"))
                                    .primary()
                                    .on_click(cx.listener(|view, _, _, cx| view.install(cx))),
                            )
                        },
                    )
                    .when(
                        self.downloaded.is_none()
                            && update.is_some_and(|update| !update.downloaded),
                        |element| {
                            element.child(
                                Button::new("update-download", t("common:update.download"))
                                    .primary()
                                    .on_click(cx.listener(|view, _, _, cx| view.download(cx))),
                            )
                        },
                    ),
            )
    }
}

struct UiBus {
    requests: Sender<UiRequest>,
}

impl Global for UiBus {}
