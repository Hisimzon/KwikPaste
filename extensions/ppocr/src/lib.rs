//! Windows CPU-only PP-OCRv6 extensions. stdout contains protocol frames only.
//! Pre/postprocessing follows PaddleX 3.7 text_detection/text_recognition processors
//! (Apache-2.0, PaddlePaddle Authors); no Python/OpenCV runtime dependency.
use anyhow::{Context, Result, ensure};
use clipper2::{EndType, JoinType, One, inflate};
use image::{GrayImage, Luma, Rgb, RgbImage};
use imageproc::{
    contours::find_contours, drawing::draw_polygon_mut, geometric_transformations::Projection,
    geometry::convex_hull, point::Point,
};
use kwikpaste_ext_protocol::{
    self as protocol, MAX_TEXT_CHARS, OcrSupport, Outcome, Request, Response, TextLine,
};
use ort::{
    environment::Environment,
    session::{Session, builder::GraphOptimizationLevel},
    value::Tensor,
};
use serde_yaml::Value;
use std::{io::Write, path::Path, sync::mpsc, time::Duration};

#[cfg(not(target_os = "windows"))]
compile_error!("PP-OCR extensions are Windows-only");

/// Static model/config bytes supplied by exactly one tier's binary entry point.
#[derive(Clone, Copy)]
pub struct Models {
    pub det: &'static [u8],
    pub rec: &'static [u8],
    pub det_config: &'static str,
    pub rec_config: &'static str,
}

type Quad = [(f64, f64); 4];

struct Engine {
    det: Session,
    rec: Session,
    dict: Vec<String>,
    thresh: f32,
    box_thresh: f64,
    unclip: f64,
    candidates: usize,
    mean: [f32; 3],
    std: [f32; 3],
    rec_h: usize,
    rec_w: usize,
    det_limit: f64,
    det_limit_max: bool,
    det_max_side: u32,
    _env: Environment,
}

impl Engine {
    /// Load the selected embedded model without a temporary file or Rust buffer copy.
    fn new(model: Models) -> Result<Self> {
        let det_cfg: Value = serde_yaml::from_str(model.det_config)?;
        let rec_cfg: Value = serde_yaml::from_str(model.rec_config)?;
        let post = &det_cfg["PostProcess"];
        let norm = &det_cfg["PreProcess"]["transform_ops"][3]["NormalizeImage"];
        let shape = &rec_cfg["PreProcess"]["transform_ops"][2]["RecResizeImg"]["image_shape"];
        let resize = &det_cfg["PreProcess"]["transform_ops"][2]["DetResizeForTest"];
        let limit_type = resize["limit_type"].as_str().unwrap_or("min");
        ensure!(
            matches!(limit_type, "min" | "max"),
            "unsupported detector limit type: {limit_type}"
        );
        let mut dict = vec![String::new()]; // CTC blank is 0, then YAML dictionary, then space.
        for ch in rec_cfg["PostProcess"]["character_dict"]
            .as_sequence()
            .context("dictionary")?
        {
            dict.push(ch.as_str().context("dictionary entry")?.to_owned());
        }
        dict.push(" ".into());
        let env = ort::init().with_name("ppocr-helper").build()?;
        let channel = |key: &str, i: usize| -> Result<f32> {
            Ok(norm[key][i]
                .as_f64()
                .context("invalid normalization channel")? as f32)
        };
        let session = |role: &str| -> Result<Session> {
            Ok(Session::builder(&env)?
                .with_intra_threads(4)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .with_inter_threads(1)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                // A short-lived background helper should not burn CPU spinning
                // in both independent session pools between det/rec invocations.
                .with_intra_op_spinning(false)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .with_inter_op_spinning(false)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                // Measured lower peak commit with arena/pattern off, without a material latency regression.
                .with_execution_providers([ort::ep::CPU::default()
                    .with_arena_allocator(false)
                    .build()])
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .with_memory_pattern(false)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .with_optimization_level(GraphOptimizationLevel::Level3)
                .map_err(|e| anyhow::anyhow!("{e}"))?
                .commit_from_memory(if role == "det" { model.det } else { model.rec })?)
        };
        Ok(Self {
            det: session("det")?,
            rec: session("rec")?,
            dict,
            thresh: post["thresh"].as_f64().context("thresh")? as f32,
            box_thresh: post["box_thresh"].as_f64().context("box_thresh")?,
            unclip: post["unclip_ratio"].as_f64().context("unclip")?,
            candidates: post["max_candidates"].as_u64().context("max_candidates")? as usize,
            mean: [
                channel("mean", 0)?,
                channel("mean", 1)?,
                channel("mean", 2)?,
            ],
            std: [channel("std", 0)?, channel("std", 1)?, channel("std", 2)?],
            rec_h: shape[1].as_u64().context("rec height")? as usize,
            rec_w: shape[2].as_u64().context("rec width")? as usize,
            det_limit: resize["limit_side_len"].as_f64().unwrap_or(736.0),
            det_limit_max: limit_type == "max",
            det_max_side: resize["max_side_limit"].as_u64().unwrap_or(4000) as u32,
            _env: env,
        })
    }

