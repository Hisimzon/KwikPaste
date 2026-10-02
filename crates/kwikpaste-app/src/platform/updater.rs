//! 更新器的宿主：建 [`Updater`]、开始后台调度，实现安装交接要的 [`HandoffHost`]。
//!
//! 交接的每一步由更新器在 core 的 runtime 上按顺序 await；这里把请求转进主线程，各占一个事件循环
//! turn，做完再回报。`UpdaterUi`（更新窗、公告对话框）归 UI 线，先放一个只记日志的占位实现：
//! 不弹窗、不打开浏览器。
//!
//! 开发构建（`target\…` 里的 exe）判定为 `InstallKind::Unmanaged`、`AppEnv::Dev`：不检查、不下载、
//! 不安装，也不上报统计、不拉公告，只记下检查时间。

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use async_channel::Sender;
use futures::channel::oneshot;
use gpui::{App, AsyncApp, Global};
use kwikpaste_updater::{
    AnnouncementOutcome, AnnouncementPrompt, HandoffHost, HostFuture, UpdateStatus, Updater,
    UpdaterUi,
};

use super::panel::{PanelCommand, Trigger, TriggerSource};
use super::{hotkey, instance, tray};
use crate::core_host;

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
    cx.set_global(HandoffRequests(requests.clone()));
    let host = Arc::new(Handoff { requests });

    let created = match Updater::new(core, Arc::new(LoggingUi), host) {
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

/// UI 线做好更新窗和公告对话框之前的占位：只记日志，公告一律当作关掉。
struct LoggingUi;

impl UpdaterUi for LoggingUi {
    fn update_available(&self, status: UpdateStatus) {
        let version = status.update.map(|update| update.version);
        log::info!("an update is available ({version:?}); the update window is not built yet");
    }

    fn show_announcement(&self, prompt: AnnouncementPrompt) -> HostFuture<'_, AnnouncementOutcome> {
        log::info!(
            "announcement {} not shown: the announcement dialog is not built yet",
            prompt.id
        );
        Box::pin(async { AnnouncementOutcome::Closed })
    }

    fn open_url(&self, url: &str) {
        log::info!("not opening {url}: the updater UI is not built yet");
    }
}
