//! 剪贴板读取：经 [`ClipboardBackend`] 读取，产出带类型标记的 [`ClipboardPayload`]。
//!
//! 平台读取由后端负责（正式运行是 clipboard-rs 封装的 macOS `NSPasteboard` / Windows 剪贴板），
//! 这里只负责按用户设置的优先级归类，并过滤掉空内容。
//!
//! 图片读取走「PNG 直通优先」：源本身是 PNG 时直取原始字节、从头部解析尺寸（零解码/编码）；
//! 仅当源是 TIFF/DIB 等非 PNG 时才回退到库的解码 + 重编码 PNG。

use std::path::{Path, PathBuf};

use super::backend::{ClipboardBackend, ClipboardFormat, SystemClipboard};
use super::payload::{ClipboardPayload, ImagePayload, TextPayload};
use crate::error::Result;
use crate::presenter::is_image_path;
use crate::settings::{Capture, CaptureKind};

/// 持有一个剪贴板后端的读取器。轻量，可按需创建；监听线程会持有一个长生命周期实例复用。
pub struct ClipboardReader<B = SystemClipboard> {
    backend: B,
}

impl ClipboardReader<SystemClipboard> {
    /// 读取本机系统剪贴板。`!Send`，在读取所在的线程上创建。
    pub fn new() -> Result<Self> {
        Ok(Self::with_backend(SystemClipboard::new()?))
    }
}

