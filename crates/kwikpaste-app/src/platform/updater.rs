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
use std::time::Duration;

use async_channel::Sender;
use futures::channel::oneshot;
use gpui::{
    Animation, AnimationExt as _, App, AppContext as _, AsyncApp, Context, Entity, Global,
    ImageSource, InteractiveElement as _, IntoElement, ParentElement as _, Render,
    StatefulInteractiveElement as _, Styled as _, TitlebarOptions, Window, WindowBounds,
    WindowOptions, div, img, prelude::FluentBuilder as _, pulsating_between, px, relative, size,
};
use kwikpaste_ui::theme::TextSize;
use kwikpaste_ui::{Button, KpStyled as _, theme};
use kwikpaste_updater::{
    AnnouncementOutcome, AnnouncementPrompt, CheckMode, DownloadProgress, HandoffHost, HostFuture,
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

/// 更新窗「正在检查」进度条一次呼吸的时长。
const CHECKING_PULSE: Duration = Duration::from_secs(2);

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

/// 手动检查更新（偏好设置的「检查更新」）：先以「正在检查」打开或唤起更新窗，结果回来后在窗里展示。
/// 正在下载或安装时只唤起窗口，不打断。
pub fn check_now(cx: &mut App) {
    let Some(status) = updater(cx).map(Updater::status) else {
        log::warn!("the updater is unavailable; the manual update check was skipped");
        return;
    };
    open_update_window(status, cx);
    if let Some(view) = cx
        .try_global::<UpdateWindowHost>()
        .map(|host| host.view.clone())
    {
        view.update(cx, |view, cx| view.check(cx));
    }
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

/// 自测更新流程展示真实检查结果；下载与安装仍由调用方驱动，确保 UI 和 updater 共用同一状态。
#[cfg(feature = "e2e-overrides")]
pub fn selftest_update_window_status(cx: &mut App, status: UpdateStatus) {
    serve_ui(UiRequest::Update(Box::new(status)), cx);
}

/// 开发自测：展示公告按钮顺序，不打开真实链接。
pub fn selftest_announcement(cx: &mut App) {
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
    // 原生模态框会泵消息，启动回调里调用会重入 GPUI 的 AppCell 并使进程崩溃。
    // 交给后台执行器，让对话框的消息循环不占着 App 的借用。
    cx.background_executor()
        .spawn(async move {
            log::info!(
                "announcement self-test result: {:?}",
                show_announcement(prompt)
            );
        })
        .detach();
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
        window_bounds: Some(WindowBounds::centered(size(px(520.), px(230.)), cx)),
        titlebar: Some(TitlebarOptions {
            title: Some(t("common:update.title")),
            ..Default::default()
        }),
        focus: false,
        ..Default::default()
    };
    match crate::platform::open_window(options, cx, |window, cx| {
        crate::platform::reveal_after_first_frame(window, cx, |window, _| {
            raise_update_window_in_place(window);
        });
        cx.new(|_| UpdateWindow::new(updater.clone(), status, requests))
    }) {
        Ok((handle, view)) => {
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
            raise_update_window_in_place(window);
        }) {
            log::debug!("the update window could not be raised: {err:#}");
        }
    }
    #[cfg(target_os = "macos")]
    let _ = (handle, cx);
}

#[cfg(target_os = "windows")]
fn raise_update_window_in_place(window: &Window) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    if !kwikpaste_os::win::keyboard::swallow_marked_alt(std::time::Duration::from_millis(50)) {
        log::debug!("the keyboard hook did not confirm the marked Alt for the update window");
        return;
    }
    if !kwikpaste_os::win::set_foreground(handle.hwnd.get()) {
        log::debug!("SetForegroundWindow did not accept the update window");
    }
}

#[cfg(target_os = "macos")]
fn raise_update_window_in_place(_: &Window) {}

struct UpdateWindow {
    updater: Updater,
    requests: Sender<UiRequest>,
    status: UpdateStatus,
    progress: Option<DownloadProgress>,
    downloaded: Option<UpdateMetadata>,
    checking: bool,
    downloading: bool,
    installing: bool,
    error: Option<String>,
    /// `error` 来自检查（而不是下载或安装）：标题换成「检查更新失败」，并提供「重新检查」。
    check_failed: bool,
    /// 已经设到原生标题栏的标题；只在变化时再设，免得每帧都发 `SetWindowTextW`。
    window_title: String,
}

