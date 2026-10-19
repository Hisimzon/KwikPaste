//! 大图预览的文字层：识别行在舞台上的位置、指针命中与跨行选区。
//!
//! 识别行的四角是原图上的归一化坐标（0..1，未旋转），按阅读方向依次为起点上、终点上、终点下、
//! 起点下；行内字符按界面字体排出的宽度比例摊进行框。选区是整段识别文字（各行以换行相连）里的
//! 字符序号区间，和 core 存的识别文字一一对应。

use std::ops::Range;

/// 平面上的点。
pub type Point = (f32, f32);

/// 原图归一化坐标到舞台坐标：图片左上角、显示尺寸（转后的方向）和顺时针转过的直角数。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub origin: Point,
    pub size: (f32, f32),
    pub turns: u8,
}

impl Placement {
    pub fn place(&self, (x, y): Point) -> Point {
        let (x, y) = match self.turns % 4 {
            1 => (1. - y, x),
            2 => (1. - x, 1. - y),
            3 => (y, 1. - x),
            _ => (x, y),
        };
        (
            self.origin.0 + x * self.size.0,
            self.origin.1 + y * self.size.1,
        )
    }

    fn quad(&self, quad: &[Point; 4]) -> [Point; 4] {
        quad.map(|point| self.place(point))
    }
}

/// 一行识别文字。
#[derive(Clone, Debug, PartialEq)]
struct Line {
    /// 行首在整段识别文字里的字符序号。
    start: usize,
    quad: [Point; 4],
    /// 每个字符边界在行框里的位置（0..=1），共“字符数 + 1”个，单调不减。
    stops: Vec<f32>,
}

impl Line {
    fn chars(&self) -> usize {
        self.stops.len() - 1
    }

    /// 离 `t`（行框里的位置）最近的字符边界。
    fn caret(&self, t: f32) -> usize {
        let after = self.stops.partition_point(|stop| *stop < t);
        if after == 0 {
            return 0;
        }
        if after == self.stops.len() {
            return self.chars();
        }
        if t - self.stops[after - 1] <= self.stops[after] - t {
            after - 1
        } else {
            after
        }
    }
}

/// 一行在舞台上的几何：起点中点、终点中点和行高。
struct Placed {
    from: Point,
    to: Point,
    thickness: f32,
}

impl Placed {
    fn new(quad: &[Point; 4]) -> Self {
        Self {
            from: midpoint(quad[0], quad[3]),
            to: midpoint(quad[1], quad[2]),
            thickness: (distance(quad[0], quad[3]) + distance(quad[1], quad[2])) / 2.,
        }
    }

    /// `point` 投到行的走向上的位置（0 是起点，1 是终点）和离中线的距离。
    fn project(&self, point: Point) -> (f32, f32) {
        let axis = (self.to.0 - self.from.0, self.to.1 - self.from.1);
        let length = (axis.0 * axis.0 + axis.1 * axis.1).sqrt().max(f32::EPSILON);
        let offset = (point.0 - self.from.0, point.1 - self.from.1);
        let along = (offset.0 * axis.0 + offset.1 * axis.1) / length;
        let across = (offset.0 * axis.1 - offset.1 * axis.0) / length;
        (along / length, across.abs())
    }

    fn contains(&self, point: Point) -> bool {
        let (t, across) = self.project(point);
        (0. ..=1.).contains(&t) && across <= self.thickness / 2.
    }

    /// 到行框的距离，框内为 0。
    fn gap(&self, point: Point) -> f32 {
        let (t, across) = self.project(point);
        let length = distance(self.from, self.to);
        let along = if t < 0. {
            -t * length
        } else if t > 1. {
            (t - 1.) * length
        } else {
            0.
        };
        along.hypot((across - self.thickness / 2.).max(0.))
    }
}

/// 一张图的全部识别行。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextLayer {
    lines: Vec<Line>,
    /// 整段识别文字的字符数。
    len: usize,
}

