//! core 的组合根：持有设置、数据库、剪贴板各存储与清理调度，给宿主一个统一入口。
//!
//! 公开 async 方法都先 [`hop`] 到 core runtime 上执行，宿主在任何执行器里都可以直接 await，
//! 不需要处在 tokio 上下文里。同步方法只做内存操作或很小的文件读写，可以在任意线程调用；
//! 带 [`ClipboardBackend`] 参数的方法要在创建该后端的线程上调用（系统剪贴板句柄是 `!Send`）。

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use crate::clipboard::{
    self, AppIconStore, CleanupPreview, CleanupReport, CleanupStatus, ClipboardBackend,
    ClipboardPayload, ClipboardReader, FileIconStore, ImageStore, WritebackGuard,
};
use crate::db::items::UpsertResult;
use crate::db::models::{
    ClipboardApp, ClipboardGroup, ClipboardItem, ClipboardItemPage, ClipboardItemQuery,
};
use crate::db::{self, DatabaseState};
use crate::env::{AppInfo, CoreOptions};
use crate::error::Result;
use crate::events::{CoreEvent, EventSink};
use crate::paths::CorePaths;
use crate::runtime::hop;
use crate::settings::{
    History, Language, Settings, SettingsDelta, SettingsLoadReport, SettingsStore,
};

/// core 的句柄，克隆很便宜，各处共用同一份状态。
#[derive(Clone)]
pub struct Core(Arc<CoreInner>);

pub(crate) struct CoreInner {
    pub(crate) info: AppInfo,
    pub(crate) paths: CorePaths,
    pub(crate) rt: Handle,
    pub(crate) events: Arc<dyn EventSink>,
    pub(crate) settings: SettingsStore,
    pub(crate) db: DatabaseState,
    pub(crate) guard: WritebackGuard,
    pub(crate) images: ImageStore,
    pub(crate) app_icons: AppIconStore,
    pub(crate) file_icons: FileIconStore,
    pub(crate) cleanup: clipboard::cleanup::CleanupScheduler,
    /// 去重入库串行执行，见 [`clipboard::persist::store_and_emit`]。
    pub(crate) upsert_lock: tokio::sync::Mutex<()>,
    cleanup_task: Mutex<Option<JoinHandle<()>>>,
}

impl Core {
    /// 启动 core：读设置 → 打开数据库并迁移 → 建各存储 → 启动自动清理（首轮立即执行）。
    ///
    /// `rt` 通常来自 [`crate::CoreRuntime::handle`]，必须启用了定时器（`enable_all`）。
    pub async fn start(
        info: AppInfo,
        paths: CorePaths,
        options: CoreOptions,
        events: Arc<dyn EventSink>,
        rt: Handle,
    ) -> Result<Core> {
        let handle = rt.clone();
        hop(&rt, async move {
            let settings = SettingsStore::new(&paths, options.locale.clone())?;
            let pool = db::init(&paths, options.db_max_connections).await?;
            let images = ImageStore::new(&paths)?;
            let app_icons = AppIconStore::new(&paths)?;
            let file_icons = FileIconStore::new(&paths)?;

            let inner = Arc::new(CoreInner {
                info,
                paths,
                rt: handle,
                events,
                settings,
                db: DatabaseState::new(pool),
                guard: WritebackGuard::new(),
                images,
                app_icons,
                file_icons,
                cleanup: Default::default(),
                upsert_lock: tokio::sync::Mutex::new(()),
                cleanup_task: Mutex::new(None),
            });

            if inner.settings.cleanup_paused() {
                log::warn!(
                    "history settings fell back on load; automatic cleanup is paused until they are saved"
                );
            }
            let task = clipboard::cleanup::spawn(&inner);
            *inner.cleanup_task() = Some(task);

            Ok(Core(inner))
        })
        .await
    }

    /// 停止后台清理并关闭连接池（SQLite 借此做 WAL checkpoint）。之后不要再调用其它 async 方法。
    pub async fn shutdown(&self) -> Result<()> {
        let core = self.clone();
        self.hop(async move {
            if let Some(task) = core.0.cleanup_task().take() {
                task.abort();
            }
            core.0.db.pool().await.close().await;
            Ok(())
        })
        .await
    }

