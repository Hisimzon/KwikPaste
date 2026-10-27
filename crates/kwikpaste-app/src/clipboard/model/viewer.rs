//! 大图预览的缩放、平移与解码尺寸。坐标都是图片舞台里的逻辑像素，原点在舞台左上角。
//!
//! 图片尺寸按“原图像素 ÷ 屏幕缩放”给出，所以倍数 1 就是一个原图像素对一个屏幕物理像素。

/// 最大放大倍数。
pub const MAX_SCALE: f32 = 8.;
/// 最小缩小倍数；比它还小的“适应窗口”倍数照常可用。
const MIN_SCALE: f32 = 0.1;
/// 适应窗口时四周留出的空白。
pub const FIT_MARGIN: f32 = 24.;
/// 按钮和 +/- 键依次经过的倍数。
const STEPS: [f32; 13] = [
    0.1, 0.25, 0.33, 0.5, 0.67, 0.75, 1., 1.5, 2., 3., 4., 6., 8.,
];
/// 解码位图最长边的上限（物理像素）：再大就由 GPU 放大，内存与纹理尺寸都有界。
const MAX_DECODE_SIDE: u32 = 8192;

/// 宽高。
pub type Size = (f32, f32);

/// 转了 `turns` 个直角之后的宽高。
pub fn rotated((width, height): Size, turns: u8) -> Size {
    if turns % 2 == 1 {
        (height, width)
    } else {
        (width, height)
    }
}

/// 放得下整张图的倍数；小图不放大。
pub fn fit_scale((width, height): Size, (stage_width, stage_height): Size) -> f32 {
    if width <= 0. || height <= 0. {
        return 1.;
    }
    let room = |stage: f32| (stage - FIT_MARGIN * 2.).max(1.);
    1_f32
        .min(room(stage_width) / width)
        .min(room(stage_height) / height)
}

/// 图片在舞台上的倍数和左上角位置。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub scale: f32,
    pub x: f32,
    pub y: f32,
    /// 处在“适应窗口”状态：舞台大小变了跟着重新适应。
    pub fitted: bool,
}

impl Camera {
    /// 适应窗口并居中。
    pub fn fit(image: Size, stage: Size) -> Self {
        let mut camera = Self {
            scale: fit_scale(image, stage),
            x: 0.,
            y: 0.,
            fitted: true,
        };
        camera.clamp(image, stage);
        camera
    }

    /// 缩放到 `scale`，`anchor` 处的图像素保持不动。
    pub fn zoom_at(&mut self, scale: f32, anchor: (f32, f32), image: Size, stage: Size) {
        let floor = MIN_SCALE.min(fit_scale(image, stage));
        let scale = scale.clamp(floor, MAX_SCALE);
        let ratio = scale / self.scale;
        self.x = anchor.0 - (anchor.0 - self.x) * ratio;
        self.y = anchor.1 - (anchor.1 - self.y) * ratio;
        self.scale = scale;
        self.fitted = false;
        self.clamp(image, stage);
    }

    /// 按档位放大（`up`）或缩小一级，以舞台中心为锚点。到头时不变。
    pub fn step(&mut self, up: bool, image: Size, stage: Size) {
        let current = self.scale;
        let next = if up {
            STEPS.iter().copied().find(|step| *step > current + 1e-3)
        } else {
            STEPS
                .iter()
                .rev()
                .copied()
                .find(|step| *step < current - 1e-3)
        };
        if let Some(next) = next {
            self.zoom_at(next, (stage.0 / 2., stage.1 / 2.), image, stage);
        }
    }

    pub fn pan(&mut self, dx: f32, dy: f32, image: Size, stage: Size) {
        self.x += dx;
        self.y += dy;
        self.clamp(image, stage);
    }

    /// 舞台尺寸变了：适应状态重新适应，否则只把图拉回可见范围。
    pub fn resize(&mut self, image: Size, stage: Size) {
        if self.fitted {
            *self = Self::fit(image, stage);
        } else {
            self.clamp(image, stage);
        }
    }

    /// 比舞台小的一边居中，比舞台大的一边不露出空白。
    fn clamp(&mut self, (width, height): Size, (stage_width, stage_height): Size) {
        let place = |offset: f32, extent: f32, stage: f32| {
            if extent <= stage {
                (stage - extent) / 2.
            } else {
                offset.clamp(stage - extent, 0.)
            }
        };
        self.x = place(self.x, width * self.scale, stage_width);
        self.y = place(self.y, height * self.scale, stage_height);
    }

