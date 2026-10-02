//! 系统剪贴板的读写接口。
//!
//! [`ClipboardReader`](super::ClipboardReader) 与写回函数只通过 [`ClipboardBackend`] 碰剪贴板：
//! 正式运行用 [`SystemClipboard`]（clipboard-rs，Windows 写图另走 clipboard-win），
//! 自动测试一律用 [`MemoryClipboard`]，不读写本机真实剪贴板。
//!
//! 实现不要求 `Send`：clipboard-rs 的上下文是 `!Send`，调用方在同一线程的同步段里创建、用完、丢弃。

use std::sync::{Arc, Mutex};

use clipboard_rs::common::RustImage;
use clipboard_rs::{Clipboard, ClipboardContent, ClipboardContext, ContentFormat};

use super::read::png_dimensions;
use crate::error::{AppError, Result};

/// 读取前探测的内容类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardFormat {
    Text,
    Html,
    Rtf,
    Image,
    Files,
}

/// 一次写入里的一种文本表示；同一次写入的几种表示并存，不互相清空。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardWrite {
    Text(String),
    Html(String),
    Rtf(String),
}

/// 剪贴板上的图片解码后重新编码成的 PNG。宽或高为 0 时 `png` 为空。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
}

/// 读写剪贴板所需的最小操作集合，语义与 clipboard-rs 的同名操作一致：
/// 每次 `set*` 都先清空剪贴板，再写入这一次给出的全部表示。
pub trait ClipboardBackend {
    fn has(&self, format: ClipboardFormat) -> bool;
    fn get_text(&self) -> Result<String>;
    fn get_html(&self) -> Result<String>;
    fn get_rich_text(&self) -> Result<String>;
    fn get_files(&self) -> Result<Vec<String>>;
    /// 平台原生 PNG 表示的原始字节，不解码、不重新编码。
    fn get_png(&self) -> Result<Vec<u8>>;
    /// 把剪贴板上任意格式的图片（TIFF、DIB 等）解码后重新编码为 PNG。
    fn get_image_as_png(&self) -> Result<DecodedImage>;
    fn set_text(&self, text: String) -> Result<()>;
    fn set(&self, contents: Vec<ClipboardWrite>) -> Result<()>;
    fn set_files(&self, files: Vec<String>) -> Result<()>;
    /// 把 PNG 字节原样放上剪贴板；Windows 另附一份 `CF_DIB` 给只认位图的应用。
    fn set_png(&self, png: Vec<u8>) -> Result<()>;
}

/// 借用的后端照样能用，`ClipboardReader::with_backend(&backend)` 不必转移所有权。
impl<T: ClipboardBackend + ?Sized> ClipboardBackend for &T {
    fn has(&self, format: ClipboardFormat) -> bool {
        (**self).has(format)
    }

    fn get_text(&self) -> Result<String> {
        (**self).get_text()
    }

    fn get_html(&self) -> Result<String> {
        (**self).get_html()
    }

    fn get_rich_text(&self) -> Result<String> {
        (**self).get_rich_text()
    }

    fn get_files(&self) -> Result<Vec<String>> {
        (**self).get_files()
    }

    fn get_png(&self) -> Result<Vec<u8>> {
        (**self).get_png()
    }

    fn get_image_as_png(&self) -> Result<DecodedImage> {
        (**self).get_image_as_png()
    }

    fn set_text(&self, text: String) -> Result<()> {
        (**self).set_text(text)
    }

    fn set(&self, contents: Vec<ClipboardWrite>) -> Result<()> {
        (**self).set(contents)
    }

    fn set_files(&self, files: Vec<String>) -> Result<()> {
        (**self).set_files(files)
    }

    fn set_png(&self, png: Vec<u8>) -> Result<()> {
        (**self).set_png(png)
    }
}

/// 平台剪贴板里 PNG 的原始格式标识符：读取时 `get_buffer` 直取原始字节，写回时原样放回。
#[cfg(target_os = "macos")]
pub(super) const PNG_FORMAT: &str = "public.png";
#[cfg(target_os = "windows")]
pub(super) const PNG_FORMAT: &str = "PNG";