impl<B: ClipboardBackend> ClipboardReader<B> {
    pub fn with_backend(backend: B) -> Self {
        Self { backend }
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    /// 读取当前剪贴板，按用户配置的内容类型顺序归类为单一载荷。
    ///
    /// 同一次复制可能同时存在文件、图片、HTML、RTF、纯文本表示；这里只返回第一个启用且可读取的类型。
    pub fn read_with_capture(&self, capture: &Capture) -> Result<Option<ClipboardPayload>> {
        let mut text_payload: Option<Option<TextPayload>> = None;

        for kind in capture.ordered_kinds() {
            if !capture.is_enabled(kind) {
                continue;
            }

            match kind {
                CaptureKind::Files => {
                    if let Some(files) = self.read_files()? {
                        if let Some(image) = self.app_cache_image(capture, &files) {
                            return Ok(Some(ClipboardPayload::Image(image)));
                        }
                        return Ok(Some(ClipboardPayload::Files(files)));
                    }
                }
                CaptureKind::Image => {
                    if self.backend.has(ClipboardFormat::Image) {
                        if let Some(image) = self.read_image()? {
                            return Ok(Some(ClipboardPayload::Image(image)));
                        }
                    }
                }
                CaptureKind::Html | CaptureKind::Rtf | CaptureKind::Text => {
                    if text_payload.is_none() {
                        text_payload = Some(self.read_text_payload()?);
                    }

                    let Some(text) = text_payload.as_ref().and_then(|payload| payload.as_ref())
                    else {
                        continue;
                    };

                    if text_contains_kind(text, kind) {
                        return Ok(Some(ClipboardPayload::Text(text.clone())));
                    }
                }
            }
        }

        Ok(None)
    }

    /// 读取当前剪贴板而不受采集设置影响，供一次性的全局纯文本粘贴使用。
    /// 文件优先于文本；图片即使和文本一起存在也会被忽略。
    pub fn read_current(&self) -> Result<Option<ClipboardPayload>> {
        if let Some(files) = self.read_files()? {
            return Ok(Some(ClipboardPayload::Files(files)));
        }
        Ok(self.read_text_payload()?.map(ClipboardPayload::Text))
    }

    /// 读取剪贴板文件路径列表；空列表视为无可用文件内容。
    fn read_files(&self) -> Result<Option<Vec<String>>> {
        if !self.backend.has(ClipboardFormat::Files) {
            return Ok(None);
        }

        let files: Vec<String> = self
            .backend
            .get_files()?
            .into_iter()
            .filter(|path| !path.is_empty())
            .collect();
        if files.is_empty() {
            return Ok(None);
        }

        Ok(Some(files))
    }

    /// 复制的「文件」其实是别的应用自己缓存的一张图、剪贴板上也带着这张图时，取图片载荷。
    ///
    /// QQ、微信等在 macOS 上复制聊天图片，会同时放图片数据和指向沙盒容器里缓存文件的 URL。
    /// 这个文件是对方应用的内部存储：读它会被系统的「其他 App 的数据」权限拦下，对方清缓存后也会消失。
    /// 用户复制的是图片，按图片收录。访达复制文件时放的图片是文件图标，所以只认应用私有目录里的文件。
    fn app_cache_image(&self, capture: &Capture, files: &[String]) -> Option<ImagePayload> {
        let [only] = files else {
            return None;
        };
        if !capture.is_enabled(CaptureKind::Image)
            || !is_app_cache_image(only, &app_cache_roots())
            || !self.backend.has(ClipboardFormat::Image)
        {
            return None;
        }

        self.read_image().ok().flatten()
    }

    /// 读取剪贴板中的文本族表示，包含纯文本、HTML 和 RTF。
    fn read_text_payload(&self) -> Result<Option<TextPayload>> {
        let has_text = self.backend.has(ClipboardFormat::Text);
        let has_html = self.backend.has(ClipboardFormat::Html);
        let has_rtf = self.backend.has(ClipboardFormat::Rtf);
        if !has_text && !has_html && !has_rtf {
            return Ok(None);
        }

        let text = if has_text {
            self.backend.get_text()?
        } else {
            String::new()
        };
        let html = read_optional(has_html, || self.backend.get_html());
        let rtf = read_optional(has_rtf, || self.backend.get_rich_text());

        if text.is_empty() && html.is_none() && rtf.is_none() {
            return Ok(None);
        }

        Ok(Some(TextPayload { text, html, rtf }))
    }

    /// 读取图片为 PNG 载荷。
    ///
    /// 快路径：剪贴板原生就带 PNG（浏览器复制图片等）时，直接取原始 PNG 字节，
    /// 宽高从 PNG 头（IHDR）解析——全程零图像解码/编码。
    ///
    /// 慢路径回退：源是 TIFF（macOS 截图/多数 App）/ DIB（Windows）等非 PNG 时，
    /// 交给后端解码再编码为 PNG（存储格式恒为 PNG，这次转码无法避免）。
    fn read_image(&self) -> Result<Option<ImagePayload>> {
        if let Ok(bytes) = self.backend.get_png() {
            if let Some((width, height)) = png_dimensions(&bytes) {
                return Ok(Some(ImagePayload {
                    bytes,
                    width,
                    height,
                }));
            }
            // 拿到了 PNG 格式字节但解析不出尺寸（异常/截断）：落到下方回退，由库重新解码。
        }

        let image = self.backend.get_image_as_png()?;
        if image.width == 0 || image.height == 0 {
            return Ok(None);
        }

        if image.png.is_empty() {
            return Ok(None);
        }

        Ok(Some(ImagePayload {
            bytes: image.png,
            width: image.width,
            height: image.height,
        }))
    }
}

/// 从 PNG 字节解析宽高（不解码像素）。
///
/// PNG 布局固定：8 字节签名 + 4 字节 IHDR 长度 + 4 字节 "IHDR" + 宽(大端 u32) + 高(大端 u32)。
/// 宽高位于偏移 16..24。校验签名与 IHDR 标记，任一不符返回 `None`（交回退路径处理）。
pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    if bytes.len() < 24 || bytes[..8] != SIGNATURE || &bytes[12..16] != b"IHDR" {
        return None;
    }
    let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]);
    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
}

/// `path` 是否是放在 `roots` 某个目录下的图片文件（按扩展名判断）。
fn is_app_cache_image(path: &str, roots: &[PathBuf]) -> bool {
    is_image_path(path) && roots.iter().any(|root| Path::new(path).starts_with(root))
}

