//! 宿主一侧的 kwikpaste-core：建 runtime、启动 core，把 core 的事件送回 GPUI 主线程。
//!
//! 数据目录由 [`crate::identity`] 决定：开发、自测构建用各自的 identifier 和 `AppEnv::Dev`
//! （`%LOCALAPPDATA%\com.fastthree.kwikpaste.native-dev\dev\…`），绝不会落到已安装 1.x 的
//! `com.fastthree.kwikpaste\prod`。只有打开 `production-identity` 的发布构建才用正式的目录。

use std::sync::Arc;

use anyhow::Context as _;
use async_channel::Receiver;
use gpui::{App, Global};
use kwikpaste_core::clipboard::MemoryClipboard;
use kwikpaste_core::{AppInfo, Core, CoreEvent, CoreOptions, CorePaths, CoreRuntime};
use kwikpaste_os::services::NativeServices;

/// 运行中的 core 与它的 runtime。作为 GPUI 全局保存，界面经 [`core`] 取用。
pub struct CoreHost {
    core: Core,
    // runtime 要活得比 core 久：core 的任务都跑在它上面。
    _runtime: CoreRuntime,
}

impl Global for CoreHost {}

/// 启动后交给平台层的 core 和它的事件流。
pub struct StartedCore {
    pub host: CoreHost,
    pub events: Receiver<CoreEvent>,
}

/// 建 runtime 并启动 core（读设置、打开并迁移数据库、启动自动清理），再接上平台能力
/// （前台应用、应用扫描、提示音），普通启动时开始监听系统剪贴板。在创建 GPUI 平台之前调用，
/// 期间阻塞当前线程。
pub fn start() -> anyhow::Result<StartedCore> {
    let identity = crate::identity::current();
    let version = semver::Version::parse(env!("CARGO_PKG_VERSION"))
        .context("the crate version is not semver")?;
    let info = AppInfo {
        name: kwikpaste_core::APP_NAME,
        identifier: identity.identifier,
        version,
        env: identity.env,
    };
    let paths = CorePaths::for_native(identity.identifier, identity.env)?;
    let options = CoreOptions {
        locale: kwikpaste_os::locale::system_locale(),
        ..CoreOptions::default()
    };

    let runtime = CoreRuntime::new()?;
    let (sender, events) = async_channel::unbounded();
    let sink = move |event: CoreEvent| {
        let _ = sender.try_send(event);
    };
    let core = futures::executor::block_on(Core::start(
        info,
        paths,
        options,
        Arc::new(sink),
        runtime.handle(),
    ))
    .context("kwikpaste-core did not start")?;
    core.set_platform_services(Arc::new(NativeServices));
    // 自测进程不读写本机剪贴板，也不监听；只有真机剪贴板探针（`--selftest-real-clipboard`）例外。
    let real_clipboard =
        !crate::selftest::active() || crate::selftest::enabled(crate::selftest::REAL_CLIPBOARD);
    if real_clipboard {
        if let Err(err) = core.start_watcher() {
            log::error!("the clipboard watcher did not start: {err}");
        }
    } else {
        core.set_clipboard_provider(Arc::new(MemoryClipboard::new()));
    }
    log::info!(
        "core started: {} {:?}, data in {}",
        identity.identifier,
        identity.env,
        core.paths().bootstrap_dir().display()
    );

    Ok(StartedCore {
        host: CoreHost {
            core,
            _runtime: runtime,
        },
        events,
    })
}

impl CoreHost {
    pub fn core(&self) -> &Core {
        &self.core
    }
}

/// 当前的 core。平台层启动后一直存在；展示窗之类不经平台层启动的模式下为 `None`。
pub fn core(cx: &App) -> Option<&Core> {
    cx.try_global::<CoreHost>().map(CoreHost::core)
}
