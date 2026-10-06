//! 2.0 自己的配色与度量：中性冷灰分层、克制的品牌蓝、柔和阴影、稍大的圆角。
//!
//! 沿用 antd token 的字段结构（[`AntdColors`]），这样 gpui-component 的映射和语义层不用改；
//! 只覆盖界面实际用到的那部分，其余（色板里的绿、红、紫等）沿用冻结的 antd 值。
//! 层次：窗口底（`color_bg_layout`）< 卡片 / 内容区（`color_bg_container`）< 浮层（`color_bg_elevated`）。

use super::antd::{self, AntdColors, AntdShadows, ShadowLayer, TokenColor};

const fn hex(rgb: u32) -> TokenColor {
    TokenColor { rgb, alpha: 1. }
}

const fn rgba(rgb: u32, alpha: f32) -> TokenColor {
    TokenColor { rgb, alpha }
}

const fn layer(y: f32, blur: f32, spread: f32, color: TokenColor) -> ShadowLayer {
    ShadowLayer {
        y,
        blur,
        spread,
        color,
    }
}

/// 亮色。
pub const LIGHT: AntdColors = AntdColors {
    color_primary: hex(0x2e6bf0),
    color_primary_hover: hex(0x4b83f5),
    color_primary_active: hex(0x1f55d1),
    color_primary_bg: hex(0xeef3fe),
    color_primary_bg_hover: hex(0xdce7fd),
    color_primary_border: hex(0xa9c3fa),
    color_success: hex(0x16a34a),
    color_success_hover: hex(0x22c55e),
    color_success_active: hex(0x15803d),
    color_success_bg: hex(0xf0fdf4),
    color_success_border: hex(0xbbf7d0),
    color_warning: hex(0xd97706),
    color_warning_hover: hex(0xf59e0b),
    color_warning_active: hex(0xb45309),
    color_warning_bg: hex(0xfffbeb),
    color_warning_border: hex(0xfde68a),
    color_error: hex(0xe5484d),
    color_error_hover: hex(0xeb6b6f),
    color_error_active: hex(0xc93a3f),
    color_error_bg: hex(0xfff1f1),
    color_error_border: hex(0xfbcaca),
    color_info: hex(0x2e6bf0),
    color_info_hover: hex(0x4b83f5),
    color_info_active: hex(0x1f55d1),
    color_info_bg: hex(0xeef3fe),
    color_info_border: hex(0xa9c3fa),
    color_link: hex(0x2e6bf0),
    color_link_hover: hex(0x4b83f5),
    color_link_active: hex(0x1f55d1),
    color_text: hex(0x1d1d21),
    color_text_secondary: hex(0x5b5b66),
    color_text_tertiary: hex(0x8a8a95),
    color_text_quaternary: hex(0xb6b6be),
    color_text_disabled: hex(0xb6b6be),
    color_text_description: hex(0x8a8a95),
    color_text_placeholder: hex(0xa3a3ad),
    color_text_light_solid: hex(0xffffff),
    color_bg_container: hex(0xffffff),
    color_bg_elevated: hex(0xffffff),
    color_bg_layout: hex(0xf4f4f6),
    color_bg_spotlight: rgba(0x1d1d21, 0.92),
    color_bg_mask: rgba(0x000000, 0.32),
    color_bg_text_hover: rgba(0x000000, 0.05),
    color_bg_text_active: rgba(0x000000, 0.09),
    color_border: hex(0xdfdfe4),
    color_border_secondary: hex(0xeaeaee),
    color_border_disabled: hex(0xd6d6dc),
    color_split: rgba(0x000000, 0.06),
    color_fill: rgba(0x000000, 0.1),
    color_fill_secondary: rgba(0x000000, 0.06),
    color_fill_tertiary: rgba(0x000000, 0.04),
    color_fill_quaternary: rgba(0x000000, 0.025),
    control_item_bg_hover: rgba(0x000000, 0.045),
    control_item_bg_active: hex(0xeef3fe),
    control_item_bg_active_hover: hex(0xdce7fd),
    blue_1: hex(0xeef3fe),
    blue_6: hex(0x2e6bf0),
    ..antd::LIGHT
};

