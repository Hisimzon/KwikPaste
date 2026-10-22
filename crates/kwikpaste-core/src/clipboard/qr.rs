//! 图片里的二维码识别（大图预览的「识别二维码」）。

use std::path::Path;

use anyhow::Context as _;
use image::ImageReader;

/// 读出图片里全部二维码的内容；没有二维码（或都读不出）时为空。
pub fn decode_qr_codes(path: &Path) -> anyhow::Result<Vec<String>> {
    let image = ImageReader::open(path)
        .with_context(|| format!("open {}", path.display()))?
        .with_guessed_format()?
        .decode()?
        .into_luma8();
    let (width, height) = image.dimensions();
    let mut prepared =
        rqrr::PreparedImage::prepare_from_greyscale(width as usize, height as usize, |x, y| {
            image.get_pixel(x as u32, y as u32).0[0]
        });

    Ok(prepared
        .detect_grids()
        .into_iter()
        .filter_map(|grid| grid.decode().ok())
        .map(|(_, content)| content)
        .filter(|content| !content.is_empty())
        .collect())
}