    pub fn info(&self) -> &AppInfo {
        &self.0.info
    }

    pub fn paths(&self) -> &CorePaths {
        &self.0.paths
    }

    /// core runtime 的句柄，宿主需要在上面跑自己的 tokio 任务时使用。
    pub fn runtime(&self) -> &Handle {
        &self.0.rt
    }

    // ---- 设置 ----

    pub fn settings(&self) -> Settings {
        self.0.settings.snapshot()
    }

    /// 当前界面语言，Rust 侧文案（[`crate::i18n`]）据此取词。
    pub fn language(&self) -> Language {
        crate::i18n::current_language(&self.0.settings)
    }

    /// 启动时读设置文件哪些字段没有采用原值。
    pub fn settings_load_report(&self) -> SettingsLoadReport {
        self.0.settings.load_report()
    }

    /// 用 JSON patch（camelCase 键，与 `settings.json` 相同）深度合并到当前设置并落盘。
    /// 成功后发 [`CoreEvent::SettingsUpdated`]；改到 `clipboard.history` 时同时请求一轮清理。
    pub async fn update_settings(&self, patch: serde_json::Value) -> Result<Settings> {
        let core = self.clone();
        self.hop(async move {
            let delta = SettingsDelta::from_patch(&patch);
            let next = core.0.settings.update(patch)?;
            if delta.touches("clipboard.history") {
                clipboard::cleanup::request(&core.0);
            }
            core.emit_settings(&next, delta);
            Ok(next)
        })
        .await
    }

    /// 恢复默认设置（保留历史记录与资源文件）。
    pub async fn reset_settings(&self) -> Result<Settings> {
        let core = self.clone();
        self.hop(async move {
            let next = core.0.settings.reset()?;
            clipboard::cleanup::request(&core.0);
            core.emit_settings(&next, SettingsDelta::replaced());
            Ok(next)
        })
        .await
    }

    // ---- 剪贴板 ----

    /// 回环抑制：监听读到内容后用它判断是不是自己刚写回的。
    pub fn writeback_guard(&self) -> &WritebackGuard {
        &self.0.guard
    }

    pub fn image_store(&self) -> &ImageStore {
        &self.0.images
    }

    pub fn app_icon_store(&self) -> &AppIconStore {
        &self.0.app_icons
    }

    pub fn file_icon_store(&self) -> &FileIconStore {
        &self.0.file_icons
    }

    /// 按当前的采集设置读取剪贴板，返回第一个启用且有内容的表示。
    pub fn read_payload(&self, backend: &dyn ClipboardBackend) -> Result<Option<ClipboardPayload>> {
        let capture = self.0.settings.snapshot().clipboard.capture;
        ClipboardReader::with_backend(backend).read_with_capture(&capture)
    }

    /// 按当前设置把载荷转成待入库记录：类型过滤、大小上限、敏感内容、子类型识别，图片原图落盘。
    /// 返回 `None` 表示按设置不收录。
    pub fn build_item(&self, payload: &ClipboardPayload) -> Result<Option<ClipboardItem>> {
        let settings = self.0.settings.snapshot();
        clipboard::build_item_with_settings(
            &self.0.images,
            payload,
            &settings.clipboard.capture,
            &settings.clipboard.sensitive,
            settings.clipboard.content.copy_plain,
        )
    }

    /// 去重入库：`source_app` 先登记到应用表；成功后发 [`CoreEvent::ClipboardUpserted`]，
    /// 新记录还会触发一次条数与存储上限检查。
    pub async fn store_item(
        &self,
        item: ClipboardItem,
        source_app: Option<ClipboardApp>,
    ) -> Result<UpsertResult> {
        let core = self.clone();
        self.hop(async move {
            clipboard::persist::persist_and_notify(&core.0, &item, source_app.as_ref()).await
        })
        .await
    }

    /// 把记录写回剪贴板并登记回环抑制；`plain = true` 只写纯文本（文件记录写成路径文本）。
    pub fn write_to_clipboard(
        &self,
        backend: &dyn ClipboardBackend,
        item: &ClipboardItem,
        plain: bool,
    ) -> Result<()> {
        clipboard::write_to_clipboard(backend, &self.0.images, &self.0.guard, item, plain)
    }

