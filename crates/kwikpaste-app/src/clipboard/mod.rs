//! 主窗口的剪贴板列表（UI 里程碑 U1）。
//!
//! - [`model`]：不依赖 GPUI 的分页缓存、控制器、排布与时间标签；
//! - [`source`]：数据接口与适配器（core、合成夹具）；
//! - [`view`]：`list(ListState)` 视图、卡片、`KpImageCache`、跑分。
//!
//! 数据来源由 [`prepare_source`] 按启动参数选择：普通启动用平台层启动的 core（经
//! [`source::core_source::CoreSource`]），并跟随 core 的记录事件刷新。合成夹具只在自测里用：
//! `--selftest-list-bench` 用 1 万行合成数据跑分，`--selftest-list-demo` 用内置示例夹具截图；
//! `--selftest-core-list` 在临时目录里另起一个 core，灌入合成记录后显示。

pub mod model;
pub mod source;
pub mod view;

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use gpui::{App, AppContext as _, Entity, Window};

use self::{
    source::{
        ClipboardSource, FixtureSource, FixtureStore,
        core_source::CoreSource,
        synthetic::{self, AssetSet},
    },
    view::ClipboardList,
};
use crate::{core_host, platform, selftest};

pub use view::init;

/// 内置的示例夹具（`ClipboardItemPage` 形状的合成数据）。
const SAMPLE: &str = include_str!("../../fixtures/list-sample.json");
/// 跑分时夹具查询的模拟延迟（与 core 查 30 行加展示层加工的量级相当）。
const BENCH_LATENCY: Duration = Duration::from_millis(8);

/// 选好的数据源。`source` 为空表示用平台层的 core；跑分时还带着夹具后端，用来模拟新记录、删除等变化。
pub struct PreparedSource {
    source: Option<Arc<dyn ClipboardSource>>,
    bench: Option<(Arc<Mutex<FixtureStore>>, AssetSet, usize)>,
}

/// 按启动参数选数据源。
pub fn prepare_source() -> PreparedSource {
    let core = PreparedSource {
        source: None,
        bench: None,
    };
    let fixtures = selftest::enabled(selftest::LIST_BENCH)
        || selftest::enabled(selftest::LIST_DEMO)
        || selftest::enabled(selftest::CORE_LIST);
    if !fixtures {
        return core;
    }

    let assets_dir = synthetic::default_assets_dir();
    let assets = match synthetic::ensure_assets(&assets_dir) {
        Ok(assets) => assets,
        Err(err) => {
            log::error!("synthetic fixture assets are unavailable: {err:#}");
            AssetSet {
                root: assets_dir,
                images: Vec::new(),
                originals: Vec::new(),
                app_icons: Vec::new(),
                file_icons: Vec::new(),
            }
        }
    };

    if selftest::enabled(selftest::LIST_BENCH) {
        let (store, rows) = view::bench::fixture(&assets);
        let source = FixtureSource::new(store, &assets).with_latency(BENCH_LATENCY);
        let store = source.store();
        return PreparedSource {
            source: Some(Arc::new(source)),
            bench: Some((store, assets, rows)),
        };
    }

    if selftest::enabled(selftest::CORE_LIST) {
        match source::start_selftest_core(&assets) {
            Ok(source) => {
                return PreparedSource {
                    source: Some(Arc::new(source)),
                    bench: None,
                };
            }
            Err(err) => log::error!("the selftest core could not start: {err:#}"),
        }
    }

    let store = FixtureStore::from_page_json(SAMPLE, &assets.root).unwrap_or_else(|err| {
        log::error!("the sample fixture is invalid: {err:#}");
        FixtureStore::default()
    });
    PreparedSource {
        source: Some(Arc::new(FixtureSource::new(store, &assets))),
        bench: None,
    }
}

/// 建面板里的列表视图（传给 `platform::start`，这时 core 和面板事件都已就绪）。
pub fn build_panel(
    prepared: &PreparedSource,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ClipboardList> {
    let host_core = core_host::core(cx).cloned();
    let source: Arc<dyn ClipboardSource> = match (&prepared.source, &host_core) {
        (Some(source), _) => source.clone(),
        (None, Some(core)) => Arc::new(CoreSource::new(core.clone())),
        (None, None) => {
            log::error!("no core is running; the clipboard list stays empty");
            Arc::new(FixtureSource::new(
                FixtureStore::default(),
                &AssetSet {
                    root: synthetic::default_assets_dir(),
                    images: Vec::new(),
                    originals: Vec::new(),
                    app_icons: Vec::new(),
                    file_icons: Vec::new(),
                },
            ))
        }
    };

    let list = cx.new(|cx| ClipboardList::new(source, window, cx));
    // 只有数据来自平台层的 core 时才跟随它的记录事件；夹具和自测 core 不受本机开发数据影响。
    if prepared.source.is_none()
        && let Some(events) = platform::core_events(cx)
    {
        list.update(cx, |list, cx| list.follow_core_events(&events, cx));
    }

    list
}

/// 面板建好之后：跑分模式下开始跑分。
pub fn attach(list: &Entity<ClipboardList>, prepared: PreparedSource, cx: &mut App) {
    if let Some((store, assets, rows)) = prepared.bench {
        let window = list.read(cx).window_handle();
        view::bench::start(list, store, assets, rows, window, cx);
    }
}