/// 暗色：底是近黑的冷灰，卡片抬高一档，浮层再高一档。
pub const DARK: AntdColors = AntdColors {
    color_primary: hex(0x4d86f7),
    color_primary_hover: hex(0x6c9bf8),
    color_primary_active: hex(0x3a6fd8),
    color_primary_bg: rgba(0x4d86f7, 0.16),
    color_primary_bg_hover: rgba(0x4d86f7, 0.24),
    color_primary_border: rgba(0x4d86f7, 0.45),
    color_success: hex(0x2fbf62),
    color_success_hover: hex(0x4ade80),
    color_success_active: hex(0x16a34a),
    color_success_bg: rgba(0x2fbf62, 0.14),
    color_success_border: rgba(0x2fbf62, 0.4),
    color_warning: hex(0xf0a020),
    color_warning_hover: hex(0xfbbf24),
    color_warning_active: hex(0xd97706),
    color_warning_bg: rgba(0xf0a020, 0.14),
    color_warning_border: rgba(0xf0a020, 0.4),
    color_error: hex(0xf2555a),
    color_error_hover: hex(0xf5777b),
    color_error_active: hex(0xd8434a),
    color_error_bg: rgba(0xf2555a, 0.14),
    color_error_border: rgba(0xf2555a, 0.4),
    color_info: hex(0x4d86f7),
    color_info_hover: hex(0x6c9bf8),
    color_info_active: hex(0x3a6fd8),
    color_info_bg: rgba(0x4d86f7, 0.16),
    color_info_border: rgba(0x4d86f7, 0.45),
    color_link: hex(0x6c9bf8),
    color_link_hover: hex(0x8db2fa),
    color_link_active: hex(0x4d86f7),
    color_text: hex(0xececf0),
    color_text_secondary: hex(0xa8a8b3),
    color_text_tertiary: hex(0x7c7c88),
    color_text_quaternary: hex(0x55555e),
    color_text_disabled: hex(0x55555e),
    color_text_description: hex(0x7c7c88),
    color_text_placeholder: hex(0x6b6b76),
    color_text_light_solid: hex(0xffffff),
    color_bg_container: hex(0x1c1c20),
    color_bg_elevated: hex(0x26262b),
    color_bg_layout: hex(0x131315),
    color_bg_spotlight: hex(0x3a3a41),
    color_bg_mask: rgba(0x000000, 0.5),
    color_bg_text_hover: rgba(0xffffff, 0.07),
    color_bg_text_active: rgba(0xffffff, 0.12),
    color_border: hex(0x36363d),
    color_border_secondary: hex(0x2a2a30),
    color_border_disabled: hex(0x3a3a41),
    color_split: rgba(0xffffff, 0.07),
    color_fill: rgba(0xffffff, 0.14),
    color_fill_secondary: rgba(0xffffff, 0.09),
    color_fill_tertiary: rgba(0xffffff, 0.06),
    color_fill_quaternary: rgba(0xffffff, 0.035),
    control_item_bg_hover: rgba(0xffffff, 0.07),
    control_item_bg_active: rgba(0x4d86f7, 0.18),
    control_item_bg_active_hover: rgba(0x4d86f7, 0.26),
    blue_1: rgba(0x4d86f7, 0.16),
    blue_6: hex(0x4d86f7),
    ..antd::DARK
};

/// 亮色阴影：浮层是一层贴边细影加一层柔和的远影；卡片只有极淡的一层。
pub const LIGHT_SHADOWS: AntdShadows = AntdShadows {
    box_shadow: [
        layer(1., 2., 0., rgba(0x000000, 0.06)),
        layer(4., 12., 0., rgba(0x000000, 0.08)),
        layer(12., 32., -4., rgba(0x000000, 0.1)),
    ],
    box_shadow_tertiary: [
        layer(1., 2., 0., rgba(0x000000, 0.04)),
        layer(1., 1., 0., rgba(0x000000, 0.02)),
        layer(0., 0., 0., rgba(0x000000, 0.)),
    ],
};

/// 暗色阴影：黑色、更重，靠它和浅一档的底色一起把浮层抬起来。
pub const DARK_SHADOWS: AntdShadows = AntdShadows {
    box_shadow: [
        layer(1., 2., 0., rgba(0x000000, 0.3)),
        layer(4., 12., 0., rgba(0x000000, 0.35)),
        layer(12., 32., -4., rgba(0x000000, 0.45)),
    ],
    box_shadow_tertiary: [
        layer(1., 2., 0., rgba(0x000000, 0.2)),
        layer(0., 0., 0., rgba(0x000000, 0.)),
        layer(0., 0., 0., rgba(0x000000, 0.)),
    ],
};

/// 圆角（px）：控件 7、卡片与浮层 10。
pub const RADIUS_XS: f32 = 3.;
pub const RADIUS_SM: f32 = 5.;
pub const RADIUS: f32 = 7.;
pub const RADIUS_LG: f32 = 10.;
