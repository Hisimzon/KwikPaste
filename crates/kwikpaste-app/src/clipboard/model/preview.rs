//! 预览窗的模型（1.x `window/preview.rs` 的几何与 `pages/Preview` 的排版规则）：面板尺寸按内容度量算，
//! 放在卡片的左右（按指针在主窗口的哪一半）或上下，夹在显示器内；文本按面板宽度和正文字体折成行；
//! 选词按起点到当前词连续勾选或取消。除了用文字系统量宽的几个函数，都是纯函数，可以 headless 单测。

use std::{collections::BTreeSet, ops::Range};

use gpui::{App, FontWeight, TextRun, Window, font, px};
use kwikpaste_core::{clipboard::WordSpan, presenter::PreviewContentMetrics};
use kwikpaste_ui::theme::{TextSize, fonts};

/// 面板与卡片之间的距离。
const GAP: f64 = 40.;
/// 面板离显示器边缘至少这么远。
const MARGIN: f64 = 32.;
const MIN_WIDTH: f64 = 288.;
const MAX_WIDTH: f64 = 480.;
const MIN_HEIGHT: f64 = 96.;
const MAX_HEIGHT: f64 = 480.;
/// 面板头部（标题、说明、类型标签）的高度。
pub const HEADER_HEIGHT: f64 = 48.;
const IMAGE_PADDING_X: f64 = 32.;
const IMAGE_PADDING_Y: f64 = 32.;
const EMPTY_HEIGHT: f64 = 96.;
/// 纯文本视图一行的高度（`leading-5.5`）。
pub const TEXT_ROW_HEIGHT: f64 = 22.;
const TEXT_VERTICAL_PADDING: f64 = 32.;
/// 纯文本视图左右各自的内边距。
pub const TEXT_PADDING_X: f64 = 16.;
/// 文件视图一行的高度。
pub const FILE_ROW_HEIGHT: f64 = 40.;
const FILE_VERTICAL_PADDING: f64 = 16.;
const FILE_MORE_HEIGHT: f64 = 40.;
const WORD_CHIP_HEIGHT: f64 = 24.;
const WORD_LINE_HEIGHT: f64 = 20.;
/// 词块之间的间距。
pub const WORD_GAP: f64 = 4.;
/// 词块区的内边距。
pub const WORDS_PADDING: f64 = 16.;
/// 选词区左右再扣掉的宽度：预览窗两侧各 1px 边框，加 1px 余量（GPUI 按设备像素取整，
/// 词块实际会比测得的略宽；宁可多留一行空白也不能裁掉最后一行）。
const WORDS_EDGE: f64 = 3.;
const FALLBACK_SIZE: (f64, f64) = (320., 240.);

/// 逻辑像素的矩形。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectF {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

impl RectF {
    fn right(&self) -> f64 {
        self.left + self.width
    }

    fn bottom(&self) -> f64 {
        self.top + self.height
    }

    fn center_x(&self) -> f64 {
        self.left + self.width / 2.
    }

    fn center_y(&self) -> f64 {
        self.top + self.height / 2.
    }

    fn inset(&self, amount: f64) -> Self {
        Self {
            left: self.left + amount,
            top: self.top + amount,
            width: (self.width - amount * 2.).max(1.),
            height: (self.height - amount * 2.).max(1.),
        }
    }

    fn intersect(&self, other: &Self) -> Option<Self> {
        let left = self.left.max(other.left);
        let top = self.top.max(other.top);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        (right > left && bottom > top).then_some(Self {
            left,
            top,
            width: right - left,
            height: bottom - top,
        })
    }

    fn clamp_into(&self, bounds: &Self) -> Self {
        let max_left = (bounds.right() - self.width).max(bounds.left);
        let max_top = (bounds.bottom() - self.height).max(bounds.top);
        Self {
            left: self.left.clamp(bounds.left, max_left),
            top: self.top.clamp(bounds.top, max_top),
            ..*self
        }
    }
}

/// 面板在卡片的哪一边。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Right,
    Left,
    Bottom,
    Top,
}

/// 预览窗的位置与大小（显示器上的逻辑像素）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    pub panel: RectF,
    pub placement: Placement,
}