impl UpdateWindow {
    fn new(updater: Updater, status: UpdateStatus, requests: Sender<UiRequest>) -> Self {
        Self {
            updater,
            requests,
            status,
            progress: None,
            downloaded: None,
            checking: false,
            downloading: false,
            installing: false,
            error: None,
            check_failed: false,
            window_title: String::new(),
        }
    }

    fn set_status(&mut self, status: UpdateStatus, cx: &mut Context<Self>) {
        // 下载中途又收到同一版本的检查结果时保留进度，避免按钮回到“安装更新”被重复点下载。
        let same_version = self.status.update.as_ref().map(|update| &update.version)
            == status.update.as_ref().map(|update| &update.version);
        self.status = status;
        self.error = None;
        self.check_failed = false;
        if !same_version {
            self.progress = None;
            self.downloaded = None;
            self.downloading = false;
            self.installing = false;
        }
        cx.notify();
    }

    /// 手动检查：窗里显示「正在检查」，结果回来后换成新状态或检查失败。
    fn check(&mut self, cx: &mut Context<Self>) {
        if self.checking || self.downloading || self.installing {
            return;
        }
        let updater = self.updater.clone();
        self.checking = true;
        self.error = None;
        self.check_failed = false;
        cx.notify();
        cx.spawn(async move |view, cx| {
            let result = updater.check(CheckMode::Manual).await;
            if let Err(err) = view.update(cx, |view, cx| {
                view.checking = false;
                match result {
                    Ok(status) => view.set_status(status, cx),
                    Err(err) => {
                        log::warn!("manual update check failed: {err:#}");
                        view.error = Some(err.to_string());
                        view.check_failed = true;
                        cx.notify();
                    }
                }
            }) {
                log::debug!("the update window closed before the check completed: {err:#}");
            }
        })
        .detach();
    }