    /// Detect, crop, recognize and join horizontal text in reading order.
    fn recognize(&mut self, path: &Path) -> Result<Outcome> {
        let reader = image::ImageReader::open(path)?.with_guessed_format()?;
        let (w, h) = reader.into_dimensions()?;
        if u64::from(w) * u64::from(h) > 40_000_000 {
            return Ok(Outcome::Skipped {
                reason: "image exceeds 40 MP".into(),
            });
        }
        ensure!(w > 0 && h > 0, "empty image");
        let image = image::open(path)?.to_rgb8();
        // DetResizeForTest's inference.yml-null defaults: minimum side 736,
        // maximum side 4000; Python round() is ties-to-even.
        let (dw, dh) = det_size(w, h, self.det_limit, self.det_limit_max, self.det_max_side);
        let resized = resize_linear(&image, dw, dh);
        let mut input = vec![0f32; 3 * dw as usize * dh as usize];
        let plane = (dw * dh) as usize;
        for (i, pixel) in resized.pixels().enumerate() {
            for c in 0..3 {
                input[c * plane + i] =
                    (pixel[2 - c] as f32 * (1.0 / 255.0) - self.mean[c]) / self.std[c];
            }
        }
        let tensor = Tensor::from_array(([1, 3, dh as usize, dw as usize], input))?;
        let output = self.det.run(ort::inputs![tensor])?;
        let (shape, map) = output[0].try_extract_tensor::<f32>()?;
        ensure!(
            shape.len() == 4 && shape[0] == 1 && shape[1] == 1,
            "det shape: {shape:?}"
        );
        let mh = shape[2] as u32;
        let mw = shape[3] as u32;
        let mut boxes = db_boxes(
            map,
            mw,
            mh,
            w,
            h,
            self.thresh,
            self.box_thresh,
            self.unclip,
            self.candidates,
        );
        drop(output);
        // SortQuadBoxes: top-to-bottom, then left-to-right within 10 px.
        boxes.sort_by(|a, b| a[0].1.total_cmp(&b[0].1).then(a[0].0.total_cmp(&b[0].0)));
        for i in 0..boxes.len().saturating_sub(1) {
            for j in (0..=i).rev() {
                if (boxes[j + 1][0].1 - boxes[j][0].1).abs() < 10.0
                    && boxes[j + 1][0].0 < boxes[j][0].0
                {
                    boxes.swap(j, j + 1);
                } else {
                    break;
                }
            }
        }
        let mut lines = Vec::new();
        let mut chars = 0;
        for q in boxes {
            let (crop, reading_quad) = crop_quad(&image, q)?;
            let height = self.rec_h;
            let width = ((height as f64 * (crop.width() as f64 / crop.height() as f64))
                .max(self.rec_w as f64) as usize)
                .min(3200);
            let scaled_w = ((height as f64 * crop.width() as f64 / crop.height() as f64).ceil()
                as usize)
                .min(width);
            let resized = resize_linear(&crop, scaled_w as u32, height as u32);
            let mut data = vec![0f32; 3 * height * width];
            for y in 0..height {
                for x in 0..scaled_w {
                    let pixel = resized.get_pixel(x as u32, y as u32);
                    for c in 0..3 {
                        data[c * height * width + y * width + x] =
                            (pixel[2 - c] as f32 / 255.0 - 0.5) / 0.5;
                    }
                }
            }
            let tensor = Tensor::from_array(([1, 3, height, width], data))?;
            let output = self.rec.run(ort::inputs![tensor])?;
            let (shape, probs) = output[0].try_extract_tensor::<f32>()?;
            ensure!(
                shape.len() == 3 && shape[0] == 1 && shape[2] as usize == self.dict.len(),
                "rec shape {shape:?}, dict {}",
                self.dict.len()
            );
            let (text, score) = ctc_decode(probs, self.dict.len(), &self.dict);
            if std::env::var_os("PPOCR_TRACE").is_some() {
                eprintln!("{}", serde_json::json!({"box":q,"text":text,"score":score}));
            }
            if !text.is_empty() {
                let separator = usize::from(!lines.is_empty());
                let remaining = MAX_TEXT_CHARS.saturating_sub(chars + separator);
                if remaining == 0 {
                    break;
                }
                let text: String = text.chars().take(remaining).collect();
                chars += text.chars().count() + separator;
                lines.push(TextLine {
                    text,
                    quad: reading_quad.map(|(x, y)| {
                        [
                            (x / w as f64).clamp(0.0, 1.0) as f32,
                            (y / h as f64).clamp(0.0, 1.0) as f32,
                        ]
                    }),
                });
                if chars >= MAX_TEXT_CHARS {
                    break;
                }
            }
        }
        Ok(Outcome::Done {
            lines,
            language: "zh-Hans".into(),
        })
    }
}