    /// 把一段纯文本（快捷信息、拆词选区）写回剪贴板并登记回环抑制。
    pub fn write_text_fragment(&self, backend: &dyn ClipboardBackend, text: &str) -> Result<()> {
        clipboard::write_text_fragment(backend, &self.0.guard, text)
    }

    /// 图片记录（`content` 即文件名）的原图路径；文件名不合法时报错。
    pub fn image_origin_path(&self, file_name: &str) -> Result<PathBuf> {
        clipboard::validate_image_file_name(file_name)?;
        Ok(self.0.images.origin_path(file_name))
    }

    /// 确保缩略图存在并返回路径；首次生成在阻塞线程池里解码，并发张数有上限。
    pub async fn ensure_thumbnail(&self, file_name: &str) -> Result<PathBuf> {
        clipboard::validate_image_file_name(file_name)?;
        let core = self.clone();
        let file_name = file_name.to_owned();
        self.hop(async move { core.0.images.ensure_thumbnail_async(&file_name).await })
            .await
    }

    // ---- 记录查询 ----

    /// 一页列表数据库原始行（文本记录不带 `content`，带来源应用名与图标文件名）。
    ///
    /// 还没有经过展示层：敏感内容未脱敏，缩略图路径、文件条目、可用动作等附加字段为空。
    /// 正式界面要等展示层（presenter）接入，这里只供开发期接线与测试。
    pub async fn query_items_raw(&self, query: ClipboardItemQuery) -> Result<ClipboardItemPage> {
        let core = self.clone();
        self.hop(async move {
            let pool = core.0.db.pool().await;
            let (list, total) = db::items::query_items_page(&pool, &query).await?;
            let has_more = query.offset + (list.len() as i64) < total;
            Ok(ClipboardItemPage {
                list,
                total,
                has_more,
            })
        })
        .await
    }

    /// 单条记录的完整字段，预览与写回用。
    pub async fn find_item(&self, id: &str) -> Result<Option<ClipboardItem>> {
        let core = self.clone();
        let id = id.to_owned();
        self.hop(async move {
            let pool = core.0.db.pool().await;
            db::items::find_item_by_id(&pool, &id).await
        })
        .await
    }

    pub async fn list_groups(&self) -> Result<Vec<ClipboardGroup>> {
        let core = self.clone();
        self.hop(async move {
            let pool = core.0.db.pool().await;
            db::groups::list_groups(&pool).await
        })
        .await
    }

    // ---- 历史清理 ----

    /// 清理状态快照（最近一次清理、存储占用、自动清理是否暂停）。
    pub fn cleanup_status(&self) -> CleanupStatus {
        clipboard::cleanup::status(&self.0)
    }

    /// 历史设置读盘时有回落且用户还没保存过，后台自动清理处于暂停。
    pub fn cleanup_paused(&self) -> bool {
        self.0.settings.cleanup_paused()
    }

    /// 立即按当前设置完整清理一次（用户的显式操作，自动清理暂停时也执行）。
    pub async fn run_cleanup_now(&self) -> Result<CleanupReport> {
        let core = self.clone();
        self.hop(async move { clipboard::cleanup::run_now(&core.0).await })
            .await
    }

    /// 按候选历史设置预演一轮清理，事务回滚，不删任何东西。
    pub async fn preview_cleanup(&self, history: History) -> Result<CleanupPreview> {
        let core = self.clone();
        self.hop(async move { clipboard::cleanup::preview(&core.0, &history).await })
            .await
    }

    /// 数据实际占用的字节数（数据目录减去 SQLite 可复用空间和 WAL 旁路文件）。
    pub async fn storage_bytes_in_use(&self) -> Result<u64> {
        let core = self.clone();
        self.hop(async move {
            let pool = core.0.db.pool().await;
            clipboard::cleanup::storage_bytes_in_use(&core.0, &pool).await
        })
        .await
    }

    async fn hop<T, F>(&self, fut: F) -> Result<T>
    where
        F: std::future::Future<Output = Result<T>> + Send + 'static,
        T: Send + 'static,
    {
        hop(&self.0.rt, fut).await
    }

