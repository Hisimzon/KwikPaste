//! 解码外来图片的统一上限：剪贴板上的图片、用户复制的图片文件、同步收到的和备份导入的原图。
//!
//! 解码器按图片头里的宽高一次性分配像素缓冲。头被篡改、或者图片大得离谱时，分配失败会直接终止进程
//! （release 是 `panic = "abort"`，分配失败本来也不会 unwind），分配成功则可能吃光内存。
//! 所以解码一律先按这里的上限核对宽高与缓冲大小，超了返回错误、不去分配。

use std::fs::File;
use std::io::{BufReader, Cursor};
use std::path::Path;

use image::error::{LimitError, LimitErrorKind};
use image::{DynamicImage, ImageDecoder, ImageError, ImageReader, ImageResult, Limits};

/// 宽、高各自的上限（像素）。GPU 纹理一般也只到 16384，再大的图显示不了。
pub const MAX_DIMENSION: u32 = 16_384;

/// 一次解码的像素缓冲上限。8K 截图（7680×4320 RGBA）约 127 MiB；缩略图并发最多 4 路，
/// 每路还有一份转换后的副本，峰值约 2 GiB。
pub const MAX_ALLOC: u64 = 256 * 1024 * 1024;

/// 解码上限：宽高与像素缓冲。
pub fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DIMENSION);
    limits.max_image_height = Some(MAX_DIMENSION);
    limits.max_alloc = Some(MAX_ALLOC);
    limits
}

/// 宽高为 `width`×`height` 的 RGBA 图是否在上限内。不解码、只看数字的路径（图片头、DIB 头）用它把关。
pub fn check_rgba_size(width: u32, height: u32) -> ImageResult<()> {
    let limits = limits();
    limits.check_dimensions(width, height)?;
    let bytes = u64::from(width) * u64::from(height) * 4;
    if bytes > MAX_ALLOC {
        return Err(ImageError::Limits(LimitError::from_kind(
            LimitErrorKind::InsufficientMemory,
        )));
    }
    Ok(())
}

/// 打开图片文件，按内容判断格式，装上解码上限。
pub fn open(path: &Path) -> ImageResult<ImageReader<BufReader<File>>> {
    let mut reader = ImageReader::open(path)?.with_guessed_format()?;
    reader.limits(limits());
    Ok(reader)
}

/// 内存里的图片字节，按内容判断格式，装上解码上限。
pub fn from_bytes(bytes: &[u8]) -> ImageResult<ImageReader<Cursor<&[u8]>>> {
    let mut reader = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?;
    reader.limits(limits());
    Ok(reader)
}

/// 按上限把解码器解成整图（需要先从解码器读 EXIF 方向等信息时用它，否则直接用读取器的 `decode`）。
/// 宽高或像素缓冲超限时返回错误。
pub fn decode(mut decoder: impl ImageDecoder) -> ImageResult<DynamicImage> {
    let mut limits = limits();
    limits.reserve(decoder.total_bytes())?;
    decoder.set_limits(limits)?;
    DynamicImage::from_decoder(decoder)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba([1, 2, 3, 255]));
        let mut png = Vec::new();
        DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png
    }

    /// 1×1 的真 PNG，把 IHDR 里的宽高改成任意值（CRC 跟着改）：图片头声称很大，像素数据只有一个点。
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut png = png(1, 1);
        png[16..20].copy_from_slice(&width.to_be_bytes());
        png[20..24].copy_from_slice(&height.to_be_bytes());
        let crc = crc32(&png[12..29]);
        png[29..33].copy_from_slice(&crc.to_be_bytes());
        png
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = 0xffff_ffff_u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn huge_dimensions_fail_before_allocating() {
        let err = from_bytes(&png_header(100_000, 100_000))
            .unwrap()
            .decode()
            .unwrap_err();
        assert!(matches!(err, ImageError::Limits(_)), "{err}");

        // 宽高都在上限内，但 RGBA 缓冲超过上限。
        let err = from_bytes(&png_header(16_000, 16_000))
            .unwrap()
            .decode()
            .unwrap_err();
        assert!(matches!(err, ImageError::Limits(_)), "{err}");
    }

    #[test]
    fn rgba_size_check_matches_the_limits() {
        assert!(check_rgba_size(7680, 4320).is_ok());
        assert!(check_rgba_size(MAX_DIMENSION, 4096).is_ok());
        assert!(check_rgba_size(MAX_DIMENSION + 1, 1).is_err());
        assert!(check_rgba_size(1, MAX_DIMENSION + 1).is_err());
        assert!(check_rgba_size(MAX_DIMENSION, MAX_DIMENSION).is_err());
    }

    #[test]
    fn normal_images_decode() {
        let png = png(40, 24);
        let decoded = from_bytes(&png).unwrap().decode().unwrap();
        assert_eq!((decoded.width(), decoded.height()), (40, 24));
        let decoded = decode(from_bytes(&png).unwrap().into_decoder().unwrap()).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (40, 24));
    }
}
