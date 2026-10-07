//! 主题度量：字号、圆角、控件高度、间距与动效。
//!
//! 数值沿用原生 1.x 的视觉契约，作为无色的公共度量；颜色必须通过语义或组件 token 读取。

use gpui::{Rems, rems};
/// 标准语义字号，与 1.x UnoCSS wind4 一致：字号 / 行高（rem）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSize {
    /// `text-xs`：12 / 16 px。
    Xs,
    /// `text-sm`：14 / 20 px，正文。
    Sm,
    /// `text-base`：16 / 24 px。
    Base,
    /// `text-lg`：18 / 28 px。
    Lg,
}

impl TextSize {
    pub const ALL: [Self; 4] = [Self::Xs, Self::Sm, Self::Base, Self::Lg];

    pub fn font_size(self) -> Rems {
        match self {
            Self::Xs => rems(0.75),
            Self::Sm => rems(0.875),
            Self::Base => rems(1.),
            Self::Lg => rems(1.125),
        }
    }

    pub fn line_height(self) -> Rems {
        match self {
            Self::Xs => rems(1.),
            Self::Sm => rems(1.25),
            Self::Base => rems(1.5),
            Self::Lg => rems(1.75),
        }
    }

    /// UnoCSS 类名，展示用。
    pub fn class_name(self) -> &'static str {
        match self {
            Self::Xs => "text-xs",
            Self::Sm => "text-sm",
            Self::Base => "text-base",
            Self::Lg => "text-lg",
        }
    }
}

const fn rem_of(px: f32) -> Rems {
    Rems(px / 16.)
}

/// 圆角（像素值来自既有界面契约）。
pub mod radius {
    use super::rem_of;
    use crate::theme::palette;
    use gpui::Rems;

    pub const XS: Rems = rem_of(palette::RADIUS_XS);
    pub const SM: Rems = rem_of(palette::RADIUS_SM);
    pub const MD: Rems = rem_of(palette::RADIUS);
    pub const LG: Rems = rem_of(palette::RADIUS_LG);
}

/// 控件高度：数值来自既有 antd `controlHeight*` 的 16 / 24 / 32 / 40 px 档位。
pub mod control_height {
    use super::rem_of;
    use gpui::Rems;
    pub const XS: Rems = rem_of(16.);
    pub const SM: Rems = rem_of(24.);
    pub const MD: Rems = rem_of(32.);
    pub const LG: Rems = rem_of(40.);
}

/// UnoCSS wind4 间距：`n` 个 0.25 rem。
pub const fn space(n: f32) -> Rems {
    Rems(n * 0.25)
}

/// 把任意像素设计值换成 rem；用于尺寸而不是间距刻度。
pub const fn px_rems(px: f32) -> Rems {
    Rems((px / 4.) * 0.25)
}

/// CSS cubic-bezier 的四个控制点。
pub type CubicBezier = [f32; 4];

/// 动效时长与缓动：数值来自既有 antd motion duration/easing 契约。
pub mod motion {
    use super::CubicBezier;
    use std::time::Duration;

    pub const FAST: Duration = Duration::from_millis(100);
    pub const MID: Duration = Duration::from_millis(200);
    pub const SLOW: Duration = Duration::from_millis(300);
    pub const COLORS: Duration = Duration::from_millis(150);
    pub const POP: Duration = Duration::from_millis(160);
    pub const SWITCH: Duration = Duration::from_millis(180);
    pub const TOAST: Duration = Duration::from_secs(3);

    pub const EASE_OUT: CubicBezier = [0.215, 0.61, 0.355, 1.];
    pub const EASE_IN_OUT: CubicBezier = [0.645, 0.045, 0.355, 1.];
    pub const EASE_OUT_CIRC: CubicBezier = [0.08, 0.82, 0.17, 1.];
    pub const MOTION_EASE_OUT: CubicBezier = [0., 0., 0.58, 1.];
    pub const PANEL: CubicBezier = [0.22, 1., 0.36, 1.];

    pub fn duration(duration: Duration, reduce_motion: bool) -> Duration {
        if reduce_motion {
            Duration::ZERO
        } else {
            duration
        }
    }
}
