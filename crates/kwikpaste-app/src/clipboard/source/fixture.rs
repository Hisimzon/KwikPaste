//! 夹具数据源：内存里的一份有序列表，按 core 的分页语义切页。
//!
//! 数据来自 JSON（`ClipboardItemPage` 形状，即 1.4.0 / core `list_items` 的输出）或合成生成器。
//! 可选的延迟模拟 core 查询耗时：在独立线程上睡眠后返回，不阻塞 UI。

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Context as _;
use futures::{FutureExt as _, future::BoxFuture};
use serde::Deserialize;

use super::{ClipboardSource, ListQuery, synthetic::AssetSet};
use crate::clipboard::model::{item::ListItem, list_model::Page};

/// JSON 夹具里资源路径的占位前缀，加载时换成合成资源目录。
pub const ASSETS_PLACEHOLDER: &str = "{assets}";

#[derive(Deserialize)]
struct PageJson {
    list: Vec<ListItem>,
}

/// 夹具的“数据库”：一份按列表顺序排好的记录（置顶在前）。
#[derive(Debug, Default)]
pub struct FixtureStore {
    items: Vec<Arc<ListItem>>,
}

impl FixtureStore {
    pub fn new(items: Vec<Arc<ListItem>>) -> Self {
        Self { items }
    }

    /// 解析 `ClipboardItemPage` 形状的 JSON；字符串里的 `{assets}` 换成 `assets_dir`。
    pub fn from_page_json(json: &str, assets_dir: &Path) -> anyhow::Result<Self> {
        let dir = assets_dir.to_string_lossy().replace('\\', "/");
        let json = json.replace(ASSETS_PLACEHOLDER, &dir);
        let page: PageJson = serde_json::from_str(&json).context("fixture is not a list page")?;

        Ok(Self::new(page.list.into_iter().map(Arc::new).collect()))
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn page(&self, query: ListQuery) -> Page {
        Page {
            items: self
                .items
                .iter()
                .skip(query.offset)
                .take(query.limit)
                .cloned()
                .collect(),
            total: self.items.len(),
        }
    }

    #[cfg(test)]
    pub fn get(&self, index: usize) -> Option<&Arc<ListItem>> {
        self.items.get(index)
    }

    pub fn index_of(&self, id: &str) -> Option<usize> {
        self.items.iter().position(|item| &*item.id == id)
    }

    /// 新记录进库：排在置顶块之后的第一位（core 按 `is_pinned DESC, updated_at DESC` 排序）。
    pub fn insert_newest(&mut self, item: ListItem) {
        let at = self.items.iter().take_while(|item| item.is_pinned).count();
        self.items.insert(at, Arc::new(item));
    }

    pub fn remove(&mut self, id: &str) -> Option<Arc<ListItem>> {
        let index = self.index_of(id)?;
        Some(self.items.remove(index))
    }

    #[allow(dead_code, reason = "置顶命令（U2）接线前只有单测在用")]
    /// 切换置顶：置顶的移到置顶块末尾，取消置顶的放回置顶块之后。
    pub fn set_pinned(&mut self, id: &str, pinned: bool) -> bool {
        let Some(item) = self.remove(id) else {
            return false;
        };
        let mut item = (*item).clone();
        item.is_pinned = pinned;
        let at = self.items.iter().take_while(|item| item.is_pinned).count();
        self.items.insert(at, Arc::new(item));
        true
    }

    pub fn patch(&mut self, id: &str, patch: impl FnOnce(&mut ListItem)) -> bool {
        let Some(item) = self.items.iter_mut().find(|item| &*item.id == id) else {
            return false;
        };
        patch(Arc::make_mut(item));
        true
    }
}

/// 以 [`FixtureStore`] 为后端的数据源。
pub struct FixtureSource {
    store: Arc<Mutex<FixtureStore>>,
    thumbnails: PathBuf,
    latency: Duration,
}

impl FixtureSource {
    pub fn new(store: FixtureStore, assets: &AssetSet) -> Self {
        Self {
            store: Arc::new(Mutex::new(store)),
            thumbnails: assets.thumbnails_dir(),
            latency: Duration::ZERO,
        }
    }

    /// 每次查询前等待 `latency`，模拟 core 查库与加工的耗时。
    pub fn with_latency(mut self, latency: Duration) -> Self {
        self.latency = latency;
        self
    }

    /// 后端数据的句柄，自测和跑分用它模拟新记录、删除、置顶等变化。
    pub fn store(&self) -> Arc<Mutex<FixtureStore>> {
        self.store.clone()
    }

    /// 在后台线程上等 `latency` 再算结果；没有延迟时就地算好。
    fn respond<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
    ) -> BoxFuture<'static, anyhow::Result<T>> {
        if self.latency.is_zero() {
            return futures::future::ready(work()).boxed();
        }