impl TextLayer {
    /// `lines` 是识别行的文字和四角；`measure` 给出一行文字每个字符边界的横坐标（从 0 开始，
    /// 共“字符数 + 1”个），只用比例。量不出宽度的行按字符数平分。
    pub fn new(
        lines: impl IntoIterator<Item = (String, [Point; 4])>,
        mut measure: impl FnMut(&str) -> Vec<f32>,
    ) -> Self {
        let mut layer = Self::default();
        for (text, quad) in lines {
            let chars = text.chars().count();
            if !layer.lines.is_empty() {
                layer.len += 1;
            }
            let offsets = measure(&text);
            let width = offsets.last().copied().unwrap_or(0.);
            let stops = if offsets.len() == chars + 1 && width > 0. {
                offsets.iter().map(|x| (x / width).clamp(0., 1.)).collect()
            } else {
                (0..=chars)
                    .map(|index| index as f32 / chars.max(1) as f32)
                    .collect()
            };
            layer.lines.push(Line {
                start: layer.len,
                quad,
                stops,
            });
            layer.len += chars;
        }
        layer
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 全部文字。
    pub fn all(&self) -> Range<usize> {
        0..self.len
    }

    /// 指针落在哪一行的框里（舞台坐标）。
    pub fn line_at(&self, placement: &Placement, point: Point) -> Option<usize> {
        self.lines.iter().position(|line| {
            line.chars() > 0 && Placed::new(&placement.quad(&line.quad)).contains(point)
        })
    }

    /// 第 `index` 行的字符区间。
    pub fn line_range(&self, index: usize) -> Range<usize> {
        self.lines
            .get(index)
            .map_or(0..0, |line| line.start..line.start + line.chars())
    }

    /// 指针在文字上时的插入点（整段文字里的字符序号）。
    pub fn caret_at(&self, placement: &Placement, point: Point) -> Option<usize> {
        let index = self.line_at(placement, point)?;
        Some(self.caret_on(index, placement, point))
    }

    /// 拖选时的插入点：指针不在任何行上就取离它最近的一行。
    pub fn nearest_caret(&self, placement: &Placement, point: Point) -> Option<usize> {
        let index = self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, line)| line.chars() > 0)
            .map(|(index, line)| (index, Placed::new(&placement.quad(&line.quad)).gap(point)))
            .min_by(|a, b| a.1.total_cmp(&b.1))?
            .0;
        Some(self.caret_on(index, placement, point))
    }

    fn caret_on(&self, index: usize, placement: &Placement, point: Point) -> usize {
        let line = &self.lines[index];
        let (t, _) = Placed::new(&placement.quad(&line.quad)).project(point);
        line.start + line.caret(t)
    }

    /// 每一行整个行框（舞台坐标），标出文字区域用。
    pub fn regions(&self, placement: &Placement) -> Vec<[Point; 4]> {
        self.lines
            .iter()
            .filter(|line| line.chars() > 0)
            .map(|line| placement.quad(&line.quad))
            .collect()
    }

    /// 选区在各行上的高亮块（舞台坐标）。
    pub fn highlights(&self, placement: &Placement, range: &Range<usize>) -> Vec<[Point; 4]> {
        self.lines
            .iter()
            .filter_map(|line| {
                let from = range.start.max(line.start) - line.start;
                let to = range
                    .end
                    .min(line.start + line.chars())
                    .checked_sub(line.start)?;
                if from >= to {
                    return None;
                }
                let [top_start, top_end, bottom_end, bottom_start] = placement.quad(&line.quad);
                let (t0, t1) = (line.stops[from], line.stops[to]);
                Some([
                    lerp(top_start, top_end, t0),
                    lerp(top_start, top_end, t1),
                    lerp(bottom_start, bottom_end, t1),
                    lerp(bottom_start, bottom_end, t0),
                ])
            })
            .collect()
    }
}

fn midpoint(a: Point, b: Point) -> Point {
    ((a.0 + b.0) / 2., (a.1 + b.1) / 2.)
}

fn distance(a: Point, b: Point) -> f32 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn lerp(a: Point, b: Point, t: f32) -> Point {
    (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> [Point; 4] {
        [
            (x, y),
            (x + width, y),
            (x + width, y + height),
            (x, y + height),
        ]
    }

    /// 两行等宽字符：“ab”在上（舞台上 10..30 × 10..20）、“cde”在下（10..40 × 40..50）。
    fn layer() -> TextLayer {
        TextLayer::new(
            [
                ("ab".to_owned(), rect(0.125, 0.125, 0.25, 0.125)),
                ("cde".to_owned(), rect(0.125, 0.5, 0.375, 0.125)),
            ],
            |text| (0..=text.chars().count()).map(|i| i as f32).collect(),
        )
    }

    const PLAIN: Placement = Placement {
        origin: (0., 0.),
        size: (80., 80.),
        turns: 0,
    };

    #[test]
    fn offsets_follow_the_joined_text() {
        let layer = layer();
        assert_eq!(layer.all(), 0..6);
        assert_eq!(layer.line_range(1), 3..6);
        assert_eq!(layer.caret_at(&PLAIN, (12., 15.)), Some(0));
        assert_eq!(layer.caret_at(&PLAIN, (39., 45.)), Some(6));
        assert_eq!(layer.caret_at(&PLAIN, (50., 30.)), None);
    }

    #[test]
    fn dragging_off_the_text_snaps_to_the_nearest_line() {
        let layer = layer();
        assert_eq!(layer.nearest_caret(&PLAIN, (5., 70.)), Some(3));
        assert_eq!(layer.nearest_caret(&PLAIN, (60., 12.)), Some(2));
    }

    #[test]
    fn highlights_cover_the_selected_part_of_each_line() {
        let blocks = layer().highlights(&PLAIN, &(1..4));
        let expected = [
            [(20., 10.), (30., 10.), (30., 20.), (20., 20.)],
            [(10., 40.), (20., 40.), (20., 50.), (10., 50.)],
        ];
        assert_eq!(blocks.len(), expected.len());
        for (block, expected) in blocks.iter().zip(expected) {
            for (point, expected) in block.iter().zip(expected) {
                assert!(distance(*point, expected) < 1e-3, "{block:?}");
            }
        }
    }

    #[test]
    fn rotation_moves_lines_with_the_picture() {
        let layer = layer();
        let turned = Placement { turns: 1, ..PLAIN };
        // 转一次之后第一行竖着落在右上角，从上往下读。
        assert_eq!(turned.place((0.125, 0.125)), (70., 10.));
        assert_eq!(layer.caret_at(&turned, (65., 12.)), Some(0));
        assert_eq!(layer.caret_at(&turned, (65., 29.)), Some(2));
    }
}