    fn download(&mut self, cx: &mut Context<Self>) {
        if self.checking || self.downloading || self.installing {
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

    fn open_release_notes(&self) {
        let Some(update) = self.status.update.as_ref() else {
            return;
        };
        if let Err(err) = kwikpaste_os::dialogs::open_url(&update.release_notes_url) {
            log::warn!("update release notes could not be opened: {err}");
        }
    }
}

impl Render for UpdateWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::semantic(cx);
        // 检查期间不展示上一次的结果，只留「取消」。
        let update = self.status.update.as_ref().filter(|_| !self.checking);
        let downloaded =
            self.downloaded.is_some() || update.is_some_and(|update| update.downloaded);
        let title = if self.checking {
            t("common:update.checking")
        } else if self.check_failed {
            t("common:update.checkErrorTitle")
        } else if self.error.is_some() {
            t("common:update.errorTitle")
        } else if self.downloading || self.installing {
            t("common:update.updatingTitle")
        } else if downloaded {
            t("common:update.downloadedTitle")
        } else if update.is_some() {
            t("common:update.available")
        } else {
            t("common:update.latestTitle")
        };
        let downloading_version = update.map_or_else(String::new, |update| update.version.clone());
        let description = if self.checking {
            t("common:update.checkingBody")
        } else if self.check_failed {
            // 检查失败的原因（哪个镜像、什么状态码）只对排查有用，已写进日志。
            t("common:update.checkError")
        } else if let Some(error) = self.error.as_deref() {
            t_args("common:update.error", &[("message", error)])
        } else if self.downloading {
            t_args(
                "common:update.downloading",
                &[("version", &downloading_version)],
            )
        } else if self.installing || downloaded {
            t("common:update.downloaded")
        } else if let Some(update) = update {
            t_args(
                "common:update.availableBody",
                &[
                    ("version", &update.version),
                    ("currentVersion", &self.status.current_version),
                ],
            )
        } else {
            t_args(
                "common:update.latest",
                &[("currentVersion", &self.status.current_version)],
            )
        };
        let progress = self
            .progress
            .and_then(|progress| progress.progress)
            .filter(|_| !self.checking);
        let progress_text = progress.map(|value| format!("{:.0}", value * 100.));
        let show_progress = self.checking || self.downloading || self.progress.is_some();
        let progress_value = if self.checking {
            1.
        } else {
            progress.unwrap_or(0.12).clamp(0., 1.) as f32
        };
        let progress_fill = div()
            .h_full()
            .w(relative(progress_value))
            .rounded_full()
            .bg(tokens.accent.solid);
        // 检查没有进度可报：整条进度条呼吸闪烁，表示还在等服务器。
        let progress_fill = if self.checking && !cx.reduce_motion() {
            progress_fill
                .with_animation(
                    "update-checking",
                    Animation::new(CHECKING_PULSE)
                        .repeat()
                        .with_easing(pulsating_between(0.35, 1.)),
                    |fill, delta| fill.opacity(delta),
                )
                .into_any_element()
        } else {
            progress_fill.into_any_element()
        };
        let show_release_notes = update.is_some() && self.error.is_none();
        let window_title = if self.downloading || self.installing {
            t("common:update.updatingTitle")
        } else {
            t("common:update.title")
        };
        if self.window_title != window_title.as_ref() {
            window.set_window_title(&window_title);
            self.window_title = window_title.to_string();
        }
        div()
            .size_full()
            .flex()
            .gap(px(20.))
            .p(px(20.))
            .bg(crate::platform::material::shell_surface(
                cx,
                tokens.surface.panel,
            ))
            .text_color(tokens.text.primary)
            .child(
                div().flex_none().w(px(56.)).child(
                    img(ImageSource::Image(crate::clipboard::view::app_logo())).size(px(56.)),
                ),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().kp_text(TextSize::Lg).child(title))
                    .child(
                        div()
                            .kp_text(TextSize::Sm)
                            .text_color(tokens.text.secondary)
                            .child(description),
                    )
                    .when(show_release_notes, |element| {
                        // 纯文字链接，左边缘与说明文字对齐（链接按钮自带左右内边距，会缩进一截）。
                        let label = t("common:update.releaseNotes");
                        element.child(
                            div().flex().child(
                                div()
                                    .id("update-release-notes")
                                    .role(gpui::Role::Link)
                                    .aria_label(label.clone())
                                    .kp_text(TextSize::Sm)
                                    .text_color(tokens.accent.solid)
                                    .cursor_pointer()
                                    .hover(|style| {
                                        style.text_color(tokens.accent.hover).underline()
                                    })
                                    .child(label)
                                    .on_click(
                                        cx.listener(|view, _, _, _| view.open_release_notes()),
                                    ),
                            ),
                        )
                    })
                    .when(show_progress, |element| {
                        element.child(
                            div()
                                .mt(px(6.))
                                .h(px(6.))
                                .w_full()
                                .overflow_hidden()
                                .rounded_full()
                                .bg(tokens.fill.default)
                                .child(progress_fill),
                        )
                    })
                    .when_some(progress_text, |element, progress| {
                        element.child(
                            div()
                                .kp_text(TextSize::Sm)
                                .text_color(tokens.text.secondary)
                                .child(t_args(
                                    "common:update.progress",
                                    &[("progress", &progress)],
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .absolute()
                    .left(px(20.))
                    .right(px(20.))
                    .bottom(px(16.))
                    .flex()
                    .justify_between()
                    .gap(px(8.))
                    .child(div().flex().gap(px(8.)).when(
                        update.is_some_and(|_| !downloaded && !self.downloading),
                        |element| {
                            element.child(
                                Button::new("update-skip", t("common:update.skip"))
                                    .on_click(cx.listener(|view, _, _, cx| view.skip(cx))),
                            )
                        },
                    ))
                    .child(div().flex_1())
                    .when(
                        update.is_some()
                            && !self.downloading
                            && !self.installing
                            && !downloaded
                            && self.error.is_none(),
                        |element| {
                            element.child(
                                Button::new("update-later", t("common:update.later"))
                                    .on_click(|_, window, _| window.remove_window()),
                            )
                        },
                    )
                    .when(self.checking || self.downloading, |element| {
                        element.child(
                            Button::new("update-cancel", t("common:update.cancel"))
                                .on_click(|_, window, _| window.remove_window()),
                        )
                    })
                    .when(self.check_failed, |element| {
                        element.child(
                            Button::new("update-check-again", t("common:update.checkAgain"))
                                .on_click(cx.listener(|view, _, _, cx| view.check(cx))),
                        )
                    })
                    .when(
                        (update.is_none() && !self.checking) || self.error.is_some(),
                        |element| {
                            element.child(
                                Button::new("update-ok", t("common:update.ok"))
                                    .primary()
                                    .on_click(|_, window, _| window.remove_window()),
                            )
                        },
                    )
                    .when(
                        update.is_some()
                            && !self.downloading
                            && !self.installing
                            && self.error.is_none()
                            && downloaded,
                        |element| {
                            element.child(
                                Button::new("update-install", t("common:update.install"))
                                    .primary()
                                    .on_click(cx.listener(|view, _, _, cx| view.install(cx))),
                            )
                        },
                    )
                    .when(
                        update.is_some()
                            && !self.downloading
                            && !self.installing
                            && !downloaded
                            && self.error.is_none(),
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
