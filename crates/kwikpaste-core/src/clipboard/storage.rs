//! 图片落盘：按内容哈希分片存储原图与缩略图。
//!
//! 目录布局（`content` 字段存文件名 `<hash>.png`，分片目录由此函数推导，不入库）：
//! ```text
//! <app_local_data>/resources/clipboard-images/
//!   origin/<hash[..2]>/<hash>.png       原图（PNG），复制时落盘
//!   thumbnails/<hash[..2]>/<hash>.png   缩略图（PNG，最长边 <= THUMBNAIL_MAX），首次预览时按需生成
//!   file-thumbnails/<key[..2]>/<key>.png 单图文件记录（复制的是一个图片文件）的缩略图，按需生成；
//!                                       key 由文件路径、大小和修改时间算出，文件改了就换一张
//! ```
//! `file-thumbnails` 是可以随时重建的缓存：清理缓存时整个清掉，也不进备份。
//! 文件名取「PNG 字节的 blake3」：同一张图重复复制 → 同字节 → 同文件名，落盘幂等，
//! 且与去重指纹同源（image 的 `content_hash` 即对 PNG 字节哈希）。
//! 按 hash 前 2 位 hex 分 256 个子目录，避免重度使用下单目录文件爆量。
//!
//! 缩略图的解码/缩放/编码不在复制热路径上——`store` 只写原图，缩略图由
//! [`ImageStore::ensure_thumbnail_async`] 在前端首次展示时按需生成并缓存，
//! 同时生成的张数受信号量限制，一屏图片首次出现时不会起几十个解码线程。

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use anyhow::Context;
use blake3::Hasher;
use clipboard_rs::common::{RustImage, RustImageData};
use tokio::sync::Semaphore;

use super::payload::ImagePayload;
use crate::error::{AppError, Result};
use crate::paths::CorePaths;

/// 缩略图最长边像素。仅用于列表预览，够清晰即可。
pub const THUMBNAIL_MAX: u32 = 300;

/// 剪贴板图片目录名，挂在 [`CorePaths::resources_dir`] 下（与 `app-icons` 并列）。
const IMAGES_DIR: &str = "clipboard-images";
const ORIGIN_DIR: &str = "origin";
const THUMBNAILS_DIR: &str = "thumbnails";
/// 单图文件记录的缩略图缓存目录名，挂在图片根目录下。
pub(crate) const FILE_THUMBNAILS_DIR: &str = "file-thumbnails";
/// 图片文件超过这个大小就不生成缩略图：解码一张几百 MB 的 TIFF 只为一个几十像素高的卡片不值得。
const FILE_THUMBNAIL_SOURCE_MAX: u64 = 64 * 1024 * 1024;

/// 一次图片落盘的结果，交给 ingest 写入 `ClipboardItem`。
pub struct StoredImage {
    /// 入库 `content`：图片文件名 `<hash>.png`（不含分片目录）。
    pub file_name: String,
    /// 去重指纹来源：PNG 字节的 blake3（十六进制）。
    #[allow(dead_code)]
    pub content_digest: String,
    pub width: i64,
    pub height: i64,
    /// 原图字节数。
    pub size: i64,
}

/// 图片存储器：持有 app data 下的 `resources/clipboard-images` 根目录。
/// 由 `Core` 持有，监听线程与各项操作共用。
#[derive(Clone)]
pub struct ImageStore {
    images_root: Arc<RwLock<PathBuf>>,
    /// 同时生成缩略图的并发许可；解码 / 缩放大图是 CPU 密集活，多张同时跑只会互相拖慢。
    thumbnail_permits: Arc<Semaphore>,
}

impl ImageStore {
    /// 解析 `<data_root>/resources/clipboard-images` 作为根。
    pub fn new(paths: &CorePaths) -> Result<Self> {
        let images_root = paths.resources_dir()?.join(IMAGES_DIR);
        Ok(Self::with_root(images_root))
    }

    #[cfg(test)]
    pub(crate) fn for_test(images_root: PathBuf) -> Self {
        Self::with_root(images_root)
    }

    fn with_root(images_root: PathBuf) -> Self {
        Self {
            images_root: Arc::new(RwLock::new(images_root)),
            thumbnail_permits: Arc::new(Semaphore::new(thumbnail_parallelism())),
        }
    }