    fn emit_settings(&self, settings: &Settings, delta: SettingsDelta) {
        self.0.events.emit(CoreEvent::SettingsUpdated {
            settings: Arc::new(settings.clone()),
            delta,
        });
    }
}

impl CoreInner {
    fn cleanup_task(&self) -> std::sync::MutexGuard<'_, Option<JoinHandle<()>>> {
        self.cleanup_task
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::sync::Mutex as StdMutex;
    use std::task::{Context, Poll, Wake, Waker};

    use chrono::{Duration, Utc};
    use serde_json::json;

    use super::*;
    use crate::clipboard::{MemoryClipboard, MemoryState};
    use crate::db::models::ClipboardKind;
    use crate::env::AppEnv;
    use crate::runtime::CoreRuntime;

    /// 不依赖 tokio 的最小执行器：模拟 GPUI 这类非 tokio 执行器在自己的线程上等待 core 的 future。
    fn block_on<F: Future>(future: F) -> F::Output {
        struct ThreadWaker(std::thread::Thread);

        impl Wake for ThreadWaker {
            fn wake(self: Arc<Self>) {
                self.0.unpark();
            }
        }

        let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
        let mut context = Context::from_waker(&waker);
        let mut future = pin!(future);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            std::thread::park();
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        runtime: CoreRuntime,
        paths: CorePaths,
        events: Arc<StdMutex<Vec<CoreEvent>>>,
    }

    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let local = temp.path().join("local");
            let paths = CorePaths::new(AppEnv::Dev, local.clone(), local.join("logs"), None);

