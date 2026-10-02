//! 接 core 的适配器：`Core::list_items` 的 `ClipboardItemView` 转成列表的 [`ListItem`]。
//!
//! core 的公开 async 方法自己跳到 core runtime 上执行，这里的 future 可以直接在 GPUI 的执行器里 await。

use std::{path::PathBuf, sync::Arc};

use futures::{FutureExt as _, future::BoxFuture};
use kwikpaste_core::{
    AppEnv, AppInfo, Core, CoreEvent, CoreOptions, CorePaths, CoreRuntime,
    clipboard::{ClipboardPayload, ImagePayload, MemoryClipboard, TextPayload},
    db::models::{ClipboardItemQuery, ClipboardKind, ClipboardSubKind, Platform as CorePlatform},
    presenter::{ClipboardAction, ClipboardItemView, FileEntry, FilesPreviewKind},
};

use super::{
    ClipboardSource, ListQuery,
    synthetic::{self, AssetSet, GenerateOptions},
};
use crate::clipboard::model::{
    item::{FileRow, FilesPreview, ItemAction, ItemKind, ListItem, Platform, SubKind},
    layout::ImageBox,
    list_model::Page,
};

/// 以 core 为数据源。
pub struct CoreSource {
    core: Core,
    /// 自测时由这里持有 core 的 runtime；正式入口由宿主持有，这里为 `None`。
    _runtime: Option<Arc<CoreRuntime>>,
}

impl CoreSource {
    pub fn new(core: Core) -> Self {
        Self {
            core,
            _runtime: None,
        }
    }

    /// 连同 runtime 一起持有：数据源活多久，core 的线程就活多久。
    pub fn with_runtime(core: Core, runtime: Arc<CoreRuntime>) -> Self {
        Self {
            _runtime: Some(runtime),
            ..Self::new(core)
        }
    }
}

impl ClipboardSource for CoreSource {
    fn list(&self, query: ListQuery) -> BoxFuture<'static, anyhow::Result<Page>> {
        let core = self.core.clone();

        async move {
            let page = core
                .list_items(ClipboardItemQuery {
                    limit: i64::try_from(query.limit).unwrap_or(i64::MAX),
                    offset: i64::try_from(query.offset).unwrap_or(i64::MAX),
                    ..ClipboardItemQuery::default()
                })
                .await?;

            Ok(Page {
                items: page
                    .list
                    .into_iter()
                    .map(|view| Arc::new(ListItem::from(view)))
                    .collect(),
                total: usize::try_from(page.total).unwrap_or_default(),
            })
        }
        .boxed()
    }

    fn thumbnail(&self, file_name: Arc<str>) -> BoxFuture<'static, anyhow::Result<PathBuf>> {
        let core = self.core.clone();

        async move { Ok(core.ensure_thumbnail(&file_name).await?) }.boxed()
    }
}