/// Apply the model detector resize limits and ties-to-even stride rounding.
fn det_size(w: u32, h: u32, limit: f64, limit_max: bool, max_side: u32) -> (u32, u32) {
    let ratio = if limit_max {
        (limit / w.max(h) as f64).min(1.0)
    } else {
        (limit / w.min(h) as f64).max(1.0)
    };
    let mut rh = (h as f64 * ratio) as u32;
    let mut rw = (w as f64 * ratio) as u32;
    if rh.max(rw) > max_side {
        let r = max_side as f64 / rh.max(rw) as f64;
        rh = (rh as f64 * r) as u32;
        rw = (rw as f64 * r) as u32;
    }
    (
        ((rw as f64 / 32.0).round_ties_even() as u32 * 32).max(32),
        ((rh as f64 / 32.0).round_ties_even() as u32 * 32).max(32),
    )
}

/// Half-pixel bilinear resize, channel order retained. Rounding differences from
/// OpenCV's fixed-point SIMD implementation are covered by end-to-end parity.
fn resize_linear(src: &RgbImage, w: u32, h: u32) -> RgbImage {
    if src.width() == w && src.height() == h {
        return src.clone();
    }
    RgbImage::from_fn(w, h, |x, y| {
        let sx = ((x as f64 + 0.5) * src.width() as f64 / w as f64 - 0.5)
            .clamp(0.0, (src.width() - 1) as f64);
        let sy = ((y as f64 + 0.5) * src.height() as f64 / h as f64 - 0.5)
            .clamp(0.0, (src.height() - 1) as f64);
        let ix = sx.floor() as u32;
        let iy = sy.floor() as u32;
        let a = sx - ix as f64;
        let b = sy - iy as f64;
        let p = [
            src.get_pixel(ix, iy),
            src.get_pixel((ix + 1).min(src.width() - 1), iy),
            src.get_pixel(ix, (iy + 1).min(src.height() - 1)),
            src.get_pixel(
                (ix + 1).min(src.width() - 1),
                (iy + 1).min(src.height() - 1),
            ),
        ];
        Rgb(std::array::from_fn(|c| {
            ((1. - b) * ((1. - a) * p[0][c] as f64 + a * p[1][c] as f64)
                + b * ((1. - a) * p[2][c] as f64 + a * p[3][c] as f64))
                .round()
                .clamp(0., 255.) as u8
        }))
    })
}