    /// 重新绑定到当前真实数据根；数据目录热迁移后由存储命令调用。
    pub fn rebase(&self, paths: &CorePaths) -> Result<()> {
        let next = paths.resources_dir()?.join(IMAGES_DIR);
        *self
            .images_root
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = next;
        Ok(())
    }

    /// 落盘原图，返回 [`StoredImage`]。已存在的文件跳过写入（幂等）。
    ///
    /// 缩略图**不在此生成**——它已移出复制热路径，改由 [`Self::ensure_thumbnail`] 在
    /// 前端首次预览时按需生成并缓存。复制路径只做「哈希 + 写原图」，避免大图编解码卡顿。
    pub fn store(&self, image: &ImagePayload) -> Result<StoredImage> {
        let content_digest = blake3_hex(&image.bytes);
        let file_name = format!("{content_digest}.png");

        let origin_path = self.shard_path(ORIGIN_DIR, &content_digest, &file_name);
        write_if_absent(&origin_path, &image.bytes)?;

        Ok(StoredImage {
            file_name,
            content_digest,
            width: i64::from(image.width),
            height: i64::from(image.height),
            size: image.bytes.len() as i64,
        })
    }

    /// 确保缩略图存在并返回其绝对路径：已存在直接返回；否则读原图 → 解码 → 缩放 → 编码 PNG → 落盘。
    ///
    /// 供 `get_clipboard_image_path(thumbnail=true)` 调用。把生成放在「读」而非「写」侧，
    /// 既将解码/编码移出复制热路径，又因「返回前文件已确保存在」天然避免前端加载到半成品文件。
    pub fn ensure_thumbnail(&self, file_name: &str) -> Result<PathBuf> {
        let thumb_path = self.thumbnail_path(file_name);
        if thumb_path.exists() {
            return Ok(thumb_path);
        }

        let origin_path = self.origin_path(file_name);
        let origin_bytes = std::fs::read(&origin_path)
            .with_context(|| format!("failed to read origin image {origin_path:?}"))?;
        let thumb_bytes = encode_thumbnail(&origin_bytes)?;
        write_if_absent(&thumb_path, &thumb_bytes)?;
        Ok(thumb_path)
    }