/// 算预览窗的位置：`card` 是卡片、`monitor` 是显示器（同一坐标系的逻辑像素），`metrics` 为 `None`
/// 表示记录已经不在了（只剩空状态）。`prefer_left`：指针在主窗口左半边。`scale` 是文本缩放，
/// 面板内容随它放大，尺寸上下限和间距也跟着放大。
pub fn geometry(
    card: RectF,
    monitor: RectF,
    metrics: Option<&PreviewContentMetrics>,
    prefer_left: bool,
    scale: f64,
) -> Geometry {
    let available = monitor.inset(MARGIN);
    let source = card
        .intersect(&monitor)
        .unwrap_or_else(|| card.clamp_into(&monitor));
    let size = panel_size(metrics, &available, scale);
    let placement = placement(&source, &available, size, prefer_left, scale);
    let panel = raw_panel(&source, placement, size, scale).clamp_into(&available);

    Geometry { panel, placement }
}

/// 用 GPUI 实际使用的正文文字系统修正选词词块宽度。
///
/// core 在没有窗口的线程上只能用字符宽度估算；预览窗开在 UI 线程，
/// 这里用同一套字体、回退字体和字号重新排一遍，保证定尺寸和实际渲染一致。
pub fn measure_metrics(
    metrics: &PreviewContentMetrics,
    text: Option<&str>,
    words: &[WordSpan],
    scale: f64,
    window: &Window,
) -> PreviewContentMetrics {
    let PreviewContentMetrics::Words { chips } = metrics else {
        return metrics.clone();
    };
    let Some(text) = text else {
        return metrics.clone();
    };

    let font_size = px(TextSize::Sm.font_size().0 * 16. * scale as f32);
    let mut measured = chips.clone();
    for ((span, _chip), measured_chip) in words.iter().zip(chips).zip(&mut measured) {
        let range = utf16_range(text, span.0, span.1);
        let Some(word) = text.get(range) else {
            continue;
        };
        let mut ui_font = font(fonts::UI_FAMILY);
        ui_font.fallbacks = Some(fonts::ui_fallbacks());
        ui_font.weight = FontWeight::NORMAL;
        let run = TextRun {
            len: word.len(),
            font: ui_font,
            ..TextRun::default()
        };
        let width = window
            .text_system()
            .layout_line(word, font_size, &[run], None)
            .width
            .as_f32() as f64;
        measured_chip.width = (width / scale + 12.).ceil().max(24.);
    }

    PreviewContentMetrics::Words { chips: measured }
}

/// 按内容度量算面板尺寸，规则与面板的渲染一一对应。
fn panel_size(
    metrics: Option<&PreviewContentMetrics>,
    available: &RectF,
    scale: f64,
) -> (f64, f64) {
    let max_width = (MAX_WIDTH * scale).min(available.width);
    let max_height = (MAX_HEIGHT * scale).min(available.height);
    let min_width = (MIN_WIDTH * scale).min(max_width);
    let min_height = (MIN_HEIGHT * scale).min(max_height);
    let header = HEADER_HEIGHT * scale;

    let (width, height) = match metrics {
        None => (max_width, header + EMPTY_HEIGHT * scale),
        Some(PreviewContentMetrics::Text { rows }) => {
            (max_width, header + text_height(*rows) * scale)
        }
        Some(PreviewContentMetrics::Words { chips }) => {
            let widths: Vec<(f64, bool)> = chips
                .iter()
                .map(|chip| (chip.width, chip.line_break))
                .collect();
            let content = max_width / scale - WORDS_PADDING * 2. - WORDS_EDGE;
            (max_width, header + words_height(&widths, content) * scale)
        }
        Some(PreviewContentMetrics::Files { shown, total }) => {
            (max_width, header + files_height(*shown, *total) * scale)
        }
        Some(PreviewContentMetrics::Image { width, height }) => {
            image_size(*width, *height, max_width, max_height, scale)
        }
    };

    (
        width.clamp(min_width, max_width),
        height.clamp(min_height, max_height),
    )
}

fn text_height(rows: u32) -> f64 {
    if rows == 0 {
        return EMPTY_HEIGHT;
    }
    f64::from(rows) * TEXT_ROW_HEIGHT + TEXT_VERTICAL_PADDING
}

