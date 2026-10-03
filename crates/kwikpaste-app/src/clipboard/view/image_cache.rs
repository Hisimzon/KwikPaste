//! `KpImageCache`：列表里剪贴板图片的有界缓存（附录 D §3.3、L7）。
//!
//! - 键是（文件路径，目标物理尺寸）。解码和缩放都在后台线程完成，缓存里只留缩到显示尺寸的位图，
//!   大图（包括单图文件记录指向的原图，附录 D §8 第 7 条）也不会把原图留在内存或图集里。
//! - LRU 64 项；淘汰时 `cx.drop_image(image, Some(window))` 把它从窗口的图集里删掉。
//! - 面板隐藏时 [`KpImageCache::clear`] 全部释放。
//! - 解码失败记成 [`ImageState::Failed`]，卡片换成静态的“坏图”图标，骨架不再脉动。
//!
//! 卡片直接问这里要 `Arc<RenderImage>`，再用 `img(ImageSource::Render(..))` 按显式尺寸画出来，
//! 不经过 GPUI 的全局资源缓存，也不在外层套 `image_cache()` 元素（那样同一张图会缓存两份）。

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use gpui::{App, Context, RenderImage, Task, Window};

/// 缓存项上限（约 4 MB：64 张 64×300 级别的 BGRA 位图）。
pub const CAPACITY: usize = 64;

/// 缓存键：同一文件按不同显示尺寸分别缓存。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ImageKey {
    pub path: Arc<Path>,
    /// 目标宽高（物理像素）。
    pub width: u32,
    pub height: u32,
}

/// 一次查询的结果。
#[derive(Clone, Debug)]
pub enum ImageState {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

enum Slot {
    Loading(#[allow(dead_code)] Task<()>),
    Ready(Arc<RenderImage>),
    Failed,
}

struct Entry {
    slot: Slot,
    used: u64,
}

/// 计数，自测和跑分读它。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: usize,
    pub decoded: u64,
    pub failed: u64,
    pub evicted: u64,
}

#[derive(Default)]
pub struct KpImageCache {
    entries: HashMap<ImageKey, Entry>,
    clock: u64,
    stats: CacheStats,
}

impl KpImageCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.entries.len(),
            ..self.stats
        }
    }

    /// 取一张图。没有时登记后台解码并返回 `Loading`，解码完成后本实体 `notify`。
    pub fn request(
        &mut self,
        key: ImageKey,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ImageState {
        self.clock += 1;
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.used = self.clock;
            return match &entry.slot {
                Slot::Loading(_) => ImageState::Loading,
                Slot::Ready(image) => ImageState::Ready(image.clone()),
                Slot::Failed => ImageState::Failed,
            };
        }

        if key.width == 0 || key.height == 0 {
            return ImageState::Failed;
        }

        self.evict_for_one(window, cx);
        let task = self.spawn_decode(key.clone(), cx);
        self.entries.insert(
            key,
            Entry {
                slot: Slot::Loading(task),
                used: self.clock,
            },
        );

        ImageState::Loading
    }

    /// 释放全部位图（面板隐藏时）。正在解码的任务随之取消。
    pub fn clear(&mut self, window: Option<&mut Window>, cx: &mut App) {
        let mut window = window;
        for (_, entry) in self.entries.drain() {
            if let Slot::Ready(image) = entry.slot {
                cx.drop_image(image, window.as_deref_mut());
            }
        }
    }

    fn evict_for_one(&mut self, window: &mut Window, cx: &mut App) {
        if self.entries.len() < CAPACITY {
            return;
        }

        let Some(oldest) = self
            .entries
            .iter()
            .min_by_key(|(_, entry)| entry.used)
            .map(|(key, _)| key.clone())
        else {
            return;
        };
        if let Some(entry) = self.entries.remove(&oldest) {
            self.stats.evicted += 1;
            if let Slot::Ready(image) = entry.slot {
                cx.drop_image(image, Some(window));
            }
        }
    }

    fn spawn_decode(&mut self, key: ImageKey, cx: &mut Context<Self>) -> Task<()> {
        let path = key.path.to_path_buf();
        let (width, height) = (key.width, key.height);
        let decode = cx
            .background_executor()
            .spawn(async move { decode(&path, width, height) });

        cx.spawn(async move |this, cx| {
            let result = decode.await;
            this.update(cx, |cache, cx| cache.finish(&key, result, cx))
                .ok();
        })
    }

    fn finish(
        &mut self,
        key: &ImageKey,
        result: anyhow::Result<RenderImage>,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.entries.get_mut(key) else {
            return;
        };

        entry.slot = match result {
            Ok(image) => {
                self.stats.decoded += 1;
                Slot::Ready(Arc::new(image))
            }
            Err(err) => {
                self.stats.failed += 1;
                log::warn!("image {} could not be decoded: {err:#}", key.path.display());
                Slot::Failed
            }
        };
        cx.notify();
    }
}

/// 解码并缩放到 `width`×`height`（物理像素），转成 GPUI 要的 BGRA。在后台线程上运行。
/// 解码按 core 的上限（图片可能来自同步或备份导入），超限返回错误。
pub fn decode(path: &Path, width: u32, height: u32) -> anyhow::Result<RenderImage> {
    // 目标尺寸由显示设置推出来，设置文件里的离谱数值不能变成一次超大的缩放分配。
    anyhow::ensure!(
        width > 0 && height > 0,
        "empty target size {width}x{height}"
    );
    kwikpaste_core::imaging::check_rgba_size(width, height)?;
    let image = kwikpaste_core::imaging::open(path)?.decode()?;
    let image = if image.width() == width && image.height() == height {
        image
    } else {
        image.resize_exact(width, height, image::imageops::FilterType::Triangle)
    };

    let mut pixels = image.into_rgba8();
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }

    Ok(RenderImage::new(vec![image::Frame::new(pixels)]))
}

/// 文件路径转缓存键里的 `Arc<Path>`。
pub fn path_of(text: &str) -> Arc<Path> {
    Arc::from(PathBuf::from(text).as_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_resizes_and_swaps_to_bgra() {
        let dir = std::env::temp_dir().join(format!("kp-image-cache-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("red.png");
        image::RgbaImage::from_pixel(40, 20, image::Rgba([255, 0, 0, 255]))
            .save(&path)
            .expect("write png");

        let image = decode(&path, 10, 5).expect("decodes");
        assert_eq!(image.size(0).width.0, 10);
        assert_eq!(image.size(0).height.0, 5);
        let bytes = image.as_bytes(0).expect("frame");
        assert_eq!(bytes.get(..4), Some(&[0, 0, 255, 255][..]), "BGRA order");

        assert!(decode(&dir.join("missing.png"), 10, 5).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
