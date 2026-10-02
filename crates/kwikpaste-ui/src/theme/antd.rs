//! 冻结的 antd v6 design token：1.x 前端实际生效的两套主题（`theme.defaultAlgorithm` /
//! `theme.darkAlgorithm`，快贴没有自定义 seed，所以就是常量）。
//!
//! 来源是 `theme/antd_tokens_light_dark.json`（antd 6.6.0，536 个 token × 2，由同目录的
//! `export-antd-tokens.mjs` 从仓库 `node_modules/antd` 导出）。这里只收录原生版用到的子集，
//! 每一项都写明 antd token 名；单测 `frozen_tokens_match_json` 逐项比对冻结表，必须精确相等。
//! 运行时与构建时都不依赖 Node，也不移植 antd 的色板派生算法。

use std::time::Duration;

use gpui::{BoxShadow, Hsla, Rgba, point, px};

/// 一个冻结的颜色：`0xRRGGBB` 加不透明度，与 antd 输出的 `#rrggbb` / `rgba(r,g,b,a)` 一一对应。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TokenColor {
    pub rgb: u32,
    pub alpha: f32,
}

impl TokenColor {
    pub fn to_hsla(self) -> Hsla {
        let [_, r, g, b] = self.rgb.to_be_bytes();

        Rgba {
            r: f32::from(r) / 255.,
            g: f32::from(g) / 255.,
            b: f32::from(b) / 255.,
            a: self.alpha,
        }
        .into()
    }
}

const fn hex(rgb: u32) -> TokenColor {
    TokenColor { rgb, alpha: 1. }
}

const fn rgba(rgb: u32, alpha: f32) -> TokenColor {
    TokenColor { rgb, alpha }
}

/// 生成随明暗变化的颜色表：每行是 `字段 = "antd token 名": 亮色值, 暗色值;`。
macro_rules! antd_colors {
    ($($field:ident = $antd:literal: $light:expr, $dark:expr;)*) => {
        /// 随明暗变化的 antd 颜色 token，字段名是 antd token 名的 snake_case。
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct AntdColors {
            $(
                #[doc = concat!("antd `", $antd, "`")]
                pub $field: TokenColor,
            )*
        }

        /// 亮色主题（`theme.defaultAlgorithm`）。
        pub const LIGHT: AntdColors = AntdColors { $($field: $light,)* };

        /// 暗色主题（`theme.darkAlgorithm`）。
        pub const DARK: AntdColors = AntdColors { $($field: $dark,)* };

        impl AntdColors {
            /// `(antd token 名, 值)` 列表，供冻结表比对。
            pub fn entries(&self) -> Vec<(&'static str, TokenColor)> {
                vec![$(($antd, self.$field),)*]
            }
        }
    };
}

