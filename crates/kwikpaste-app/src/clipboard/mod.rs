//! 主窗口（UI 里程碑 U1、U2）：剪贴板列表、头部与搜索、分组栏、快捷动作、多选。
//!
//! - [`model`]：不依赖 GPUI 的分页缓存、控制器、筛选、空态、快捷动作、多选、排布与时间标签；
//! - [`source`]：数据接口与适配器（core、合成夹具）；
//! - [`view`]：主窗口、`list(ListState)` 视图、卡片、`KpImageCache`、跑分。
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

use gpui::{App, AppContext as _, Entity, TaskExt as _, Window};

use self::{
    source::{
        ClipboardSource, FixtureSource, FixtureStore, Group,
        core_source::CoreSource,
        synthetic::{self, AssetSet},
    },
    view::{ClipboardPanel, ListIntent},
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
    let fixtures = selftest::list_selftest() || selftest::enabled(selftest::CORE_LIST);
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

    let store = FixtureStore::from_page_json(SAMPLE, &assets.root)
        .map(|store| store.with_groups(demo_groups()))
        .unwrap_or_else(|err| {
            log::error!("the sample fixture is invalid: {err:#}");
            FixtureStore::default()
        });
    PreparedSource {
        source: Some(Arc::new(FixtureSource::new(store, &assets))),
        bench: None,
    }
}

/// 示例夹具的自定义分组（截图与交互自测用）：两个预设图标的分组，各放几条记录。
fn demo_groups() -> Vec<(Group, Vec<usize>)> {
    let group = |id: &str, name: &str, icon: &str| Group {
        id: id.into(),
        name: name.into(),
        icon: icon.into(),
        is_hidden: false,
    };

    vec![
        (
            group("demo-work", "工作", "i-lets-icons:book"),
            vec![0, 2, 5],
        ),
        (group("demo-code", "代码", "i-lets-icons:code"), vec![3]),
        (
            group("demo-hidden", "已隐藏", "i-lets-icons:box"),
            Vec::new(),
        ),
    ]
    .into_iter()
    .map(|(mut group, members)| {
        group.is_hidden = &*group.id == "demo-hidden";
        (group, members)
    })
    .collect()
}

/// 建面板里的主窗口（传给 `platform::start`，这时 core 和面板事件都已就绪）。
pub fn build_panel(
    prepared: &PreparedSource,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ClipboardPanel> {
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

    let panel = cx.new(|cx| ClipboardPanel::new(source, window, cx));
    // 只有数据来自平台层的 core 时才跟随它的事件、接上粘贴链路；夹具和自测 core 不受本机开发数据
    // 影响，夹具里的记录也粘贴不了。
    if prepared.source.is_none() {
        if let Some(events) = platform::core_events(cx) {
            panel.update(cx, |panel, cx| panel.follow_core_events(&events, cx));
        }
        let list = panel.read(cx).list().clone();
        cx.subscribe(&list, |_, intent: &ListIntent, cx| {
            if let ListIntent::Paste { id, plain } = intent {
                platform::paste::paste(cx, id.to_string(), *plain, false).detach_and_log_err(cx);
            }
        })
        .detach();
    }

    panel
}

/// 面板建好之后：跑分模式下开始跑分，交互自测开始跑脚本，演示模式按环境变量摆出要截图的状态。
pub fn attach(panel: &Entity<ClipboardPanel>, prepared: PreparedSource, cx: &mut App) {
    if let Some((store, assets, rows)) = prepared.bench {
        let list = panel.read(cx).list().clone();
        let window = list.read(cx).window_handle();
        view::bench::start(&list, store, assets, rows, window, cx);
    }
    if selftest::enabled(selftest::PANEL_UI) {
        view::selftest::run(panel.clone(), cx);
    } else if selftest::enabled(selftest::LIST_DEMO) {
        view::selftest::stage_demo(panel.clone(), cx);
    }
}
