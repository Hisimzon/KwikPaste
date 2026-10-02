//! 窗口定位的纯函数。坐标一律是屏幕坐标、y 轴向下；Windows 上是物理像素，macOS 上是翻转过 y 轴的 point。

/// 屏幕上的点。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// 宽高。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: i32,
    pub height: i32,
}

/// 矩形，右、下边界不含（与 Win32 `RECT` 相同）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// 外框比内容区多出的四边宽度（Windows 上是不可见的拉伸边）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Insets {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn from_origin_size(origin: Point, size: Size) -> Self {
        Self {
            left: origin.x,
            top: origin.y,
            right: origin.x + size.width,
            bottom: origin.y + size.height,
        }
    }

    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn size(&self) -> Size {
        Size {
            width: self.width(),
            height: self.height(),
        }
    }

    /// 把四边各向外扩 `insets`：内容区 → 外框。
    pub fn outset(&self, insets: Insets) -> Self {
        Self {
            left: self.left - insets.left,
            top: self.top - insets.top,
            right: self.right + insets.right,
            bottom: self.bottom + insets.bottom,
        }
    }

    /// `outer` 比 `self`（内容区）多出的四边宽度。
    pub fn insets_within(&self, outer: Rect) -> Insets {
        Insets {
            left: self.left - outer.left,
            top: self.top - outer.top,
            right: outer.right - self.right,
            bottom: outer.bottom - self.bottom,
        }
    }
}

/// 逻辑尺寸按缩放换算成物理尺寸，四舍五入，至少 1。
pub fn scale_size(logical: (f64, f64), scale: f64) -> Size {
    Size {
        width: ((logical.0 * scale).round() as i32).max(1),
        height: ((logical.1 * scale).round() as i32).max(1),
    }
}

/// 面板内容区跟随光标：左上角放在光标处；超出工作区右、下边时向左、向上推回；
/// 比工作区还大时收到工作区大小。
pub fn follow_cursor(cursor: Point, work_area: Rect, size: Size) -> Rect {
    let size = Size {
        width: size.width.clamp(1, work_area.width().max(1)),
        height: size.height.clamp(1, work_area.height().max(1)),
    };
    let x = cursor
        .x
        .min(work_area.right - size.width)
        .max(work_area.left);
    let y = cursor
        .y
        .min(work_area.bottom - size.height)
        .max(work_area.top);

    Rect::from_origin_size(Point { x, y }, size)
}

/// 面板内容区放在工作区正中；比工作区还大时收到工作区大小。
pub fn center_in(work_area: Rect, size: Size) -> Rect {
    let size = Size {
        width: size.width.clamp(1, work_area.width().max(1)),
        height: size.height.clamp(1, work_area.height().max(1)),
    };
    let origin = Point {
        x: work_area.left + (work_area.width() - size.width) / 2,
        y: work_area.top + (work_area.height() - size.height) / 2,
    };

    Rect::from_origin_size(origin, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centers_inside_the_work_area_and_clamps_the_size() {
        let work = Rect {
            left: 100,
            top: 50,
            right: 1100,
            bottom: 850,
        };

        let centered = center_in(
            work,
            Size {
                width: 400,
                height: 600,
            },
        );
        let clamped = center_in(
            work,
            Size {
                width: 4000,
                height: 600,
            },
        );

        assert_eq!(
            centered,
            Rect {
                left: 400,
                top: 150,
                right: 800,
                bottom: 750,
            }
        );
        assert_eq!((clamped.left, clamped.width()), (100, 1000));
    }

    const WORK: Rect = Rect {
        left: 0,
        top: 0,
        right: 3840,
        bottom: 2088,
    };
    const PANEL: Size = Size {
        width: 540,
        height: 900,
    };

    #[test]
    fn places_top_left_at_cursor_when_it_fits() {
        let rect = follow_cursor(Point { x: 100, y: 200 }, WORK, PANEL);

        assert_eq!(
            rect,
            Rect {
                left: 100,
                top: 200,
                right: 640,
                bottom: 1100
            }
        );
    }

    #[test]
    fn pushes_back_from_the_right_and_bottom_edges() {
        let rect = follow_cursor(Point { x: 3800, y: 2000 }, WORK, PANEL);

        assert_eq!(rect.right, WORK.right);
        assert_eq!(rect.bottom, WORK.bottom);
        assert_eq!(rect.size(), PANEL);
    }

    #[test]
    fn stays_inside_a_work_area_with_negative_origin() {
        let portrait = Rect {
            left: 3840,
            top: -767,
            right: 6000,
            bottom: 3001,
        };
        let rect = follow_cursor(Point { x: 3700, y: -900 }, portrait, PANEL);

        assert_eq!(rect.left, 3840);
        assert_eq!(rect.top, -767);
        assert_eq!(rect.size(), PANEL);
    }

    #[test]
    fn shrinks_to_a_work_area_smaller_than_the_panel() {
        let small = Rect {
            left: 0,
            top: 0,
            right: 400,
            bottom: 600,
        };
        let rect = follow_cursor(Point { x: 50, y: 50 }, small, PANEL);

        assert_eq!(rect, small);
    }

    #[test]
    fn scales_logical_size_with_rounding() {
        assert_eq!(
            scale_size((360.0, 600.0), 1.5),
            Size {
                width: 540,
                height: 900
            }
        );
        assert_eq!(
            scale_size((360.0, 600.0), 1.75),
            Size {
                width: 630,
                height: 1050
            }
        );
        assert_eq!(
            scale_size((361.0, 601.0), 1.25),
            Size {
                width: 451,
                height: 751
            }
        );
    }

    #[test]
    fn insets_round_trip_through_outset() {
        let client = Rect {
            left: 100,
            top: 200,
            right: 640,
            bottom: 1100,
        };
        let outer = Rect {
            left: 89,
            top: 200,
            right: 651,
            bottom: 1111,
        };
        let insets = client.insets_within(outer);

        assert_eq!(
            insets,
            Insets {
                left: 11,
                top: 0,
                right: 11,
                bottom: 11
            }
        );
        assert_eq!(client.outset(insets), outer);
    }
}