/// Find the minimum-area rotated box around the integer contour.
fn min_box(points: &[Point<i32>]) -> (Quad, f64) {
    let hull = convex_hull(points);
    let mut best_area = f64::INFINITY;
    let mut best = [(0., 0.); 4];
    let mut short = 0.;
    for i in 0..hull.len() {
        let a = hull[i];
        let b = hull[(i + 1) % hull.len()];
        let dx = (b.x - a.x) as f64;
        let dy = (b.y - a.y) as f64;
        let len = dx.hypot(dy);
        if len == 0. {
            continue;
        }
        let (u, v) = (dx / len, dy / len);
        let (mut xmin, mut xmax, mut ymin, mut ymax) = (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        );
        for p in &hull {
            let x = p.x as f64 * u + p.y as f64 * v;
            let y = -(p.x as f64) * v + p.y as f64 * u;
            xmin = xmin.min(x);
            xmax = xmax.max(x);
            ymin = ymin.min(y);
            ymax = ymax.max(y);
        }
        let area = (xmax - xmin) * (ymax - ymin);
        if area < best_area - 1e-6 {
            best_area = area;
            short = (xmax - xmin).min(ymax - ymin);
            best = [(xmin, ymin), (xmax, ymin), (xmax, ymax), (xmin, ymax)]
                .map(|(x, y)| (x * u - y * v, x * v + y * u));
        }
    }
    best.sort_by(|a, b| a.0.total_cmp(&b.0));
    if best[0].1 > best[1].1 {
        best.swap(0, 1);
    }
    if best[2].1 > best[3].1 {
        best.swap(2, 3);
    }
    ([best[0], best[2], best[3], best[1]], short)
}