    /// [`Self::ensure_thumbnail`] 的异步版本：先拿并发许可，再到阻塞线程池里解码，
    /// 已存在的缩略图不占许可直接返回。列表首次展示与预览取图都走这里。
    ///
    /// 必须在 core 的 tokio runtime 里调用；宿主经 `Core::ensure_thumbnail` 使用。
    pub(crate) async fn ensure_thumbnail_async(&self, file_name: &str) -> Result<PathBuf> {
        let thumb_path = self.thumbnail_path(file_name);
        if thumb_path.exists() {
            return Ok(thumb_path);
        }

        let _permit = self
            .thumbnail_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|err| AppError::Clipboard(format!("thumbnail semaphore closed: {err}")))?;
        let store = self.clone();
        let file_name = file_name.to_owned();
        tokio::task::spawn_blocking(move || store.ensure_thumbnail(&file_name))
            .await
            .map_err(|err| AppError::Clipboard(format!("thumbnail task join failed: {err}")))?
    }

    /// 单图文件记录的缩略图路径（不管生成没有）；读不到文件信息时返回 `None`。
    pub fn file_thumbnail_path(&self, source: &Path) -> Option<PathBuf> {
        let key = file_thumbnail_key(source)?;
        let file_name = format!("{key}.png");
        Some(self.shard_path(FILE_THUMBNAILS_DIR, &key, &file_name))
    }

    /// 确保单图文件记录的缩略图存在并返回路径。按 EXIF 方向摆正后缩放，与图片记录同样的尺寸规则。
    pub fn ensure_file_thumbnail(&self, source: &Path) -> Result<PathBuf> {
        let thumb_path = self
            .file_thumbnail_path(source)
            .ok_or_else(|| AppError::Clipboard(format!("image file is unreadable: {source:?}")))?;
        if thumb_path.exists() {
            return Ok(thumb_path);
        }

        let size = std::fs::metadata(source)
            .with_context(|| format!("failed to read {source:?}"))?
            .len();
        if size > FILE_THUMBNAIL_SOURCE_MAX {
            return Err(AppError::Clipboard(format!(
                "image file is too large for a thumbnail: {size} bytes"
            )));
        }
        let thumb_bytes = encode_file_thumbnail(source)?;
        write_if_absent(&thumb_path, &thumb_bytes)?;
        Ok(thumb_path)
    }

    /// [`Self::ensure_file_thumbnail`] 的异步版本，与图片记录共用并发许可。
    pub(crate) async fn ensure_file_thumbnail_async(&self, source: &Path) -> Result<PathBuf> {
        if let Some(path) = self
            .file_thumbnail_path(source)
            .filter(|path| path.exists())
        {
            return Ok(path);
        }

        let _permit = self
            .thumbnail_permits
            .clone()
            .acquire_owned()
            .await
            .map_err(|err| AppError::Clipboard(format!("thumbnail semaphore closed: {err}")))?;
        let store = self.clone();
        let source = source.to_path_buf();
        tokio::task::spawn_blocking(move || store.ensure_file_thumbnail(&source))
            .await
            .map_err(|err| AppError::Clipboard(format!("thumbnail task join failed: {err}")))?
    }

    /// 删除一张图片的原图与缩略图。缩略图懒生成、可能不存在，缺失文件视作成功（幂等）。
    /// 删后顺手清理变空的分片目录（`origin/<ab>`、`thumbnails/<ab>`）。
    ///
    /// 调用前提：库里该图至多一行（image 去重指纹源自 PNG 字节，落盘文件名即字节哈希），
    /// 故删行后该文件必为孤儿，可直接删，无需引用计数。其余 IO 错误上抛由调用方记日志。
    pub fn remove(&self, file_name: &str) -> Result<()> {
        let origin = self.origin_path(file_name);
        let thumb = self.thumbnail_path(file_name);
        remove_if_present(&origin)?;
        remove_if_present(&thumb)?;
        // 分片目录可能被同前缀的其他图共享，非空时保留——remove_dir 只删空目录，
        // 非空 / 不存在都返回 Err，一并忽略；目录清理是尽力而为，不影响删图结果。
        remove_dir_if_empty(origin.parent());
        remove_dir_if_empty(thumb.parent());
        Ok(())
    }

    /// 一张图片在磁盘上的占用：原图 + 已生成的缩略图；读不到的文件按 0 计。
    pub fn stored_bytes(&self, file_name: &str) -> u64 {
        [self.origin_path(file_name), self.thumbnail_path(file_name)]
            .iter()
            .filter_map(|path| std::fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .sum()
    }

    /// 由文件名解析原图绝对路径（分片目录从文件名前 2 位推导）。供写回/粘贴使用。
    pub fn origin_path(&self, file_name: &str) -> PathBuf {
        self.shard_path(ORIGIN_DIR, shard_key(file_name), file_name)
    }

    /// 由文件名解析缩略图绝对路径。供前端预览取图。
    pub fn thumbnail_path(&self, file_name: &str) -> PathBuf {
        self.shard_path(THUMBNAILS_DIR, shard_key(file_name), file_name)
    }

    fn shard_path(&self, kind_dir: &str, shard_src: &str, file_name: &str) -> PathBuf {
        self.images_root()
            .join(kind_dir)
            .join(shard_dir(shard_src))
            .join(file_name)
    }

    fn images_root(&self) -> PathBuf {
        self.images_root
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

/// 校验图片文件名：必须是单层 `<name>.png`，不含路径分隔符 / 父目录引用。
/// 文件名来自界面（即记录的 `content`），拼路径前必须先过这一关，防止路径穿越。
pub fn validate_image_file_name(file_name: &str) -> Result<()> {
    let invalid = file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
        || !file_name.ends_with(".png");

    if invalid {
        return Err(AppError::Clipboard(format!(
            "invalid image file name: {file_name:?}"
        )));
    }
    Ok(())
}

/// 同时生成缩略图的张数：留一半核心给采集、检索和 UI，且至少 1、最多 4。
fn thumbnail_parallelism() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get() / 2)
        .unwrap_or(1)
        .clamp(1, 4)
}