/// 拆词视图按内容宽度模拟 flex 折行（1.x `words_content_height`）：`chips` 是（估算宽度、前面是否换段）。
fn words_height(chips: &[(f64, bool)], content_width: f64) -> f64 {
    if chips.is_empty() || content_width <= 0. {
        return EMPTY_HEIGHT;
    }

    let mut rows = 0_u32;
    let mut breaks = 0_u32;
    let mut wrapped_lines = 0.;
    let mut line_width = 0.;
    for &(chip_width, line_break) in chips {
        let width = chip_width.min(content_width);
        wrapped_lines += (chip_width / content_width).ceil().max(1.) - 1.;
        if rows == 0 {
            rows = 1;
            line_width = width;
        } else if line_break {
            breaks += 1;
            rows += 1;
            line_width = width;
        } else if line_width + WORD_GAP + width > content_width {
            rows += 1;
            line_width = width;
        } else {
            line_width += WORD_GAP + width;
        }
    }

    f64::from(rows) * WORD_CHIP_HEIGHT
        + f64::from(rows - 1 + breaks) * WORD_GAP
        + wrapped_lines * WORD_LINE_HEIGHT
        + WORDS_PADDING * 2.
}

fn files_height(shown: u32, total: u32) -> f64 {
    if shown == 0 {
        return EMPTY_HEIGHT;
    }
    let footer = if total > shown { FILE_MORE_HEIGHT } else { 0. };
    f64::from(shown) * FILE_ROW_HEIGHT + FILE_VERTICAL_PADDING + footer
}

/// 图片按原始宽高等比缩到面板上限内，面板正好裹住缩放后的图加内边距。
fn image_size(
    width: Option<f64>,
    height: Option<f64>,
    max_width: f64,
    max_height: f64,
    scale: f64,
) -> (f64, f64) {
    let fallback = (FALLBACK_SIZE.0 * scale, FALLBACK_SIZE.1 * scale);
    let (Some(width), Some(height)) = (width, height) else {
        return fallback;
    };
    if width <= 0. || height <= 0. {
        return fallback;
    }

    let padding_x = IMAGE_PADDING_X * scale;
    let padding_y = (HEADER_HEIGHT + IMAGE_PADDING_Y) * scale;
    let fit = 1_f64
        .min((max_width - padding_x).max(1.) / width)
        .min((max_height - padding_y).max(1.) / height);

    (
        (width * fit).ceil() + padding_x,
        (height * fit).ceil() + padding_y,
    )
}

/// 按偏好的一侧依次找放得下的方位：偏好侧、另一侧、下、上；都放不下时用偏好侧（再夹进显示器）。
fn placement(
    source: &RectF,
    available: &RectF,
    (width, height): (f64, f64),
    prefer_left: bool,
    scale: f64,
) -> Placement {
    let gap = GAP * scale;
    let (preferred, opposite) = if prefer_left {
        (Placement::Left, Placement::Right)
    } else {
        (Placement::Right, Placement::Left)
    };
    let fits = |placement: Placement| match placement {
        Placement::Right => source.right() + gap + width <= available.right(),
        Placement::Left => source.left - gap - width >= available.left,
        Placement::Bottom => source.bottom() + gap + height <= available.bottom(),
        Placement::Top => source.top - gap - height >= available.top,
    };

    [preferred, opposite, Placement::Bottom, Placement::Top]
        .into_iter()
        .find(|placement| fits(*placement))
        .unwrap_or(preferred)
}

fn raw_panel(
    source: &RectF,
    placement: Placement,
    (width, height): (f64, f64),
    scale: f64,
) -> RectF {
    let gap = GAP * scale;
    let centered_top = source.center_y() - height / 2.;
    let centered_left = source.center_x() - width / 2.;
    let (left, top) = match placement {
        Placement::Right => (source.right() + gap, centered_top),
        Placement::Left => (source.left - gap - width, centered_top),
        Placement::Bottom => (centered_left, source.bottom() + gap),
        Placement::Top => (centered_left, source.top - gap - height),
    };

    RectF {
        left,
        top,
        width,
        height,
    }
}

/// 纯文本视图的折行宽度：文本内容的面板与显示器能给的最大宽度一样，再减去两侧 1 px 边框和左右内边距。
pub fn text_wrap_width(monitor: &RectF, scale: f64) -> f64 {
    let panel = (MAX_WIDTH * scale).min(monitor.inset(MARGIN).width);
    panel - 2. - TEXT_PADDING_X * 2. * scale
}