#[allow(clippy::too_many_arguments)]
/// Threshold, score and unclip DB contours into source-image quadrilaterals.
fn db_boxes(
    map: &[f32],
    mw: u32,
    mh: u32,
    w: u32,
    h: u32,
    thresh: f32,
    box_thresh: f64,
    unclip: f64,
    candidates: usize,
) -> Vec<Quad> {
    let bitmap = GrayImage::from_fn(mw, mh, |x, y| {
        Luma([if map[(y * mw + x) as usize] > thresh {
            255
        } else {
            0
        }])
    });
    let contours = find_contours::<i32>(&bitmap);
    let mut boxes = Vec::new();
    for contour in contours.iter().rev().take(candidates) {
        if contour.points.len() < 3 {
            continue;
        }
        let (q, short) = min_box(&contour.points);
        if short < 3. {
            continue;
        }
        let xmin = q
            .iter()
            .map(|p| p.0.floor() as i32)
            .min()
            .unwrap()
            .clamp(0, mw as i32 - 1);
        let xmax = q
            .iter()
            .map(|p| p.0.ceil() as i32)
            .max()
            .unwrap()
            .clamp(0, mw as i32 - 1);
        let ymin = q
            .iter()
            .map(|p| p.1.floor() as i32)
            .min()
            .unwrap()
            .clamp(0, mh as i32 - 1);
        let ymax = q
            .iter()
            .map(|p| p.1.ceil() as i32)
            .max()
            .unwrap()
            .clamp(0, mh as i32 - 1);
        let mut mask = GrayImage::new((xmax - xmin + 1) as u32, (ymax - ymin + 1) as u32);
        let pts = q.map(|(x, y)| Point::new((x - xmin as f64) as i32, (y - ymin as f64) as i32));
        if pts[0] == pts[3] {
            continue;
        }
        draw_polygon_mut(&mut mask, &pts, Luma([1]));
        let (mut sum, mut n) = (0f64, 0usize);
        for (x, y, p) in mask.enumerate_pixels() {
            if p[0] > 0 {
                sum += map[((y + ymin as u32) * mw + x + xmin as u32) as usize] as f64;
                n += 1;
            }
        }
        if n == 0 || sum / (n as f64) < box_thresh {
            continue;
        }
        let area = (0..4)
            .map(|i| q[i].0 * q[(i + 1) % 4].1 - q[(i + 1) % 4].0 * q[i].1)
            .sum::<f64>()
            .abs()
            / 2.;
        let perimeter = (0..4)
            .map(|i| (q[i].0 - q[(i + 1) % 4].0).hypot(q[i].1 - q[(i + 1) % 4].1))
            .sum::<f64>();
        if perimeter <= 0. {
            continue;
        }
        // pyclipper truncates input floating coordinates to integer pixels.
        let path: Vec<_> = q
            .iter()
            .map(|&(x, y)| (x as i64 as f64, y as i64 as f64))
            .collect();
        let expanded: Vec<Vec<(f64, f64)>> = inflate::<One>(
            path,
            area * unclip / perimeter,
            JoinType::Round,
            EndType::Polygon,
            2.0,
        )
        .into();
        let points: Vec<_> = expanded
            .into_iter()
            .flatten()
            .map(|(x, y)| Point::new(x as i32, y as i32))
            .collect();
        if points.len() < 3 {
            continue;
        }
        let (mut q, short) = min_box(&points);
        if short < 5. {
            continue;
        }
        for p in &mut q {
            p.0 = (p.0 * w as f64 / mw as f64)
                .round_ties_even()
                .clamp(0., (w - 1) as f64);
            p.1 = (p.1 * h as f64 / mh as f64)
                .round_ties_even()
                .clamp(0., (h - 1) as f64);
        }
        // PaddleX FilterBoxes removes boxes narrower/shorter than 3 px.
        if dist(q[0], q[1]) as i32 <= 3 || dist(q[0], q[3]) as i32 <= 3 {
            continue;
        }
        boxes.push(q);
    }
    boxes
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}
fn cubic(x: f64) -> f64 {
    let x = x.abs();
    let a = -0.75;
    if x <= 1. {
        (a + 2.) * x * x * x - (a + 3.) * x * x + 1.
    } else if x < 2. {
        a * x * x * x - 5. * a * x * x + 8. * a * x - 4. * a
    } else {
        0.
    }
}
/// 透视裁剪并返回与裁剪阅读方向一致的四角；竖排旋转时同步调整角点顺序。
fn crop_quad(src: &RgbImage, q: Quad) -> Result<(RgbImage, Quad)> {
    let (q, _) = min_box(&q.map(|(x, y)| Point::new(x as i32, y as i32)));
    let w = dist(q[0], q[1]).max(dist(q[2], q[3])) as u32;
    let h = dist(q[0], q[3]).max(dist(q[1], q[2])) as u32;
    ensure!(w > 0 && h > 0, "empty crop");
    let source = q.map(|(x, y)| (x as f32, y as f32));
    let dest = [
        (0., 0.),
        (w as f32, 0.),
        (w as f32, h as f32),
        (0., h as f32),
    ];
    let inverse =
        Projection::from_control_points(dest, source).context("singular perspective transform")?;
    let image = RgbImage::from_fn(w, h, |x, y| {
        let (sx, sy) = inverse * (x as f32, y as f32);
        // OpenCV INTER_CUBIC interpolation-table resolution is 1/32 pixel.
        let sx = (sx as f64 * 32.).round_ties_even() / 32.;
        let sy = (sy as f64 * 32.).round_ties_even() / 32.;
        let ix = sx.floor() as i32;
        let iy = sy.floor() as i32;
        let mut rgb = [0f64; 3];
        for yy in -1..=2 {
            for xx in -1..=2 {
                let weight = cubic(sx - (ix + xx) as f64) * cubic(sy - (iy + yy) as f64);
                let p = src.get_pixel(
                    (ix + xx).clamp(0, src.width() as i32 - 1) as u32,
                    (iy + yy).clamp(0, src.height() as i32 - 1) as u32,
                );
                for c in 0..3 {
                    rgb[c] += p[c] as f64 * weight;
                }
            }
        }
        Rgb(rgb.map(|v| v.round().clamp(0., 255.) as u8))
    });
    Ok(if h as f64 / w as f64 >= 1.5 {
        // 逆时针旋转后，原右上角成为阅读起点，原右下角成为终点。
        (image::imageops::rotate270(&image), [q[1], q[2], q[3], q[0]])
    } else {
        (image, q)
    })
}

/// Greedily decode CTC predictions, collapsing repeated labels across nonblanks.
fn ctc_decode(probs: &[f32], classes: usize, dict: &[String]) -> (String, f64) {
    let mut last = usize::MAX;
    let mut text = String::new();
    let mut sum = 0.;
    let mut n = 0;
    for row in probs.chunks_exact(classes) {
        let mut best = 0;
        for i in 1..row.len() {
            if row[i] > row[best] {
                best = i;
            }
        }
        if best != 0 && best != last {
            text.push_str(&dict[best]);
            sum += row[best] as f64;
            n += 1;
        }
        last = best;
    }
    (text, if n > 0 { sum / n as f64 } else { 0. })
}