/// 本机系统剪贴板。`!Send`，在使用它的线程上创建。
pub struct SystemClipboard {
    ctx: ClipboardContext,
}

impl SystemClipboard {
    pub fn new() -> Result<Self> {
        let ctx = ClipboardContext::new().map_err(clip_err)?;
        Ok(Self { ctx })
    }
}

impl ClipboardBackend for SystemClipboard {
    fn has(&self, format: ClipboardFormat) -> bool {
        self.ctx.has(match format {
            ClipboardFormat::Text => ContentFormat::Text,
            ClipboardFormat::Html => ContentFormat::Html,
            ClipboardFormat::Rtf => ContentFormat::Rtf,
            ClipboardFormat::Image => ContentFormat::Image,
            ClipboardFormat::Files => ContentFormat::Files,
        })
    }

    fn get_text(&self) -> Result<String> {
        self.ctx.get_text().map_err(clip_err)
    }

    fn get_html(&self) -> Result<String> {
        self.ctx.get_html().map_err(clip_err)
    }

    fn get_rich_text(&self) -> Result<String> {
        self.ctx.get_rich_text().map_err(clip_err)
    }

    fn get_files(&self) -> Result<Vec<String>> {
        self.ctx.get_files().map_err(clip_err)
    }

    fn get_png(&self) -> Result<Vec<u8>> {
        self.ctx.get_buffer(PNG_FORMAT).map_err(clip_err)
    }

    fn get_image_as_png(&self) -> Result<DecodedImage> {
        let image = self.ctx.get_image().map_err(clip_err)?;
        let (width, height) = image.get_size();
        if width == 0 || height == 0 {
            return Ok(DecodedImage {
                width,
                height,
                png: Vec::new(),
            });
        }

        let png = image.to_png().map_err(clip_err)?.get_bytes().to_vec();
        Ok(DecodedImage { width, height, png })
    }

    fn set_text(&self, text: String) -> Result<()> {
        self.ctx.set_text(text).map_err(clip_err)
    }

    fn set(&self, contents: Vec<ClipboardWrite>) -> Result<()> {
        let contents = contents
            .into_iter()
            .map(|content| match content {
                ClipboardWrite::Text(text) => ClipboardContent::Text(text),
                ClipboardWrite::Html(html) => ClipboardContent::Html(html),
                ClipboardWrite::Rtf(rtf) => ClipboardContent::Rtf(rtf),
            })
            .collect();
        self.ctx.set(contents).map_err(clip_err)
    }

    fn set_files(&self, files: Vec<String>) -> Result<()> {
        self.ctx.set_files(files).map_err(clip_err)
    }

    #[cfg(target_os = "macos")]
    fn set_png(&self, png: Vec<u8>) -> Result<()> {
        self.ctx
            .set(vec![ClipboardContent::Other(PNG_FORMAT.to_owned(), png)])
            .map_err(clip_err)
    }