/// 纯文本视图的行：按换行拆开，每行再按正文字体（`text-sm`）在 `width` 内折开，不把句末标点放到行首。
/// 返回每行的字节区间。
pub fn text_rows(text: &str, width: f64, scale: f64, cx: &App) -> Vec<Range<usize>> {
    let font_size = px(TextSize::Sm.font_size().0 * 16. * scale as f32);
    kwikpaste_ui::wrap_rows(text, px(width as f32), font_size, cx)
}

/// UTF-16 区间（core 的词区间）换成字节区间；越界时截到文本末尾。
pub fn utf16_range(text: &str, start: u32, end: u32) -> Range<usize> {
    let (start, end) = (start as usize, end as usize);
    let mut units = 0;
    let mut byte_start = text.len();
    let mut byte_end = text.len();
    for (offset, c) in text.char_indices() {
        if units >= start && byte_start == text.len() {
            byte_start = offset;
        }
        if units >= end {
            byte_end = offset;
            break;
        }
        units += c.len_utf16();
    }
    byte_start.min(byte_end)..byte_end
}

/// 字节数的显示（1.x `formatBytes`）：1024 进位，10 以下保留一位小数。
pub fn format_bytes(value: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = value.max(0) as f64;
    let mut unit = 0;
    while size >= 1024. && unit < UNITS.len() - 1 {
        size /= 1024.;
        unit += 1;
    }
    let digits = if unit == 0 || size >= 10. { 0 } else { 1 };
    format!(
        "{size:.digits$} {}",
        UNITS.get(unit).copied().unwrap_or("B")
    )
}

/// 选词（1.x `useWordSelection`）：按下的词决定这一笔是勾选还是取消，拖过的区间跟着变。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WordSelection {
    selected: BTreeSet<usize>,
    drag: Option<WordDrag>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WordDrag {
    anchor: usize,
    base: BTreeSet<usize>,
    select: bool,
}

impl WordSelection {
    pub fn selected(&self) -> &BTreeSet<usize> {
        &self.selected
    }

    pub fn is_selected(&self, index: usize) -> bool {
        self.selected.contains(&index)
    }

    pub fn clear(&mut self) {
        self.selected.clear();
        self.drag = None;
    }

    /// 按下一个词：开始一笔。
    pub fn press(&mut self, index: usize) {
        let drag = WordDrag {
            anchor: index,
            base: self.selected.clone(),
            select: !self.selected.contains(&index),
        };
        self.drag = Some(drag);
        self.extend(index);
    }

    /// 拖到另一个词：从起点到这里的区间按这一笔的方向勾选或取消。
    pub fn extend(&mut self, index: usize) {
        let Some(drag) = &self.drag else {
            return;
        };
        let mut next = drag.base.clone();
        for current in drag.anchor.min(index)..=drag.anchor.max(index) {
            if drag.select {
                next.insert(current);
            } else {
                next.remove(&current);
            }
        }
        self.selected = next;
    }

    pub fn release(&mut self) {
        self.drag = None;
    }

    pub fn dragging(&self) -> bool {
        self.drag.is_some()
    }
}

#[cfg(test)]
mod tests {
    use kwikpaste_core::clipboard::split_words;
    use kwikpaste_core::presenter::PreviewWordChip;

    use super::*;

    const MONITOR: RectF = RectF {
        left: 0.,
        top: 0.,
        width: 1920.,
        height: 1080.,
    };

    fn card(left: f64) -> RectF {
        RectF {
            left,
            top: 300.,
            width: 340.,
            height: 80.,
        }
    }

    #[test]
    fn text_panels_take_the_full_width_and_grow_with_rows() {
        let geometry = geometry(
            card(800.),
            MONITOR,
            Some(&PreviewContentMetrics::Text { rows: 3 }),
            false,
            1.,
        );
        assert_eq!(geometry.placement, Placement::Right);
        assert_eq!(geometry.panel.width, 480.);
        assert_eq!(geometry.panel.height, 48. + 3. * 22. + 32.);
        assert_eq!(geometry.panel.left, 800. + 340. + 40.);
        assert_eq!(geometry.panel.top, 340. - geometry.panel.height / 2.);
    }

    #[test]
    fn the_pointer_side_wins_and_falls_back_to_the_other_side() {
        let left = geometry(card(800.), MONITOR, None, true, 1.);
        assert_eq!(left.placement, Placement::Left);

        // 贴着右边缘：右边放不下，换到左边。
        let edge = geometry(card(1500.), MONITOR, None, false, 1.);
        assert_eq!(edge.placement, Placement::Left);
    }