        let latency = self.latency;
        let (sender, receiver) = futures::channel::oneshot::channel();
        std::thread::spawn(move || {
            std::thread::sleep(latency);
            let _ = sender.send(work());
        });

        async move { receiver.await.context("fixture worker stopped")? }.boxed()
    }
}

impl ClipboardSource for FixtureSource {
    fn list(&self, query: ListQuery) -> BoxFuture<'static, anyhow::Result<Page>> {
        let store = self.store.clone();

        self.respond(move || {
            let store = store
                .lock()
                .map_err(|_| anyhow::anyhow!("fixture store is poisoned"))?;
            Ok(store.page(query))
        })
    }

    /// 合成缩略图都已在资源目录里；文件名以 `missing-` 开头的模拟生成失败。
    fn thumbnail(&self, file_name: Arc<str>) -> BoxFuture<'static, anyhow::Result<PathBuf>> {
        let path = self.thumbnails.join(&*file_name);

        self.respond(move || {
            if file_name.starts_with("missing-") || !path.exists() {
                anyhow::bail!("no thumbnail for {file_name}");
            }
            Ok(path)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("../../../fixtures/list-sample.json");

    #[test]
    fn the_sample_fixture_parses_and_resolves_assets() {
        let store = FixtureStore::from_page_json(SAMPLE, Path::new("C:\\tmp\\fixtures"))
            .expect("sample fixture parses");
        assert!(store.len() >= 12);

        let page = store.page(ListQuery {
            offset: 0,
            limit: 100,
        });
        assert_eq!(page.total, store.len());
        let icon = page
            .items
            .iter()
            .find_map(|item| item.source_app_icon_path.clone())
            .expect("some item has an app icon");
        assert!(icon.starts_with("C:/tmp/fixtures/"), "{icon}");
        assert!(
            !page
                .items
                .iter()
                .any(|item| item.id.contains(ASSETS_PLACEHOLDER))
        );
    }

    /// 夹具的每一条都能按 core 的数据库行反序列化：必填字段齐全、类型一致。
    #[test]
    fn the_sample_fixture_has_the_core_row_shape() {
        let value: serde_json::Value = serde_json::from_str(SAMPLE).expect("json");
        let list = value
            .get("list")
            .and_then(serde_json::Value::as_array)
            .expect("list array");
        for entry in list {
            let row: Result<kwikpaste_core::db::models::ClipboardItem, _> =
                serde_json::from_value(entry.clone());
            assert!(row.is_ok(), "{entry}: {row:?}");
        }
    }

    /// core 展示层的 golden 输出（1.4.0 的列表 JSON）原样能读成 [`ListItem`]。
    #[test]
    fn core_golden_list_payloads_parse() {
        for json in [
            include_str!("../../../../kwikpaste-core/tests/fixtures/presenter/list-default.json"),
            include_str!(
                "../../../../kwikpaste-core/tests/fixtures/presenter/list-unredacted.json"
            ),
        ] {
            let store =
                FixtureStore::from_page_json(json, Path::new("/tmp")).expect("golden parses");
            assert!(store.len() > 10);
        }
    }

    #[test]
    fn store_mutations_keep_the_pinned_block_first() {
        let store = FixtureStore::from_page_json(SAMPLE, Path::new("/tmp")).expect("parses");
        let mut store = store;
        let pinned = store.items.iter().take_while(|item| item.is_pinned).count();
        let first_regular = store.get(pinned).map(|item| item.id.clone()).expect("rows");

        assert!(store.set_pinned(&first_regular, true));
        assert_eq!(
            store.get(pinned).map(|item| item.id.clone()),
            Some(first_regular.clone())
        );
        assert!(store.get(pinned).is_some_and(|item| item.is_pinned));

        assert!(store.set_pinned(&first_regular, false));
        assert_eq!(
            store.get(pinned).map(|item| item.id.clone()),
            Some(first_regular.clone())
        );

        let removed = store.remove(&first_regular);
        assert!(removed.is_some());
        assert_eq!(store.index_of(&first_regular), None);
    }

    #[test]
    fn latency_runs_off_the_calling_thread() {
        let store = FixtureStore::from_page_json(SAMPLE, Path::new("/tmp")).expect("parses");
        let assets = AssetSet {
            root: PathBuf::from("/tmp"),
            images: Vec::new(),
            originals: Vec::new(),
            app_icons: Vec::new(),
            file_icons: Vec::new(),
        };
        let source = FixtureSource::new(store, &assets).with_latency(Duration::from_millis(5));

        let page = futures::executor::block_on(source.list(ListQuery {
            offset: 2,
            limit: 3,
        }))
        .expect("page");
        assert_eq!(page.items.len(), 3);

        let missing = futures::executor::block_on(source.thumbnail(Arc::from("missing-1.png")));
        assert!(missing.is_err());
    }
}