antd_colors! {
    color_primary = "colorPrimary": hex(0x1677ff), hex(0x1668dc);
    color_primary_hover = "colorPrimaryHover": hex(0x4096ff), hex(0x3c89e8);
    color_primary_active = "colorPrimaryActive": hex(0x0958d9), hex(0x1554ad);
    color_primary_bg = "colorPrimaryBg": hex(0xe6f4ff), hex(0x15325b);
    color_primary_bg_hover = "colorPrimaryBgHover": hex(0xbae0ff), hex(0x15417e);
    color_primary_border = "colorPrimaryBorder": hex(0x91caff), hex(0x15325b);
    color_success = "colorSuccess": hex(0x52c41a), hex(0x49aa19);
    color_success_hover = "colorSuccessHover": hex(0x95de64), hex(0x306317);
    color_success_active = "colorSuccessActive": hex(0x389e0d), hex(0x3c8618);
    color_success_bg = "colorSuccessBg": hex(0xf6ffed), hex(0x162312);
    color_success_border = "colorSuccessBorder": hex(0xb7eb8f), hex(0x274916);
    color_warning = "colorWarning": hex(0xfaad14), hex(0xd89614);
    color_warning_hover = "colorWarningHover": hex(0xffd666), hex(0x7c5914);
    color_warning_active = "colorWarningActive": hex(0xd48806), hex(0xaa7714);
    color_warning_bg = "colorWarningBg": hex(0xfffbe6), hex(0x2b2111);
    color_warning_border = "colorWarningBorder": hex(0xffe58f), hex(0x594214);
    color_error = "colorError": hex(0xff4d4f), hex(0xdc4446);
    color_error_hover = "colorErrorHover": hex(0xff7875), hex(0xe86e6b);
    color_error_active = "colorErrorActive": hex(0xd9363e), hex(0xad393a);
    color_error_bg = "colorErrorBg": hex(0xfff2f0), hex(0x2c1618);
    color_error_border = "colorErrorBorder": hex(0xffccc7), hex(0x5b2526);
    color_info = "colorInfo": hex(0x1677ff), hex(0x1668dc);
    color_info_hover = "colorInfoHover": hex(0x69b1ff), hex(0x15417e);
    color_info_active = "colorInfoActive": hex(0x0958d9), hex(0x1554ad);
    color_info_bg = "colorInfoBg": hex(0xe6f4ff), hex(0x111a2c);
    color_info_border = "colorInfoBorder": hex(0x91caff), hex(0x15325b);
    color_link = "colorLink": hex(0x1677ff), hex(0x1668dc);
    color_link_hover = "colorLinkHover": hex(0x69b1ff), hex(0x15417e);
    color_link_active = "colorLinkActive": hex(0x0958d9), hex(0x1554ad);
    color_text = "colorText": rgba(0x000000, 0.88), rgba(0xffffff, 0.85);
    color_text_secondary = "colorTextSecondary": rgba(0x000000, 0.65), rgba(0xffffff, 0.65);
    color_text_tertiary = "colorTextTertiary": rgba(0x000000, 0.45), rgba(0xffffff, 0.45);
    color_text_quaternary = "colorTextQuaternary": rgba(0x000000, 0.25), rgba(0xffffff, 0.25);
    color_text_disabled = "colorTextDisabled": rgba(0x000000, 0.25), rgba(0xffffff, 0.25);
    color_text_description = "colorTextDescription": rgba(0x000000, 0.45), rgba(0xffffff, 0.45);
    color_text_placeholder = "colorTextPlaceholder": rgba(0x000000, 0.25), rgba(0xffffff, 0.25);
    color_text_light_solid = "colorTextLightSolid": hex(0xffffff), hex(0xffffff);
    color_bg_container = "colorBgContainer": hex(0xffffff), hex(0x141414);
    color_bg_elevated = "colorBgElevated": hex(0xffffff), hex(0x1f1f1f);
    color_bg_layout = "colorBgLayout": hex(0xf5f5f5), hex(0x000000);
    color_bg_spotlight = "colorBgSpotlight": rgba(0x000000, 0.85), hex(0x424242);
    color_bg_mask = "colorBgMask": rgba(0x000000, 0.45), rgba(0x000000, 0.45);
    color_bg_text_hover = "colorBgTextHover": rgba(0x000000, 0.06), rgba(0xffffff, 0.12);
    color_bg_text_active = "colorBgTextActive": rgba(0x000000, 0.15), rgba(0xffffff, 0.18);
    color_border = "colorBorder": hex(0xd9d9d9), hex(0x424242);
    color_border_secondary = "colorBorderSecondary": hex(0xf0f0f0), hex(0x303030);
    color_border_disabled = "colorBorderDisabled": hex(0xd9d9d9), hex(0x424242);
    color_split = "colorSplit": rgba(0x050505, 0.06), rgba(0xfdfdfd, 0.12);
    color_fill = "colorFill": rgba(0x000000, 0.15), rgba(0xffffff, 0.18);
    color_fill_secondary = "colorFillSecondary": rgba(0x000000, 0.06), rgba(0xffffff, 0.12);
    color_fill_tertiary = "colorFillTertiary": rgba(0x000000, 0.04), rgba(0xffffff, 0.08);
    color_fill_quaternary = "colorFillQuaternary": rgba(0x000000, 0.02), rgba(0xffffff, 0.04);
    color_white = "colorWhite": hex(0xffffff), hex(0xffffff);
    control_item_bg_hover = "controlItemBgHover": rgba(0x000000, 0.04), rgba(0xffffff, 0.08);
    control_item_bg_active = "controlItemBgActive": hex(0xe6f4ff), hex(0x15325b);
    control_item_bg_active_hover = "controlItemBgActiveHover": hex(0xbae0ff), hex(0x15417e);
    blue_1 = "blue-1": hex(0xe6f4ff), hex(0x111a2c);
    blue_6 = "blue-6": hex(0x1677ff), hex(0x1668dc);
    cyan_1 = "cyan-1": hex(0xe6fffb), hex(0x112123);
    cyan_6 = "cyan-6": hex(0x13c2c2), hex(0x13a8a8);
    green_1 = "green-1": hex(0xf6ffed), hex(0x162312);
    green_6 = "green-6": hex(0x52c41a), hex(0x49aa19);
    red_1 = "red-1": hex(0xfff1f0), hex(0x2a1215);
    red_6 = "red-6": hex(0xf5222d), hex(0xd32029);
    yellow_1 = "yellow-1": hex(0xfeffe6), hex(0x2b2611);
    yellow_6 = "yellow-6": hex(0xfadb14), hex(0xd8bd14);
    magenta_1 = "magenta-1": hex(0xfff0f6), hex(0x291321);
    magenta_6 = "magenta-6": hex(0xeb2f96), hex(0xcb2b83);
    orange_6 = "orange-6": hex(0xfa8c16), hex(0xd87a16);
    purple_6 = "purple-6": hex(0x722ed1), hex(0x642ab5);
    gold_3 = "gold-3": hex(0xffe58f), hex(0x594214);
}