    #[test]
    fn images_fit_inside_the_limits() {
        let geometry = geometry(
            card(200.),
            MONITOR,
            Some(&PreviewContentMetrics::Image {
                width: Some(4000.),
                height: Some(1000.),
            }),
            false,
            1.,
        );
        assert_eq!(geometry.panel.width, 480.);
        assert!(geometry.panel.height < 480.);

        let small = panel_size(
            Some(&PreviewContentMetrics::Image {
                width: Some(10.),
                height: Some(10.),
            }),
            &MONITOR.inset(MARGIN),
            1.,
        );
        assert_eq!(small, (288., 96.));
    }

    #[test]
    fn words_wrap_like_flex() {
        let chips = vec![
            PreviewWordChip {
                width: 100.,
                line_break: false,
            };
            9
        ];
        // 内容宽 448：一行放 4 个（4×100 + 3×4 = 412），9 个占 3 行。
        let height = panel_size(
            Some(&PreviewContentMetrics::Words { chips }),
            &MONITOR.inset(MARGIN),
            1.,
        )
        .1;
        assert_eq!(height, 48. + 3. * 24. + 2. * 4. + 32.);
    }

    #[test]
    fn measured_order_preview_keeps_the_phone_suffix_visible() {
        let text = "订单 20260924 已发货，联系 138 1234 5678";
        let tokens = split_words(text).tokens;
        let estimated: Vec<_> = tokens
            .iter()
            .map(|token| PreviewWordChip::new(&token.text, token.line_break))
            .collect();
        let estimated_height = words_height(
            &estimated
                .iter()
                .map(|chip| (chip.width, chip.line_break))
                .collect::<Vec<_>>(),
            448.,
        );
        assert_eq!(estimated_height, 56.);

        // GPUI 的真实字形比 em 估算略宽；这组宽度让最后的 `5678` 落到第二行，
        // 面板尺寸因此必须多留一行，避免原来的裁切。
        let measured: Vec<_> = estimated
            .iter()
            .map(|chip| PreviewWordChip {
                width: chip.width + 2.,
                ..*chip
            })
            .collect();
        let measured_height = words_height(
            &measured
                .iter()
                .map(|chip| (chip.width, chip.line_break))
                .collect::<Vec<_>>(),
            448.,
        );
        assert_eq!(measured_height, 84.);
    }

    #[test]
    fn panels_scale_with_text_size() {
        let size = panel_size(None, &MONITOR.inset(MARGIN), 1.25);
        assert_eq!(size, (600., (48. + 96.) * 1.25));
    }

    #[test]
    fn long_text_fills_the_panel_and_scrolls() {
        let (width, height) = panel_size(
            Some(&PreviewContentMetrics::Text { rows: 40 }),
            &MONITOR.inset(MARGIN),
            1.,
        );
        assert_eq!(width, 480.);
        assert_eq!(height, 480.);
    }

    #[test]
    fn text_wraps_inside_the_borders_and_padding() {
        assert_eq!(text_wrap_width(&MONITOR, 1.), 480. - 2. - 32.);
        assert_eq!(text_wrap_width(&MONITOR, 1.25), 600. - 2. - 40.);
    }

    #[test]
    fn utf16_ranges_map_to_bytes() {
        let text = "a中😀b";
        assert_eq!(&text[utf16_range(text, 1, 2)], "中");
        assert_eq!(&text[utf16_range(text, 2, 4)], "😀");
        assert_eq!(&text[utf16_range(text, 4, 5)], "b");
        assert_eq!(utf16_range(text, 9, 12), text.len()..text.len());
    }

    #[test]
    fn bytes_read_like_1x() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(20 * 1024 * 1024), "20 MB");
    }

    #[test]
    fn word_drags_follow_the_first_word() {
        let mut selection = WordSelection::default();
        selection.press(2);
        selection.extend(4);
        selection.release();
        assert_eq!(
            selection.selected().iter().copied().collect::<Vec<_>>(),
            [2, 3, 4]
        );

        // 从已选的词开始拖：这一笔是取消。
        selection.press(3);
        selection.extend(5);
        selection.release();
        assert_eq!(
            selection.selected().iter().copied().collect::<Vec<_>>(),
            [2]
        );
    }
}