/// 应用私有存储的根目录：沙盒容器、应用组容器、缓存和每用户临时目录。
/// iCloud 云盘（`Mobile Documents`）、`CloudStorage` 下的网盘同样在 `~/Library` 里，但那是用户自己的文件，不算。
#[cfg(target_os = "macos")]
fn app_cache_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from("/private/var/folders"),
        PathBuf::from("/var/folders"),
    ];
    if let Some(home) = dirs::home_dir() {
        let library = home.join("Library");
        roots.extend(["Containers", "Group Containers", "Caches"].map(|dir| library.join(dir)));
    }
    roots
}

/// Windows 上还没遇到这样复制图片的应用，文件照常按文件收录。
#[cfg(not(target_os = "macos"))]
fn app_cache_roots() -> Vec<PathBuf> {
    Vec::new()
}

/// 仅当 `available` 时读取，读取失败或空串都归并为 `None`，
/// 让「格式存在但内容为空」与「格式不存在」对下游表现一致。
fn read_optional(available: bool, read: impl FnOnce() -> Result<String>) -> Option<String> {
    if !available {
        return None;
    }
    read().ok().filter(|value| !value.is_empty())
}

/// 判断文本载荷中是否实际带有指定文本族表示。
fn text_contains_kind(text: &TextPayload, kind: CaptureKind) -> bool {
    let has_plain = !text.text.trim().is_empty();

    match kind {
        CaptureKind::Text => has_plain,
        CaptureKind::Html => {
            has_plain
                && text
                    .html
                    .as_ref()
                    .is_some_and(|html| !html.trim().is_empty())
        }
        CaptureKind::Rtf => {
            has_plain && text.rtf.as_ref().is_some_and(|rtf| !rtf.trim().is_empty())
        }
        CaptureKind::Files | CaptureKind::Image => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipboard::backend::{MemoryClipboard, MemoryState};

    /// 用 image crate 生成一张纯色 PNG，取其真实头部字节验证解析。
    fn sample_png(w: u32, h: u32) -> Vec<u8> {
        use std::io::Cursor;
        let buf = image::RgbaImage::from_pixel(w, h, image::Rgba([1, 2, 3, 255]));
        let mut out = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(buf)
            .write_to(&mut out, image::ImageFormat::Png)
            .unwrap();
        out.into_inner()
    }

    #[test]
    fn png_dimensions_reads_real_png_header() {
        assert_eq!(png_dimensions(&sample_png(123, 45)), Some((123, 45)));
    }

    #[test]
    fn png_dimensions_rejects_non_png_and_truncated() {
        // 非 PNG 签名。
        assert_eq!(png_dimensions(b"not a png at all really"), None);
        // 截断到 24 字节以下。
        assert_eq!(png_dimensions(&sample_png(8, 8)[..20]), None);
        // 空。
        assert_eq!(png_dimensions(&[]), None);
    }

    #[test]
    fn rich_text_without_plain_is_not_captureable_text() {
        let text = TextPayload {
            text: String::new(),
            html: Some("<b>only html</b>".to_owned()),
            rtf: Some(r"{\rtf1 only rtf}".to_owned()),
        };

        assert!(!text_contains_kind(&text, CaptureKind::Html));
        assert!(!text_contains_kind(&text, CaptureKind::Rtf));
        assert!(!text_contains_kind(&text, CaptureKind::Text));
    }

    fn read(state: MemoryState, capture: &Capture) -> Option<ClipboardPayload> {
        ClipboardReader::with_backend(MemoryClipboard::with_state(state))
            .read_with_capture(capture)
            .unwrap()
    }

    #[test]
    fn reads_text_with_rich_representations() {
        let payload = read(
            MemoryState {
                text: Some("hello kwikpaste".to_owned()),
                html: Some("<b>hello</b> kwikpaste".to_owned()),
                rtf: Some(String::new()),
                ..MemoryState::default()
            },
            &Capture::default(),
        )
        .expect("clipboard should contain text");

        assert_eq!(
            payload,
            ClipboardPayload::Text(TextPayload {
                text: "hello kwikpaste".to_owned(),
                html: Some("<b>hello</b> kwikpaste".to_owned()),
                // 存在但为空的表示与不存在等同。
                rtf: None,
            })
        );
    }

    #[test]
    fn files_win_over_text_in_default_order_and_follow_custom_order() {
        let state = MemoryState {
            text: Some("C:/a.txt".to_owned()),
            files: Some(vec!["C:/a.txt".to_owned(), String::new()]),
            ..MemoryState::default()
        };

        assert_eq!(
            read(state.clone(), &Capture::default()),
            Some(ClipboardPayload::Files(vec!["C:/a.txt".to_owned()]))
        );

        let text_first = Capture {
            order: vec![CaptureKind::Text, CaptureKind::Files],
            ..Capture::default()
        };
        assert!(matches!(
            read(state, &text_first),
            Some(ClipboardPayload::Text(_))
        ));
    }

    #[test]
    fn app_cache_image_matches_images_under_the_roots_only() {
        let roots = [PathBuf::from("/Users/me/Library/Containers")];
        let cached = "/Users/me/Library/Containers/com.tencent.qq/Data/Pic/ed8b.png";

        assert!(is_app_cache_image(cached, &roots));
        assert!(!is_app_cache_image(
            "/Users/me/Library/Containers/com.tencent.qq/Data/a.docx",
            &roots
        ));
        assert!(!is_app_cache_image("/Users/me/Desktop/ed8b.png", &roots));
        assert!(!is_app_cache_image(
            "/Users/me/Library/ContainersBackup/ed8b.png",
            &roots
        ));
        assert!(!is_app_cache_image(cached, &[]));
    }

    // QQ 复制聊天图片：沙盒容器里的缓存文件 + 图片数据，按图片收录；关掉图片收录时退回文件。
    #[cfg(target_os = "macos")]
    #[test]
    fn app_cache_file_with_image_data_is_read_as_image() {
        let cached = dirs::home_dir()
            .unwrap()
            .join("Library/Containers/com.tencent.qq/Data/Pic/ed8b.png")
            .to_string_lossy()
            .into_owned();
        let png = sample_png(6, 4);
        let state = MemoryState {
            files: Some(vec![cached.clone()]),
            png: Some(png.clone()),
            ..MemoryState::default()
        };

        assert_eq!(
            read(state.clone(), &Capture::default()),
            Some(ClipboardPayload::Image(ImagePayload {
                bytes: png,
                width: 6,
                height: 4,
            }))
        );

        let no_images = Capture {
            image: false,
            ..Capture::default()
        };
        assert_eq!(
            read(state, &no_images),
            Some(ClipboardPayload::Files(vec![cached]))
        );
    }

    // 访达复制文件也会带一张图（文件图标），用户目录里的文件照常按文件收录。
    #[test]
    fn user_file_with_image_data_stays_files() {
        let photo = dirs::home_dir()
            .unwrap()
            .join("Desktop/photo.png")
            .to_string_lossy()
            .into_owned();
        let state = MemoryState {
            files: Some(vec![photo.clone()]),
            png: Some(sample_png(6, 4)),
            ..MemoryState::default()
        };

        assert_eq!(
            read(state, &Capture::default()),
            Some(ClipboardPayload::Files(vec![photo]))
        );
    }

    #[test]
    fn png_is_passed_through_without_reencoding() {
        let png = sample_png(7, 5);
        let payload = read(
            MemoryState {
                png: Some(png.clone()),
                ..MemoryState::default()
            },
            &Capture::default(),
        );

        assert_eq!(
            payload,
            Some(ClipboardPayload::Image(ImagePayload {
                bytes: png,
                width: 7,
                height: 5,
            }))
        );
    }

    #[test]
    fn disabled_kinds_and_blank_text_are_not_read() {
        let state = MemoryState {
            text: Some("  ".to_owned()),
            png: Some(sample_png(2, 2)),
            ..MemoryState::default()
        };
        let no_images = Capture {
            image: false,
            ..Capture::default()
        };

        assert_eq!(read(state, &no_images), None);
        assert_eq!(read(MemoryState::default(), &Capture::default()), None);
    }
}