    /// 解码放在打开剪贴板之前，占用剪贴板只做两次拷贝；系统会按需从 `CF_DIB`
    /// 合成 `CF_BITMAP` / `CF_DIBV5`。
    #[cfg(target_os = "windows")]
    fn set_png(&self, png: Vec<u8>) -> Result<()> {
        let dib = windows_png::png_to_dib(&png)?;

        let _clipboard = windows_png::open_clipboard()?;
        clipboard_win::empty().map_err(clip_err)?;
        let png_format = clipboard_win::register_format(PNG_FORMAT)
            .ok_or_else(|| AppError::Clipboard("PNG clipboard format unavailable".to_owned()))?;
        clipboard_win::raw::set_without_clear(png_format.get(), &png).map_err(clip_err)?;
        clipboard_win::raw::set_without_clear(clipboard_win::formats::CF_DIB, &dib)
            .map_err(clip_err)?;
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod windows_png {
    use super::clip_err;
    use crate::error::Result;

    /// 打开剪贴板失败后的退避间隔。别的剪贴板监听程序可能正读着上一份（大图）内容，
    /// `new_attempts` 自带的重试只让出时间片、几微秒就耗尽，与读取侧一样给一段有界的等待。
    const OPEN_RETRY_DELAYS: [std::time::Duration; 3] = [
        std::time::Duration::from_millis(15),
        std::time::Duration::from_millis(35),
        std::time::Duration::from_millis(75),
    ];

    pub(super) fn open_clipboard() -> Result<clipboard_win::Clipboard> {
        let mut result = clipboard_win::Clipboard::new_attempts(10);
        for delay in OPEN_RETRY_DELAYS {
            if result.is_ok() {
                break;
            }
            std::thread::sleep(delay);
            result = clipboard_win::Clipboard::new_attempts(10);
        }
        result.map_err(clip_err)
    }

    /// `BITMAPINFOHEADER` 的字节长度。
    pub(super) const DIB_HEADER_LEN: usize = 40;

    /// 解码 PNG 为 `CF_DIB` 数据：`BITMAPINFOHEADER` + 自下而上的 32 位 BGRA 行（`BI_RGB`）。
    pub(super) fn png_to_dib(png: &[u8]) -> Result<Vec<u8>> {
        let rgba = image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .map_err(clip_err)?
            .into_rgba8();
        let (width, height) = rgba.dimensions();
        let row_len = width as usize * 4;
        let pixels_len = row_len * height as usize;

        let mut dib = Vec::with_capacity(DIB_HEADER_LEN + pixels_len);
        dib.extend_from_slice(&(DIB_HEADER_LEN as u32).to_le_bytes());
        dib.extend_from_slice(&(width as i32).to_le_bytes());
        // 正高度表示行自下而上存放。
        dib.extend_from_slice(&(height as i32).to_le_bytes());
        dib.extend_from_slice(&1u16.to_le_bytes());
        dib.extend_from_slice(&32u16.to_le_bytes());
        dib.extend_from_slice(&0u32.to_le_bytes());
        dib.extend_from_slice(&(pixels_len as u32).to_le_bytes());
        // 分辨率、调色板计数：全 0。
        dib.extend_from_slice(&[0; 16]);

        for row in rgba.as_raw().chunks_exact(row_len).rev() {
            for pixel in row.chunks_exact(4) {
                dib.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
            }
        }
        Ok(dib)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn png_to_dib_writes_bottom_up_bgra() {
            // 2×2：上排红、绿，下排蓝、半透明白。
            let image = image::RgbaImage::from_raw(
                2,
                2,
                vec![
                    255, 0, 0, 255, 0, 255, 0, 255, //
                    0, 0, 255, 255, 255, 255, 255, 128,
                ],
            )
            .unwrap();
            let mut png = Vec::new();
            image::DynamicImage::ImageRgba8(image)
                .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
                .unwrap();

            let dib = png_to_dib(&png).unwrap();

            assert_eq!(dib.len(), DIB_HEADER_LEN + 16);
            assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 40);
            assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 2);
            assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), 2);
            assert_eq!(u16::from_le_bytes(dib[14..16].try_into().unwrap()), 32);
            assert_eq!(
                &dib[DIB_HEADER_LEN..],
                &[
                    255, 0, 0, 255, 255, 255, 255, 128, // 下排：蓝、半透明白
                    0, 0, 255, 255, 0, 255, 0, 255, // 上排：红、绿
                ]
            );
        }
    }
}

/// 每次需要读写剪贴板时打开一个后端。`Core` 的高层读写都经它拿后端，测试换成 [`MemoryClipboard`]。
///
/// 返回的后端只在打开它的线程上用完即弃（系统剪贴板句柄是 `!Send`）。
pub trait ClipboardProvider: Send + Sync + 'static {
    fn open(&self) -> Result<Box<dyn ClipboardBackend>>;
}

/// 本机系统剪贴板的提供者。
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClipboardProvider;

impl ClipboardProvider for SystemClipboardProvider {
    fn open(&self) -> Result<Box<dyn ClipboardBackend>> {
        Ok(Box::new(SystemClipboard::new()?))
    }
}