/// Run one embedded tier with the existing bounded OCR protocol and idle lifetime.
pub fn run(model: Models) -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("--licenses") {
        std::io::stdout().write_all(
            concat!(
                include_str!("../licenses/onnxruntime-LICENSE"),
                "\n",
                include_str!("../licenses/onnxruntime-ThirdPartyNotices.txt"),
                "\n",
                include_str!("../licenses/PaddleOCR-LICENSE"),
            )
            .as_bytes(),
        )?;
        return Ok(());
    }
    unsafe {
        use windows_sys::Win32::System::Threading::*;
        SetPriorityClass(GetCurrentProcess(), BELOW_NORMAL_PRIORITY_CLASS);
        SetPriorityClass(GetCurrentProcess(), PROCESS_MODE_BACKGROUND_BEGIN);
    }
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("ocr-input".into())
        .spawn(move || {
            let mut reader = std::io::stdin();
            let mut buffer = Vec::new();
            loop {
                let request = protocol::read_frame::<Request>(&mut reader, &mut buffer);
                let done = !matches!(request, Ok(Some(_)));
                if sender.send(request).is_err() || done {
                    break;
                }
            }
        })?;
    let mut engine = Engine::new(model);
    if let Err(e) = &engine {
        eprintln!("engine initialization: {e:#}");
    }
    let mut writer = std::io::stdout().lock();
    let mut buffer = Vec::new();
    loop {
        let request = match receiver.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(Some(request))) => request,
            Ok(Err(e)) => return Err(e.into()),
            _ => break,
        };
        let response = match request {
            Request::Probe => Response::Probe {
                support: if engine.is_ok() {
                    OcrSupport::Available {
                        languages: vec!["zh-Hans".into(), "en".into(), "ja".into()],
                    }
                } else {
                    OcrSupport::Unsupported
                },
            },
            Request::Recognize { job_id, path } => {
                let outcome =
                    match &mut engine {
                        Ok(engine) => engine.recognize(Path::new(&path)).unwrap_or_else(|err| {
                            Outcome::Failed {
                                reason: format!("{err:#}"),
                            }
                        }),
                        Err(err) => Outcome::Failed {
                            reason: format!("{err:#}"),
                        },
                    };
                Response::Recognize { job_id, outcome }
            }
        };
        protocol::write_frame(&mut writer, &response, &mut buffer)?;
    }
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn crop_reading_frame_follows_vertical_rotation() {
        let source = RgbImage::new(40, 40);
        let horizontal = [(2., 3.), (22., 3.), (22., 13.), (2., 13.)];
        let (crop, quad) = crop_quad(&source, horizontal).unwrap();
        assert_eq!(crop.dimensions(), (20, 10));
        assert_eq!(quad, horizontal);
        let vertical = [(2., 3.), (12., 3.), (12., 33.), (2., 33.)];
        let (crop, quad) = crop_quad(&source, vertical).unwrap();
        assert_eq!(crop.dimensions(), (30, 10));
        assert_eq!(quad, [vertical[1], vertical[2], vertical[3], vertical[0]]);
    }
    #[test]
    fn ctc_blank_repeat() {
        let dict = vec!["".into(), "a".into(), "中".into()];
        let p = [0., 1., 0., 0., 1., 0., 1., 0., 0., 0., 1., 0., 0., 0., 1.];
        assert_eq!(ctc_decode(&p, 3, &dict).0, "aa中");
    }
    #[test]
    fn shape_limits() {
        assert_eq!(det_size(860, 220, 736.0, false, 4000), (2880, 736));
        for (w, h) in [(560, 240), (10000, 200), (200, 10000), (1, 1)] {
            let (w, h) = det_size(w, h, 736.0, false, 4000);
            assert!(w >= 32 && h >= 32 && w <= 4000 && h <= 4000 && w % 32 == 0 && h % 32 == 0);
        }
    }
    #[test]
    fn rectangle() {
        let (q, s) = min_box(&[
            Point::new(2, 3),
            Point::new(22, 3),
            Point::new(22, 13),
            Point::new(2, 13),
        ]);
        assert_eq!(q, [(2., 3.), (22., 3.), (22., 13.), (2., 13.)]);
        assert_eq!(s, 10.);
    }
}