/// 自测：在临时目录里启动一个独立的 core（开发环境、内存剪贴板、不启动监听），灌入合成记录。
///
/// 只碰 `<临时目录>/kwikpaste-selftest-core-<pid>`，不读本机任何 1.x 或开发版数据，不碰系统剪贴板。
pub fn start_selftest_core(assets: &AssetSet) -> anyhow::Result<CoreSource> {
    let root = std::env::temp_dir().join(format!("kwikpaste-selftest-core-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let local = root.join("local");
    let paths = CorePaths::new(AppEnv::Dev, local.clone(), local.join("logs"), None);
    let runtime = Arc::new(CoreRuntime::new()?);
    let info = AppInfo {
        name: kwikpaste_core::APP_NAME,
        identifier: SELFTEST_IDENTIFIER,
        version: semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
        env: AppEnv::Dev,
    };
    let sink = |event: CoreEvent| log::debug!("selftest core event: {event:?}");

    let core = futures::executor::block_on(Core::start(
        info,
        paths,
        CoreOptions::default(),
        Arc::new(sink),
        runtime.handle(),
    ))?;
    core.set_clipboard_provider(Arc::new(MemoryClipboard::default()));
    futures::executor::block_on(seed(&core, assets))?;
    log::info!("selftest core started in {}", root.display());

    Ok(CoreSource::with_runtime(core, runtime))
}

/// 自测 core 的 identifier（只写进它自己临时目录里的 storage.json）。
const SELFTEST_IDENTIFIER: &str = "com.fastthree.kwikpaste.native-dev.selftest";

/// 按 core 的采集流程（`build_item` → `store_item`）存几十条合成记录：文本、各子类型、图片、文件。
async fn seed(core: &Core, assets: &AssetSet) -> anyhow::Result<()> {
    let mut payloads = Vec::new();
    for index in 0..24usize {
        let item = synthetic::item(
            assets,
            index,
            GenerateOptions {
                pinned: 0,
                missing_thumbnails: false,
                rows: 24,
            },
        );
        let payload = match item.kind {
            ItemKind::Text => item.summary.as_ref().map(|summary| {
                ClipboardPayload::Text(TextPayload {
                    text: summary.to_string(),
                    html: None,
                    rtf: None,
                })
            }),
            ItemKind::Image => match item.image_thumbnail_path.as_deref() {
                Some(path) => {
                    let bytes = std::fs::read(path)?;
                    let (width, height) = image::image_dimensions(path)?;
                    Some(ClipboardPayload::Image(ImagePayload {
                        bytes,
                        width,
                        height,
                    }))
                }
                None => None,
            },
            ItemKind::Files => Some(ClipboardPayload::Files(
                assets
                    .app_icons
                    .iter()
                    .take(1 + index % 3)
                    .map(|path| path.to_string())
                    .collect(),
            )),
        };
        payloads.extend(payload);
    }

    // 最旧的先存，列表按更新时间倒序，最后存的在最上面。
    for payload in payloads.into_iter().rev() {
        if let Some(item) = core.build_item(&payload)? {
            core.store_item(item, None).await?;
        }
    }

    Ok(())
}

fn shared(text: String) -> Arc<str> {
    Arc::from(text)
}

fn dimension(value: Option<i64>) -> Option<u32> {
    value.and_then(|value| u32::try_from(value).ok())
}

/// core 的记录类型转成列表的类型。
pub fn item_kind(kind: ClipboardKind) -> ItemKind {
    match kind {
        ClipboardKind::Text => ItemKind::Text,
        ClipboardKind::Image => ItemKind::Image,
        ClipboardKind::Files => ItemKind::Files,
    }
}

impl From<ClipboardItemView> for ListItem {
    fn from(view: ClipboardItemView) -> Self {
        let item = view.item;

        Self {
            id: shared(item.id),
            kind: item_kind(item.kind),
            sub_kind: item.sub_kind.map(|sub_kind| match sub_kind {
                ClipboardSubKind::Rtf => SubKind::Rtf,
                ClipboardSubKind::Html => SubKind::Html,
                ClipboardSubKind::Url => SubKind::Url,
                ClipboardSubKind::Email => SubKind::Email,
                ClipboardSubKind::Color => SubKind::Color,
                ClipboardSubKind::Path => SubKind::Path,
            }),
            group_id: item.group_id.map(shared),
            content: shared(item.content),
            summary: item.summary.map(shared),
            width: dimension(item.width),
            height: dimension(item.height),
            is_favorite: item.is_favorite,
            is_pinned: item.is_pinned,
            is_sensitive: item.is_sensitive,
            platform: match item.platform {
                CorePlatform::Macos => Platform::Macos,
                CorePlatform::Windows => Platform::Windows,
            },
            note: item.note.map(shared),
            created_at: item.created_at,
            source_app_id: item.source_app_id.map(shared),
            source_app_name: item.source_app_name.map(shared),
            source_app_icon_path: view.source_app_icon_path.map(shared),
            origin_device_id: item.origin_device_id.map(shared),
            origin_device_name: view.origin_device_name.map(shared),
            image_thumbnail_path: view.image_thumbnail_path.map(shared),
            file_entries: view
                .file_entries
                .map(|entries| entries.into_iter().map(FileRow::from).collect()),
            files_preview_kind: view.files_preview_kind.map(|kind| match kind {
                FilesPreviewKind::ImagePreview => FilesPreview::ImagePreview,
                FilesPreviewKind::List => FilesPreview::List,
            }),
            available_actions: view
                .available_actions
                .into_iter()
                .map(ItemAction::from)
                .collect(),
            color_preview: view.color_preview.map(shared),
            quick_snippets: view.quick_snippets.into_iter().map(shared).collect(),
            image_display: view.image_display_size.map(|size| ImageBox {
                width: size.width as f32,
                height: size.height as f32,
            }),
        }
    }
}

impl From<FileEntry> for FileRow {
    fn from(entry: FileEntry) -> Self {
        Self {
            path: shared(entry.path),
            name: shared(entry.name),
            is_dir: entry.is_dir,
            is_image: entry.is_image,
            exists: entry.exists,
            icon_path: entry.icon_path.map(shared),
            width: None,
            height: None,
        }
    }
}

impl From<ClipboardAction> for ItemAction {
    fn from(action: ClipboardAction) -> Self {
        match action {
            ClipboardAction::Paste => Self::Paste,
            ClipboardAction::PasteAsPlainText => Self::PasteAsPlainText,
            ClipboardAction::PasteAsPath => Self::PasteAsPath,
            ClipboardAction::Copy => Self::Copy,
            ClipboardAction::SaveImage => Self::SaveImage,
            ClipboardAction::SplitWords => Self::SplitWords,
            ClipboardAction::OpenLink => Self::OpenLink,
            ClipboardAction::SendEmail => Self::SendEmail,
            ClipboardAction::RevealInFinder => Self::RevealInFinder,
            ClipboardAction::RevealInExplorer => Self::RevealInExplorer,
            ClipboardAction::ToggleFavorite => Self::ToggleFavorite,
            ClipboardAction::TogglePinned => Self::TogglePinned,
            ClipboardAction::EditNote => Self::EditNote,
            ClipboardAction::Select => Self::Select,
            ClipboardAction::Delete => Self::Delete,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::clipboard::model::item::FilesPreview;

    /// 真 core（临时目录、内存剪贴板）存入合成记录，经适配器读回列表。
    #[test]
    fn lists_seeded_records_through_the_real_core() {
        let dir = std::env::temp_dir().join(format!("kp-core-source-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let png = |name: &str, width: u32, height: u32| -> Arc<str> {
            let path = dir.join(name);
            image::RgbaImage::from_pixel(width, height, image::Rgba([40, 90, 200, 255]))
                .save(&path)
                .expect("write png");
            Arc::from(path.to_string_lossy().as_ref())
        };
        let images = (0..2)
            .map(|index| synthetic::SyntheticImage {
                file_name: Arc::from(format!("seed-{index}.png")),
                width: 120,
                height: 60,
                thumbnail: png(&format!("seed-{index}.png"), 120, 60),
            })
            .collect();
        let assets = AssetSet {
            root: PathBuf::from(&dir),
            images,
            originals: Vec::new(),
            app_icons: vec![png("icon-a.png", 16, 16), png("icon-b.png", 16, 16)],
            file_icons: Vec::new(),
        };

        let source = start_selftest_core(&assets).expect("core starts");
        let page = futures::executor::block_on(source.list(ListQuery {
            offset: 0,
            limit: 100,
        }))
        .expect("lists");

        assert_eq!(page.total, page.items.len());
        assert!(page.items.iter().any(|item| item.kind == ItemKind::Text));
        let image = page
            .items
            .iter()
            .find(|item| item.kind == ItemKind::Image)
            .expect("an image record");
        assert!(image.image_display.is_some(), "core sends the display size");
        assert!(
            page.items
                .iter()
                .any(|item| { item.kind == ItemKind::Files && item.files_preview_kind.is_some() })
        );
        assert!(
            page.items
                .iter()
                .all(|item| !item.available_actions.is_empty())
        );
        // 单个存在的 PNG 文件按图片预览显示。
        assert!(
            page.items
                .iter()
                .any(|item| { item.files_preview_kind == Some(FilesPreview::ImagePreview) })
        );

        let thumbnail = futures::executor::block_on(source.thumbnail(image.content.clone()))
            .expect("thumbnail generated");
        assert!(thumbnail.exists());

        drop(source);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(
            std::env::temp_dir().join(format!("kwikpaste-selftest-core-{}", std::process::id())),
        );
    }
}