/// CSS `box-shadow` 的一层（antd 的阴影水平偏移都是 0）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowLayer {
    pub y: f32,
    pub blur: f32,
    pub spread: f32,
    pub color: TokenColor,
}

impl ShadowLayer {
    pub fn to_box_shadow(self) -> BoxShadow {
        BoxShadow {
            color: self.color.to_hsla(),
            offset: point(px(0.), px(self.y)),
            blur_radius: px(self.blur),
            spread_radius: px(self.spread),
            inset: false,
        }
    }
}

const fn layer(y: f32, blur: f32, spread: f32, color: TokenColor) -> ShadowLayer {
    ShadowLayer {
        y,
        blur,
        spread,
        color,
    }
}

/// 随明暗变化的阴影 token。antd 6 的暗色阴影由 `colorTextBase`（白）派生，冻结表照原样保留；
/// 冻结表里的 `0.010000000000000002` 是浮点误差，按 f32 与 `0.01` 相同。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AntdShadows {
    /// antd `boxShadow`（`boxShadowSecondary` 取值相同）：message、Modal、下拉等浮层。
    pub box_shadow: [ShadowLayer; 3],
    /// antd `boxShadowTertiary`：卡片一类贴近底面的元素。
    pub box_shadow_tertiary: [ShadowLayer; 3],
}

pub const LIGHT_SHADOWS: AntdShadows = AntdShadows {
    box_shadow: [
        layer(6., 16., 0., rgba(0x000000, 0.08)),
        layer(3., 6., -4., rgba(0x000000, 0.12)),
        layer(9., 28., 8., rgba(0x000000, 0.05)),
    ],
    box_shadow_tertiary: [
        layer(1., 2., 0., rgba(0x000000, 0.05)),
        layer(1., 6., -1., rgba(0x000000, 0.03)),
        layer(2., 4., 0., rgba(0x000000, 0.03)),
    ],
};

pub const DARK_SHADOWS: AntdShadows = AntdShadows {
    box_shadow: [
        layer(6., 16., 0., rgba(0xffffff, 0.016)),
        layer(3., 6., -4., rgba(0xffffff, 0.024)),
        layer(9., 28., 8., rgba(0xffffff, 0.01)),
    ],
    box_shadow_tertiary: [
        layer(1., 2., 0., rgba(0xffffff, 0.01)),
        layer(1., 6., -1., rgba(0xffffff, 0.006)),
        layer(2., 4., 0., rgba(0xffffff, 0.006)),
    ],
};