/// 分片子目录名：取来源串前 2 个字符（hash 恒为 hex，必有 2 位）；
/// 异常短串兜底为 `00`，保证始终有一层分片。
fn shard_dir(src: &str) -> &str {
    if src.len() >= 2 {
        &src[..2]
    } else {
        "00"
    }
}

/// 从文件名 `<hash>.png` 取分片来源（即 hash 本身）。
fn shard_key(file_name: &str) -> &str {
    file_name.split('.').next().unwrap_or(file_name)
}

fn blake3_hex(bytes: &[u8]) -> String {
    let mut hasher = Hasher::new();
    hasher.update(bytes);
    hasher.finalize().to_hex().to_string()
}

/// 写文件（自动建分片目录）；目标已存在则跳过，保证幂等且不重复 IO。
fn write_if_absent(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create image dir {parent:?}"))?;
    }
    std::fs::write(path, bytes).with_context(|| format!("failed to write image {path:?}"))?;
    Ok(())
}

/// 删文件；文件不存在时静默成功（幂等），其余 IO 错误上抛。
fn remove_if_present(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(AppError::from(
            anyhow::Error::new(err).context(format!("failed to remove image {path:?}")),
        )),
    }
}

/// 尽力删除空目录：`remove_dir` 仅在目录为空时成功，非空（仍有同分片的其他图）/
/// 不存在都返回 `Err`，一律忽略——目录清理不影响删图结果，无需上抛。
fn remove_dir_if_empty(dir: Option<&Path>) {
    if let Some(dir) = dir {
        let _ = std::fs::remove_dir(dir);
    }
}

/// 图片文件的显示宽高：只读文件头，不解码整图；按 EXIF 方向摆正（转 90° 的宽高互换）。
/// 不认识的格式（如 SVG）或读不出来时返回 `None`。
pub fn image_file_dimensions(path: &Path) -> Option<(u32, u32)> {
    use image::ImageDecoder;

    let mut decoder = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?
        .into_decoder()
        .ok()?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return None;
    }
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    Some(if swaps_axes(orientation) {
        (height, width)
    } else {
        (width, height)
    })
}

fn swaps_axes(orientation: image::metadata::Orientation) -> bool {
    use image::metadata::Orientation;

    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

/// 缓存键：路径 + 大小 + 修改时间的 blake3。文件被改写后键随之变化，不会拿到旧图。
fn file_thumbnail_key(source: &Path) -> Option<String> {
    let metadata = std::fs::metadata(source).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());

    let mut hasher = Hasher::new();
    hasher.update(source.to_string_lossy().as_bytes());
    hasher.update(&[0x1f]);
    hasher.update(&metadata.len().to_le_bytes());
    hasher.update(&modified.to_le_bytes());
    Some(hasher.finalize().to_hex().to_string())
}