            Self {
                _temp: temp,
                runtime: CoreRuntime::new().unwrap(),
                paths,
                events: Arc::default(),
            }
        }

        fn write_settings(&self, content: &str) {
            let dir = self.paths.config_dir().unwrap();
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("settings.json"), content).unwrap();
        }

        fn start(&self) -> Core {
            let events = self.events.clone();
            let sink = move |event: CoreEvent| {
                events.lock().unwrap().push(event);
            };
            let info = AppInfo {
                name: crate::APP_NAME,
                identifier: crate::APP_IDENTIFIER,
                version: semver::Version::new(2, 0, 0),
                env: AppEnv::Dev,
            };

            block_on(Core::start(
                info,
                self.paths.clone(),
                CoreOptions::default(),
                Arc::new(sink),
                self.runtime.handle(),
            ))
            .unwrap()
        }

        fn take_events(&self) -> Vec<CoreEvent> {
            std::mem::take(&mut *self.events.lock().unwrap())
        }
    }

    fn sample_png(w: u32, h: u32) -> Vec<u8> {
        let buf = image::RgbaImage::from_pixel(w, h, image::Rgba([9, 8, 7, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(buf)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    /// 测试线程上没有 tokio 上下文，每个公开方法都直接在自制执行器里 await。
    #[test]
    fn facade_works_without_a_tokio_context() {
        assert!(Handle::try_current().is_err());
        let fixture = Fixture::new();
        let core = fixture.start();

        let settings =
            block_on(core.update_settings(json!({"appearance": {"theme": "dark"}}))).unwrap();
        assert_eq!(settings.appearance.theme, crate::settings::Theme::Dark);
        assert_eq!(
            core.settings().appearance.theme,
            crate::settings::Theme::Dark
        );

        let clipboard = MemoryClipboard::with_state(MemoryState {
            text: Some("https://example.com".to_owned()),
            ..MemoryState::default()
        });
        let payload = core.read_payload(&clipboard).unwrap().unwrap();
        let item = core.build_item(&payload).unwrap().unwrap();
        let first = block_on(core.store_item(item.clone(), None)).unwrap();
        let again = block_on(core.store_item(item.clone(), None)).unwrap();
        assert!(!first.deduplicated);
        assert!(again.deduplicated);
        assert_eq!(first.id, again.id);

        let image = MemoryClipboard::with_state(MemoryState {
            png: Some(sample_png(30, 20)),
            ..MemoryState::default()
        });
        let image_item = core
            .build_item(&core.read_payload(&image).unwrap().unwrap())
            .unwrap()
            .unwrap();
        block_on(core.store_item(image_item.clone(), None)).unwrap();
        let thumbnail = block_on(core.ensure_thumbnail(&image_item.content)).unwrap();
        assert!(thumbnail.is_file());
        assert!(core
            .image_origin_path(&image_item.content)
            .unwrap()
            .is_file());
        assert!(block_on(core.ensure_thumbnail("../escape.png")).is_err());

        let page = block_on(core.query_items_raw(ClipboardItemQuery::default())).unwrap();
        assert_eq!(page.total, 2);
        assert!(!page.has_more);
        let full = block_on(core.find_item(&first.id)).unwrap().unwrap();
        assert_eq!(full.content, "https://example.com");
        assert!(block_on(core.list_groups()).unwrap().is_empty());

        let target = MemoryClipboard::new();
        core.write_to_clipboard(&target, &full, false).unwrap();
        assert_eq!(
            target.snapshot().text.as_deref(),
            Some("https://example.com")
        );
        assert!(core.writeback_guard().should_skip(&full.content_hash));
        core.write_text_fragment(&target, "example").unwrap();
        assert_eq!(target.snapshot().text.as_deref(), Some("example"));

        // 小库的 WAL 比数据本身还大，Windows 上目录枚举拿到的 WAL 大小又滞后，这里只验证能算出来。
        block_on(core.storage_bytes_in_use()).unwrap();
        let preview = block_on(core.preview_cleanup(History::default())).unwrap();
        assert_eq!(preview.removed, 0);
        assert_eq!(block_on(core.run_cleanup_now()).unwrap().removed, 0);
        assert!(!core.cleanup_status().auto_cleanup_paused);

        let events = fixture.take_events();
        assert!(events.iter().any(|event| matches!(
            event,
            CoreEvent::SettingsUpdated { delta, .. } if delta.touches("appearance.theme")
        )));
        let upserts: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                CoreEvent::ClipboardUpserted {
                    kind, deduplicated, ..
                } => Some((*kind, *deduplicated)),
                _ => None,
            })
            .collect();
        assert_eq!(
            upserts,
            [
                (ClipboardKind::Text, false),
                (ClipboardKind::Text, true),
                (ClipboardKind::Image, false)
            ]
        );

        block_on(core.shutdown()).unwrap();
    }

    #[test]
    fn automatic_cleanup_waits_until_history_settings_are_saved() {
        let fixture = Fixture::new();
        fixture.write_settings(
            r#"{"clipboard": {"history": {"retention": {"value": 1, "unit": "minutes"}, "maxCount": "many"}}}"#,
        );
        let core = fixture.start();
        assert!(core.cleanup_paused());

        let clipboard = MemoryClipboard::with_state(MemoryState {
            text: Some("old record".to_owned()),
            ..MemoryState::default()
        });
        let mut item = core
            .build_item(&core.read_payload(&clipboard).unwrap().unwrap())
            .unwrap()
            .unwrap();
        item.created_at = Utc::now() - Duration::days(2);
        item.updated_at = item.created_at;
        let stored = block_on(core.store_item(item, None)).unwrap();

        block_on(core.0.rt.spawn({
            let core = core.clone();
            async move { clipboard::cleanup::run_due(&core.0).await }
        }))
        .unwrap();
        assert!(block_on(core.find_item(&stored.id)).unwrap().is_some());
        assert!(core.cleanup_status().auto_cleanup_paused);

        // 用户在偏好页保存一次历史设置：暂停解除，过期记录被清理。
        block_on(core.update_settings(json!({"clipboard": {"history": {"maxCount": 0}}}))).unwrap();
        assert!(!core.cleanup_paused());
        block_on(core.0.rt.spawn({
            let core = core.clone();
            async move { clipboard::cleanup::run_due(&core.0).await }
        }))
        .unwrap();

        assert!(block_on(core.find_item(&stored.id)).unwrap().is_none());
        assert!(fixture
            .take_events()
            .iter()
            .any(|event| matches!(event, CoreEvent::ClipboardCleaned { removed: 1 })));
        block_on(core.shutdown()).unwrap();
    }
}