/// 生成与明暗无关的数值 token：每行是 `常量: 类型 = 值 => "antd token 名";`。
macro_rules! antd_numbers {
    ($($name:ident: $ty:ty = $value:expr => $antd:literal;)*) => {
        $(
            #[doc = concat!("antd `", $antd, "`")]
            pub const $name: $ty = $value;
        )*

        /// `(antd token 名, 值)` 列表，供冻结表比对。
        pub const NUMBERS: &[(&str, f64)] = &[$(($antd, $value as f64),)*];
    };
}

antd_numbers! {
    FONT_SIZE: f32 = 14. => "fontSize";
    FONT_SIZE_SM: f32 = 12. => "fontSizeSM";
    FONT_SIZE_LG: f32 = 16. => "fontSizeLG";
    FONT_SIZE_XL: f32 = 20. => "fontSizeXL";
    FONT_HEIGHT: f32 = 22. => "fontHeight";
    FONT_HEIGHT_SM: f32 = 20. => "fontHeightSM";
    FONT_HEIGHT_LG: f32 = 24. => "fontHeightLG";
    FONT_WEIGHT_STRONG: f32 = 600. => "fontWeightStrong";
    CONTROL_HEIGHT: f32 = 32. => "controlHeight";
    CONTROL_HEIGHT_SM: f32 = 24. => "controlHeightSM";
    CONTROL_HEIGHT_XS: f32 = 16. => "controlHeightXS";
    CONTROL_HEIGHT_LG: f32 = 40. => "controlHeightLG";
    BORDER_RADIUS: f32 = 6. => "borderRadius";
    BORDER_RADIUS_XS: f32 = 2. => "borderRadiusXS";
    BORDER_RADIUS_SM: f32 = 4. => "borderRadiusSM";
    BORDER_RADIUS_LG: f32 = 8. => "borderRadiusLG";
    PADDING_XXS: f32 = 4. => "paddingXXS";
    PADDING_XS: f32 = 8. => "paddingXS";
    PADDING_SM: f32 = 12. => "paddingSM";
    PADDING: f32 = 16. => "padding";
    PADDING_LG: f32 = 24. => "paddingLG";
    MARGIN_XS: f32 = 8. => "marginXS";
    MARGIN_SM: f32 = 12. => "marginSM";
    LINE_WIDTH: f32 = 1. => "lineWidth";
    CONTROL_OUTLINE_WIDTH: f32 = 2. => "controlOutlineWidth";
}

/// antd `motionDurationFast`（`0.1s`）。
pub const MOTION_DURATION_FAST: Duration = Duration::from_millis(100);
/// antd `motionDurationMid`（`0.2s`）。
pub const MOTION_DURATION_MID: Duration = Duration::from_millis(200);
/// antd `motionDurationSlow`（`0.3s`）。
pub const MOTION_DURATION_SLOW: Duration = Duration::from_millis(300);

/// `(antd token 名, 时长)` 列表，供冻结表比对。
pub const DURATIONS: &[(&str, Duration)] = &[
    ("motionDurationFast", MOTION_DURATION_FAST),
    ("motionDurationMid", MOTION_DURATION_MID),
    ("motionDurationSlow", MOTION_DURATION_SLOW),
];

/// CSS `cubic-bezier(x1, y1, x2, y2)` 的四个控制点。
pub type CubicBezier = [f32; 4];

/// antd `motionEaseOut`。
pub const MOTION_EASE_OUT: CubicBezier = [0.215, 0.61, 0.355, 1.];
/// antd `motionEaseInOut`。
pub const MOTION_EASE_IN_OUT: CubicBezier = [0.645, 0.045, 0.355, 1.];
/// antd `motionEaseOutCirc`。
pub const MOTION_EASE_OUT_CIRC: CubicBezier = [0.08, 0.82, 0.17, 1.];

/// `(antd token 名, 控制点)` 列表，供冻结表比对。
pub const EASINGS: &[(&str, CubicBezier)] = &[
    ("motionEaseOut", MOTION_EASE_OUT),
    ("motionEaseInOut", MOTION_EASE_IN_OUT),
    ("motionEaseOutCirc", MOTION_EASE_OUT_CIRC),
];