/// 内存里的假剪贴板：自动测试和自测入口用它代替本机剪贴板。
///
/// 行为按真实剪贴板建模：每次写入先清空，再放入这次给出的表示；图片只保存 PNG 字节，
/// `get_png` 原样返回，`get_image_as_png` 从 PNG 头读出宽高。克隆出的实例共享同一份内容，
/// 所以它本身也是一个 [`ClipboardProvider`]。
#[derive(Debug, Default, Clone)]
pub struct MemoryClipboard {
    state: Arc<Mutex<MemoryState>>,
}

impl ClipboardProvider for MemoryClipboard {
    fn open(&self) -> Result<Box<dyn ClipboardBackend>> {
        Ok(Box::new(self.clone()))
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MemoryState {
    pub text: Option<String>,
    pub html: Option<String>,
    pub rtf: Option<String>,
    pub files: Option<Vec<String>>,
    pub png: Option<Vec<u8>>,
}

impl MemoryClipboard {
    pub fn new() -> Self {
        Self::default()
    }

    /// 用给定内容初始化，模拟别的应用刚复制了这些表示。
    pub fn with_state(state: MemoryState) -> Self {
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    /// 当前内容的快照，测试据此断言写回了什么。
    pub fn snapshot(&self) -> MemoryState {
        self.lock().clone()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MemoryState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn replace(&self, next: MemoryState) {
        *self.lock() = next;
    }
}

impl ClipboardBackend for MemoryClipboard {
    fn has(&self, format: ClipboardFormat) -> bool {
        let state = self.lock();
        match format {
            ClipboardFormat::Text => state.text.is_some(),
            ClipboardFormat::Html => state.html.is_some(),
            ClipboardFormat::Rtf => state.rtf.is_some(),
            ClipboardFormat::Image => state.png.is_some(),
            ClipboardFormat::Files => state.files.is_some(),
        }
    }

    fn get_text(&self) -> Result<String> {
        self.lock().text.clone().ok_or_else(|| missing("text"))
    }

    fn get_html(&self) -> Result<String> {
        self.lock().html.clone().ok_or_else(|| missing("html"))
    }

    fn get_rich_text(&self) -> Result<String> {
        self.lock().rtf.clone().ok_or_else(|| missing("rtf"))
    }

    fn get_files(&self) -> Result<Vec<String>> {
        self.lock().files.clone().ok_or_else(|| missing("files"))
    }

    fn get_png(&self) -> Result<Vec<u8>> {
        self.lock().png.clone().ok_or_else(|| missing("png"))
    }

    fn get_image_as_png(&self) -> Result<DecodedImage> {
        let png = self.get_png()?;
        let (width, height) = png_dimensions(&png).ok_or_else(|| missing("image"))?;
        Ok(DecodedImage { width, height, png })
    }

    fn set_text(&self, text: String) -> Result<()> {
        self.replace(MemoryState {
            text: Some(text),
            ..MemoryState::default()
        });
        Ok(())
    }

    fn set(&self, contents: Vec<ClipboardWrite>) -> Result<()> {
        let mut next = MemoryState::default();
        for content in contents {
            match content {
                ClipboardWrite::Text(text) => next.text = Some(text),
                ClipboardWrite::Html(html) => next.html = Some(html),
                ClipboardWrite::Rtf(rtf) => next.rtf = Some(rtf),
            }
        }
        self.replace(next);
        Ok(())
    }

    fn set_files(&self, files: Vec<String>) -> Result<()> {
        self.replace(MemoryState {
            files: Some(files),
            ..MemoryState::default()
        });
        Ok(())
    }

    fn set_png(&self, png: Vec<u8>) -> Result<()> {
        self.replace(MemoryState {
            png: Some(png),
            ..MemoryState::default()
        });
        Ok(())
    }
}

fn missing(format: &str) -> AppError {
    AppError::Clipboard(format!("clipboard has no {format} content"))
}

pub(super) fn clip_err<E: std::fmt::Display>(err: E) -> AppError {
    AppError::Clipboard(err.to_string())
}
