//! 自测：往平台自测进程自己的 core（开发数据目录 `…selftest-platform\dev`）灌合成记录，内存验收（H1）用。
//!
//! `--selftest-seed=<n>`：按 core 的采集流程（`build_item` → `store_item`）存 n 条记录，最旧的先存：
//! 每 9 条里 1 条图片（1280×720、1920×1080、2560×1440 轮换，其中 6 张是 3840×2160 的 4K），每 50 条里
//! 1 条文件，其余是文本（纯文本、长文本、HTML、RTF、链接、中文）。图片各不相同（渐变加噪点块加序号
//! 条纹），PNG 体积接近真实截图。

use std::io::Cursor;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::Context as _;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder as _};
use kwikpaste_core::Core;
use kwikpaste_core::clipboard::{ClipboardPayload, ImagePayload, TextPayload};

/// 灌好之后的统计。
pub struct Seeded {
    pub records: usize,
    pub images: usize,
    pub four_k: usize,
    pub image_bytes: usize,
    pub elapsed_ms: u128,
}

const SIZES: [(u32, u32); 3] = [(1280, 720), (1920, 1080), (2560, 1440)];
const FOUR_K: (u32, u32) = (3840, 2160);
const FOUR_K_COUNT: usize = 6;

/// 灌 `count` 条记录。生成图片和写盘都在调用方给的后台执行器上做。
pub async fn seed(core: Core, count: usize) -> anyhow::Result<Seeded> {
    let started = Instant::now();
    let files = seed_files()?;
    let mut seeded = Seeded {
        records: 0,
        images: 0,
        four_k: 0,
        image_bytes: 0,
        elapsed_ms: 0,
    };

    for index in (0..count).rev() {
        let payload = if index % 9 == 4 {
            let image_index = index / 9;
            let (width, height) = if image_index < FOUR_K_COUNT {
                seeded.four_k += 1;
                FOUR_K
            } else {
                SIZES[image_index % SIZES.len()]
            };
            let bytes = png(width, height, index as u32)?;
            seeded.images += 1;
            seeded.image_bytes += bytes.len();
            ClipboardPayload::Image(ImagePayload {
                bytes,
                width,
                height,
            })
        } else if index % 50 == 7 {
            let take = 1 + index % files.len();
            ClipboardPayload::Files(files.iter().take(take).cloned().collect())
        } else {
            ClipboardPayload::Text(text(index))
        };
        if let Some(item) = core.build_item(&payload)? {
            core.store_item(item, None).await?;
            seeded.records += 1;
        }
    }
    seeded.elapsed_ms = started.elapsed().as_millis();

    Ok(seeded)
}

fn text(index: usize) -> TextPayload {
    let plain = match index % 7 {
        0 => format!("https://example.com/kwikpaste/seed/{index}?ref=memory"),
        1 => {
            format!("快贴内存验收第 {index} 条：中文内容，包含标点、数字 {index} 和一些常见词语。")
        }
        2 => format!("seed {index}: ").repeat(40 + index % 60),
        _ => format!("kwikpaste memory seed record {index} with a short line of text"),
    };
    let (html, rtf) = match index % 11 {
        3 => (
            Some(format!("<p><b>seed {index}</b> <i>html</i> record</p>")),
            None,
        ),
        5 => (
            None,
            Some(format!("{{\\rtf1\\ansi {{\\b seed {index}}} rtf record}}")),
        ),
        _ => (None, None),
    };

    TextPayload {
        text: plain,
        html,
        rtf,
    }
}

/// 一张各不相同的 RGB 图：横竖渐变，按序号散布的噪点块，再加一条序号条纹。
fn png(width: u32, height: u32, seed: u32) -> anyhow::Result<Vec<u8>> {
    let mut pixels = vec![0u8; width as usize * height as usize * 3];
    for y in 0..height {
        for x in 0..width {
            let offset = (y as usize * width as usize + x as usize) * 3;
            let block = mix(u64::from((x / 48) ^ ((y / 48) << 8) ^ (seed << 16)));
            let (r, g, b) = if block.is_multiple_of(5) {
                let noise = mix(block ^ u64::from(x) ^ (u64::from(y) << 20));
                (noise as u8, (noise >> 8) as u8, (noise >> 16) as u8)
            } else {
                (
                    (x * 255 / width) as u8,
                    (y * 255 / height) as u8,
                    (seed.wrapping_mul(37) % 256) as u8,
                )
            };
            pixels[offset] = r;
            pixels[offset + 1] = g;
            pixels[offset + 2] = b;
        }
    }
    // 序号条纹：保证每张图的内容（和去重指纹）不同。
    for bit in 0..16 {
        let on = seed >> bit & 1 == 1;
        for y in 0..8.min(height) {
            for x in bit * 16..(bit * 16 + 16).min(width) {
                let offset = (y as usize * width as usize + x as usize) * 3;
                pixels[offset..offset + 3].fill(if on { 255 } else { 0 });
            }
        }
    }

    let mut bytes = Vec::new();
    PngEncoder::new_with_quality(
        Cursor::new(&mut bytes),
        CompressionType::Fast,
        FilterType::Adaptive,
    )
    .write_image(&pixels, width, height, ExtendedColorType::Rgb8)
    .context("seed image encode")?;

    Ok(bytes)
}

/// 文件记录指向的几个真实文件（放在临时目录里）。
fn seed_files() -> anyhow::Result<Vec<String>> {
    let dir: PathBuf = std::env::temp_dir().join("kwikpaste-seed-files");
    std::fs::create_dir_all(&dir)?;
    let mut paths = Vec::new();
    for name in [
        "notes.txt",
        "report.pdf",
        "draft.docx",
        "sheet.xlsx",
        "archive.zip",
    ] {
        let path = dir.join(name);
        if !path.exists() {
            std::fs::write(&path, name)?;
        }
        paths.push(path.to_string_lossy().into_owned());
    }

    Ok(paths)
}

fn mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    x = x.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    x ^ (x >> 33)
}