/// 解码图片文件（PNG、JPEG、GIF、WebP、BMP、TIFF、ICO）→ 按 EXIF 方向摆正 → 缩放 → 编码 PNG。
fn encode_file_thumbnail(source: &Path) -> Result<Vec<u8>> {
    use image::ImageDecoder;

    let mut decoder = image::ImageReader::open(source)
        .with_context(|| format!("failed to open {source:?}"))?
        .with_guessed_format()
        .with_context(|| format!("failed to read {source:?}"))?
        .into_decoder()
        .map_err(clip_err)?;
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = image::DynamicImage::from_decoder(decoder).map_err(clip_err)?;
    image.apply_orientation(orientation);

    // 只缩小不放大，与显示尺寸的算法（缩放比不超过 1）一致。
    if image.width().max(image.height()) > THUMBNAIL_MAX {
        image = image.thumbnail(THUMBNAIL_MAX, THUMBNAIL_MAX);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(clip_err)?;
    Ok(out.into_inner())
}

/// 把原图 PNG 字节解码 → 生成缩略图（最长边 <= [`THUMBNAIL_MAX`]，保持比例）→ 重新编码 PNG。
fn encode_thumbnail(png_bytes: &[u8]) -> Result<Vec<u8>> {
    let image = RustImageData::from_bytes(png_bytes).map_err(clip_err)?;
    let thumb = image
        .thumbnail(THUMBNAIL_MAX, THUMBNAIL_MAX)
        .map_err(clip_err)?;
    Ok(thumb.to_png().map_err(clip_err)?.get_bytes().to_vec())
}

fn clip_err<E: std::fmt::Display>(err: E) -> AppError {
    AppError::Clipboard(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用 image crate 生成一张纯色 PNG 作测试输入。
    fn sample_png(w: u32, h: u32) -> Vec<u8> {
        use std::io::Cursor;
        let buf = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(buf)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    fn temp_store() -> (tempdir_guard::TempDir, ImageStore) {
        let dir = tempdir_guard::TempDir::new();
        let store = ImageStore::for_test(dir.path().join("resources").join("clipboard-images"));
        (dir, store)
    }

    #[test]
    fn stores_origin_under_hash_shard_without_thumbnail() {
        let (_dir, store) = temp_store();
        let payload = ImagePayload {
            bytes: sample_png(64, 48),
            width: 64,
            height: 48,
        };

        let stored = store.store(&payload).unwrap();
        assert!(stored.file_name.ends_with(".png"));
        assert_eq!(stored.file_name, format!("{}.png", stored.content_digest));
        assert_eq!(stored.width, 64);
        assert_eq!(stored.height, 48);
        assert!(stored.size > 0);

        // 原图落在 <前2位>/<file_name>；缩略图此刻尚未生成（移出热路径）。
        let origin = store.origin_path(&stored.file_name);
        let thumb = store.thumbnail_path(&stored.file_name);
        assert!(origin.exists(), "origin should exist: {origin:?}");
        assert!(
            !thumb.exists(),
            "thumbnail should NOT exist before ensure_thumbnail: {thumb:?}"
        );
        assert_eq!(
            origin.parent().unwrap().file_name().unwrap().to_str(),
            Some(&stored.content_digest[..2])
        );
        // 原图字节与输入一致（未改动）。
        assert_eq!(std::fs::read(&origin).unwrap(), payload.bytes);
    }

    #[test]
    fn ensure_thumbnail_generates_then_caches() {
        let (_dir, store) = temp_store();
        let payload = ImagePayload {
            bytes: sample_png(64, 48),
            width: 64,
            height: 48,
        };
        let stored = store.store(&payload).unwrap();

        // 首次：从原图生成缩略图并返回其路径。
        let thumb = store.ensure_thumbnail(&stored.file_name).unwrap();
        assert!(thumb.exists(), "thumbnail should be generated: {thumb:?}");
        assert_eq!(thumb, store.thumbnail_path(&stored.file_name));
        let first_bytes = std::fs::read(&thumb).unwrap();
        assert!(!first_bytes.is_empty());

        // 再次：命中缓存，路径一致、内容不变（幂等，不重复编码）。
        let thumb2 = store.ensure_thumbnail(&stored.file_name).unwrap();
        assert_eq!(thumb, thumb2);
        assert_eq!(std::fs::read(&thumb2).unwrap(), first_bytes);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ensure_thumbnail_async_generates_then_caches_like_sync() {
        let (_dir, store) = temp_store();
        let payload = ImagePayload {
            bytes: sample_png(64, 48),
            width: 64,
            height: 48,
        };
        let stored = store.store(&payload).unwrap();

        let thumb = store
            .ensure_thumbnail_async(&stored.file_name)
            .await
            .unwrap();
        assert!(thumb.exists(), "thumbnail should be generated: {thumb:?}");
        assert_eq!(thumb, store.thumbnail_path(&stored.file_name));
        let first_bytes = std::fs::read(&thumb).unwrap();

        let again = store
            .ensure_thumbnail_async(&stored.file_name)
            .await
            .unwrap();
        assert_eq!(thumb, again);
        assert_eq!(std::fs::read(&again).unwrap(), first_bytes);
    }

    #[test]
    fn accepts_plain_png_file_name() {
        assert!(validate_image_file_name("abcdef0123.png").is_ok());
    }

    #[test]
    fn rejects_traversal_and_subpaths() {
        for bad in [
            "",
            "evil.txt",
            "../secret.png",
            "..\\secret.png",
            "sub/dir.png",
            "a/b.png",
            "/abs.png",
            "name..png", // 含 ".." 序列，保守拒绝
        ] {
            assert!(
                validate_image_file_name(bad).is_err(),
                "should reject: {bad:?}"
            );
        }
    }

    #[test]
    fn thumbnail_parallelism_stays_within_bounds() {
        assert!((1..=4).contains(&thumbnail_parallelism()));
    }

    #[test]
    fn ensure_thumbnail_errors_when_origin_missing() {
        let (_dir, store) = temp_store();
        // 原图从未落盘：ensure_thumbnail 读不到原图，报错而非 panic。
        let result = store.ensure_thumbnail("0000000000000000.png");
        assert!(result.is_err());
    }

    #[test]
    fn store_is_idempotent_for_same_bytes() {
        let (_dir, store) = temp_store();
        let payload = ImagePayload {
            bytes: sample_png(32, 32),
            width: 32,
            height: 32,
        };

        let a = store.store(&payload).unwrap();
        let b = store.store(&payload).unwrap();
        // 同字节 → 同文件名 / 同 digest，幂等。
        assert_eq!(a.file_name, b.file_name);
        assert_eq!(a.content_digest, b.content_digest);
    }

    #[test]
    fn remove_deletes_origin_and_thumbnail_idempotently() {
        let (_dir, store) = temp_store();
        let payload = ImagePayload {
            bytes: sample_png(40, 30),
            width: 40,
            height: 30,
        };
        let stored = store.store(&payload).unwrap();
        store.ensure_thumbnail(&stored.file_name).unwrap();

        let origin = store.origin_path(&stored.file_name);
        let thumb = store.thumbnail_path(&stored.file_name);
        assert!(origin.exists() && thumb.exists());

        store.remove(&stored.file_name).unwrap();
        assert!(!origin.exists(), "origin should be removed");
        assert!(!thumb.exists(), "thumbnail should be removed");
        // 分片目录已空 → 一并清理。
        assert!(
            !origin.parent().unwrap().exists(),
            "empty origin shard dir should be removed"
        );
        assert!(
            !thumb.parent().unwrap().exists(),
            "empty thumbnail shard dir should be removed"
        );

        // 再次删除：文件已不存在，仍成功（幂等）。
        store.remove(&stored.file_name).unwrap();
    }

    #[test]
    fn remove_keeps_shard_dir_when_other_image_shares_prefix() {
        let (_dir, store) = temp_store();
        let payload = ImagePayload {
            bytes: sample_png(40, 30),
            width: 40,
            height: 30,
        };
        let stored = store.store(&payload).unwrap();
        let shard = store
            .origin_path(&stored.file_name)
            .parent()
            .unwrap()
            .to_path_buf();

        // 模拟同前缀的另一张图占用同一分片目录。
        let sibling = shard.join("sibling.png");
        std::fs::write(&sibling, b"x").unwrap();

        store.remove(&stored.file_name).unwrap();
        // 目标图已删，但分片目录非空 → 必须保留，sibling 不受影响。
        assert!(!store.origin_path(&stored.file_name).exists());
        assert!(shard.exists(), "non-empty shard dir must be kept");
        assert!(sibling.exists(), "sibling image must survive");
    }

    #[test]
    fn remove_succeeds_when_thumbnail_never_generated() {
        let (_dir, store) = temp_store();
        let payload = ImagePayload {
            bytes: sample_png(16, 16),
            width: 16,
            height: 16,
        };
        let stored = store.store(&payload).unwrap();
        // 只落了原图，缩略图从未生成；remove 不应因缩略图缺失而失败。
        assert!(!store.thumbnail_path(&stored.file_name).exists());
        store.remove(&stored.file_name).unwrap();
        assert!(!store.origin_path(&stored.file_name).exists());
    }

    #[test]
    fn path_resolution_matches_store_layout() {
        let (_dir, store) = temp_store();
        let file_name = "abcdef0123456789.png";
        assert_eq!(
            store.origin_path(file_name),
            store
                .images_root()
                .join("origin")
                .join("ab")
                .join(file_name)
        );
        assert_eq!(
            store.thumbnail_path(file_name),
            store
                .images_root()
                .join("thumbnails")
                .join("ab")
                .join(file_name)
        );
    }

    /// 手动性能基线用的合成数据：向一个真实 KwikPaste 数据库写入 N 条大尺寸图片记录并落盘原图，
    /// 不生成缩略图，用来复现「首次展示时缩略图尚不存在」的冷路径。只在显式 `--ignored` 时运行。
    ///
    /// 环境变量：`KWIKPASTE_SEED_DB`（sqlite 文件）、`KWIKPASTE_SEED_IMAGES_ROOT`
    /// （`resources/clipboard-images` 目录）、`KWIKPASTE_SEED_COUNT`（默认 24）、
    /// `KWIKPASTE_SEED_WIDTH` / `KWIKPASTE_SEED_HEIGHT`（默认 2560 x 1600）。
    #[tokio::test]
    #[ignore = "writes synthetic rows into a real KwikPaste database; run with --ignored for manual profiling"]
    async fn seed_synthetic_images_for_manual_profiling() {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

        use crate::clipboard::ingest::build_item;
        use crate::clipboard::payload::{ClipboardPayload, ImagePayload};
        use crate::db::items::insert_item;

        let db_path = std::env::var("KWIKPASTE_SEED_DB").expect("KWIKPASTE_SEED_DB is required");
        let images_root = std::env::var("KWIKPASTE_SEED_IMAGES_ROOT")
            .expect("KWIKPASTE_SEED_IMAGES_ROOT is required");
        let count = env_or("KWIKPASTE_SEED_COUNT", 24u32);
        let width = env_or("KWIKPASTE_SEED_WIDTH", 2560u32);
        let height = env_or("KWIKPASTE_SEED_HEIGHT", 1600u32);

        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .expect("open seed database");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("run migrations on seed database");

        let store = ImageStore::for_test(PathBuf::from(images_root));
        for seed in 0..count {
            let bytes = synthetic_png(width, height, seed);
            let payload = ClipboardPayload::Image(ImagePayload {
                bytes,
                width,
                height,
            });
            let item = build_item(&store, &payload)
                .expect("build synthetic image item")
                .expect("image capture is enabled by default");
            insert_item(&pool, &item)
                .await
                .expect("insert synthetic image row");
            assert!(store.origin_path(&item.content).exists());
            assert!(!store.thumbnail_path(&item.content).exists());
        }

        println!("seeded {count} synthetic {width}x{height} images into {db_path}");
    }

    fn env_or<T: std::str::FromStr>(key: &str, default: T) -> T {
        std::env::var(key)
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    }

    /// 渐变底 + 按 seed 变化的色块：每张字节不同（哈希不同），PNG 体积可控，解码尺寸真实。
    fn synthetic_png(w: u32, h: u32, seed: u32) -> Vec<u8> {
        use std::io::Cursor;

        let block_x = (seed * 97) % w.max(1);
        let block_y = (seed * 53) % h.max(1);
        let block = w / 6;
        let tint = ((seed * 41) % 200) as u8;
        let buf = image::RgbaImage::from_fn(w, h, |x, y| {
            let in_block =
                x >= block_x && x < block_x + block && y >= block_y && y < block_y + block;
            if in_block {
                image::Rgba([255 - tint, tint, 128, 255])
            } else {
                image::Rgba([
                    (x * 255 / w.max(1)) as u8,
                    (y * 255 / h.max(1)) as u8,
                    tint,
                    255,
                ])
            }
        });
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(buf)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    /// 极简自清理临时目录（避免引第三方 tempfile 依赖）。
    mod tempdir_guard {
        use std::path::{Path, PathBuf};

        pub struct TempDir(PathBuf);

        impl TempDir {
            pub fn new() -> Self {
                let path = std::env::temp_dir()
                    .join(format!("kwikpaste-imgstore-{}", uuid::Uuid::new_v4()));
                std::fs::create_dir_all(&path).unwrap();
                Self(path)
            }
            pub fn path(&self) -> &Path {
                &self.0
            }
        }

        impl Drop for TempDir {
            fn drop(&mut self) {
                std::fs::remove_dir_all(&self.0).ok();
            }
        }
    }
}

/// 单图文件记录：文件头读宽高、按 EXIF 方向摆正、缩略图按文件内容变化换新。
#[cfg(test)]
mod file_image_tests {
    use std::io::Cursor;

    use super::*;

    fn encode(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
        let buf = image::RgbImage::from_fn(width, height, |x, y| {
            image::Rgb([(x * 7) as u8, (y * 5) as u8, 90])
        });
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(buf)
            .write_to(&mut out, format)
            .unwrap();
        out.into_inner()
    }

    /// 在 JPEG 的 SOI 之后插一段只含方向标记的 EXIF（APP1）。
    fn with_exif_orientation(jpeg: &[u8], orientation: u16) -> Vec<u8> {
        let mut tiff = b"II*\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00".to_vec();
        tiff.extend_from_slice(&orientation.to_le_bytes());
        tiff.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let mut payload = b"Exif\x00\x00".to_vec();
        payload.extend_from_slice(&tiff);
        let length = (payload.len() + 2) as u16;

        let mut out = jpeg[..2].to_vec();
        out.extend_from_slice(&[0xff, 0xe1]);
        out.extend_from_slice(&length.to_be_bytes());
        out.extend_from_slice(&payload);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    fn store(temp: &tempfile::TempDir) -> ImageStore {
        ImageStore::for_test(temp.path().join("resources").join("clipboard-images"))
    }

    #[test]
    fn dimensions_come_from_the_header() {
        let temp = tempfile::tempdir().unwrap();
        for (name, format) in [
            ("a.png", image::ImageFormat::Png),
            ("b.jpg", image::ImageFormat::Jpeg),
            ("c.gif", image::ImageFormat::Gif),
            ("d.bmp", image::ImageFormat::Bmp),
        ] {
            let path = temp.path().join(name);
            std::fs::write(&path, encode(40, 20, format)).unwrap();
            assert_eq!(image_file_dimensions(&path), Some((40, 20)), "{name}");
        }

        let svg = temp.path().join("e.svg");
        std::fs::write(&svg, b"<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
        assert_eq!(image_file_dimensions(&svg), None);
        assert_eq!(
            image_file_dimensions(&temp.path().join("missing.png")),
            None
        );
    }

    /// 手机拍的照片常带「转 90°」的方向标记：宽高互换，缩略图也摆正。
    #[test]
    fn exif_rotation_swaps_the_axes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("photo.jpg");
        std::fs::write(
            &path,
            with_exif_orientation(&encode(40, 20, image::ImageFormat::Jpeg), 6),
        )
        .unwrap();

        assert_eq!(image_file_dimensions(&path), Some((20, 40)));
        let thumb = store(&temp).ensure_file_thumbnail(&path).unwrap();
        assert_eq!(image_file_dimensions(&thumb), Some((20, 40)));
    }

    #[test]
    fn file_thumbnails_are_cached_until_the_file_changes() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let path = temp.path().join("wide.png");
        std::fs::write(&path, encode(900, 300, image::ImageFormat::Png)).unwrap();

        let thumb = store.ensure_file_thumbnail(&path).unwrap();
        assert!(thumb.starts_with(
            temp.path()
                .join("resources")
                .join("clipboard-images")
                .join(FILE_THUMBNAILS_DIR)
        ));
        assert_eq!(image_file_dimensions(&thumb), Some((THUMBNAIL_MAX, 100)));
        assert_eq!(store.file_thumbnail_path(&path), Some(thumb.clone()));
        assert_eq!(store.ensure_file_thumbnail(&path).unwrap(), thumb);

        // 文件改写后（大小不同）缓存键随之变化，拿到的是新图的缩略图。
        std::fs::write(&path, encode(200, 400, image::ImageFormat::Png)).unwrap();
        let changed = store.ensure_file_thumbnail(&path).unwrap();
        assert_ne!(changed, thumb);
        assert_eq!(image_file_dimensions(&changed), Some((150, 300)));

        let text = temp.path().join("notes.png");
        std::fs::write(&text, b"not an image").unwrap();
        assert!(store.ensure_file_thumbnail(&text).is_err());
        assert!(store
            .ensure_file_thumbnail(&temp.path().join("missing.png"))
            .is_err());
        assert_eq!(store.file_thumbnail_path(temp.path()), None);
    }
}