    /// 图比舞台大，可以拖动。
    pub fn pannable(&self, (width, height): Size, (stage_width, stage_height): Size) -> bool {
        width * self.scale > stage_width + 0.5 || height * self.scale > stage_height + 0.5
    }
}

/// 以 `scale` 显示时需要的解码尺寸：不超过原图，最长边不超过上限。
pub fn decode_size((width, height): (u32, u32), scale: f32) -> (u32, u32) {
    let longest = width.max(height).max(1) as f32;
    let factor = scale.min(1.).min(MAX_DECODE_SIDE as f32 / longest);
    let side = |value: u32| ((value as f32 * factor).round() as u32).max(1);
    (side(width), side(height))
}

/// 已解码的位图还能不能用：不比需要的小（会糊），也不超过两倍（白占内存）。
pub fn decoded_fits(decoded: (u32, u32), needed: (u32, u32)) -> bool {
    decoded.0 >= needed.0 && decoded.0 <= needed.0.saturating_mul(2).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAGE: Size = (800., 600.);

    #[test]
    fn manual_zoom_keeps_physical_scale_across_device_changes() {
        let pixels = (1600., 1200.);
        let mut camera = Camera::fit(pixels, STAGE);
        camera.zoom_at(1., (400., 300.), pixels, STAGE);
        for device in [2., 1.] {
            let logical = (pixels.0 / device, pixels.1 / device);
            camera.resize(logical, STAGE);
            assert_eq!(camera.scale, 1.);
            assert_eq!(logical.0 * camera.scale * device, pixels.0);
            assert_eq!(logical.1 * camera.scale * device, pixels.1);
        }
    }

    #[test]
    fn fit_shrinks_large_images_and_centers_small_ones() {
        let large = Camera::fit((4000., 3000.), STAGE);
        assert!((large.scale - (552. / 3000.)).abs() < 1e-4);
        assert!(large.fitted);

        let small = Camera::fit((200., 100.), STAGE);
        assert_eq!(small.scale, 1.);
        assert_eq!((small.x, small.y), (300., 250.));
    }

    #[test]
    fn zoom_keeps_the_anchor_pixel_in_place() {
        let image = (4000., 3000.);
        let mut camera = Camera::fit(image, STAGE);
        let anchor = (500., 200.);
        let before = (
            (anchor.0 - camera.x) / camera.scale,
            (anchor.1 - camera.y) / camera.scale,
        );
        camera.zoom_at(1., anchor, image, STAGE);
        let after = (
            (anchor.0 - camera.x) / camera.scale,
            (anchor.1 - camera.y) / camera.scale,
        );
        assert!((before.0 - after.0).abs() < 1e-2 && (before.1 - after.1).abs() < 1e-2);
        assert!(!camera.fitted);
    }

    #[test]
    fn pan_stops_at_the_edges() {
        let image = (4000., 3000.);
        let mut camera = Camera::fit(image, STAGE);
        camera.zoom_at(1., (0., 0.), image, STAGE);
        camera.pan(500., 500., image, STAGE);
        assert_eq!((camera.x, camera.y), (0., 0.));
        camera.pan(-10_000., -10_000., image, STAGE);
        assert_eq!((camera.x, camera.y), (800. - 4000., 600. - 3000.));
    }

    #[test]
    fn steps_walk_the_ladder_and_stop_at_the_ends() {
        let image = (1000., 1000.);
        let mut camera = Camera::fit(image, STAGE);
        camera.zoom_at(1., (0., 0.), image, STAGE);
        camera.step(true, image, STAGE);
        assert_eq!(camera.scale, 1.5);
        camera.zoom_at(MAX_SCALE, (0., 0.), image, STAGE);
        camera.step(true, image, STAGE);
        assert_eq!(camera.scale, MAX_SCALE);
    }

    #[test]
    fn decode_size_never_exceeds_the_original_or_the_cap() {
        assert_eq!(decode_size((4000, 3000), 0.25), (1000, 750));
        assert_eq!(decode_size((4000, 3000), 4.), (4000, 3000));
        assert_eq!(decode_size((20_000, 10_000), 1.), (8192, 4096));
        assert!(decoded_fits((1000, 750), (900, 675)));
        assert!(!decoded_fits((1000, 750), (1200, 900)));
        assert!(!decoded_fits((4000, 3000), (1000, 750)));
    }

    #[test]
    fn rotation_swaps_sides_on_odd_turns() {
        assert_eq!(rotated((4., 3.), 1), (3., 4.));
        assert_eq!(rotated((4., 3.), 2), (4., 3.));
    }
}
