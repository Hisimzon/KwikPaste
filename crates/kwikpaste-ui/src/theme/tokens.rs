//! 语义层：应用读颜色和度量的唯一入口，对应 1.x UnoCSS 的 `ant-*` 颜色类与 wind4 数值。
//!
//! 颜色字段与 1.x 前端实际用到的 `text-ant-*` / `bg-ant-*` / `border-ant-*` 等类一一对应，
//! 每个字段注明来源的 antd token。带透明度的写法（`primary/10`、`ring primary/35`）在使用处
//! 写成 `.opacity(x)`。尺寸一律用 rem（基准 16 px，跟随文本缩放）。

use gpui::{BoxShadow, Hsla, Rems, rems};

use super::antd::{self, CubicBezier};

/// 应用可用的颜色 token。
///
/// 这是过渡兼容视图：所有字段在第三阶段会被角色化语义 token 替换。
#[derive(Clone, Debug, PartialEq)]
pub struct KpTokens {
    /// antd `colorText`（`text-ant-text`，全局正文色）。
    pub text: Hsla,
    /// antd `colorTextSecondary`（`text-ant-secondary`）。
    pub secondary: Hsla,
    /// antd `colorTextTertiary`（`text-ant-tertiary`）。
    pub tertiary: Hsla,
    /// antd `colorTextQuaternary`（`text-ant-quaternary`、`bg-ant-text-quaternary`）。
    pub quaternary: Hsla,
    /// antd `colorBorderDisabled`（`text-ant-disabled`）：1.x 的 presetAntdColors 按 token 顺序生成
    /// 别名，`ant-disabled` 先被 `colorBorderDisabled` 占用，所以 1.x 实际显示的是它而不是
    /// `colorTextDisabled`。这里照现状冻结。
    pub disabled: Hsla,
    /// antd `colorTextDescription`（`text-ant-description`）。
    pub description: Hsla,
    /// antd `colorTextPlaceholder`（输入框占位符）。
    pub placeholder: Hsla,
    /// antd `colorTextLightSolid`（`text-ant-light-solid`，深底上的白字）。
    pub light_solid: Hsla,

    /// antd `colorPrimary`（`text/bg/border/ring/fill-ant-primary`）。
    pub primary: Hsla,
    /// antd `colorPrimaryHover`。
    pub primary_hover: Hsla,
    /// antd `colorPrimaryActive`。
    pub primary_active: Hsla,
    /// antd `colorPrimaryBg`（`bg-ant-primary-bg`）。
    pub primary_bg: Hsla,
    /// antd `colorPrimaryBorder`（搜索框聚焦边框）。
    pub primary_border: Hsla,
    /// antd `colorSuccess`（`text/bg-ant-success`）。
    pub success: Hsla,
    /// antd `colorSuccessBg`。
    pub success_bg: Hsla,
    /// antd `colorSuccessBorder`。
    pub success_border: Hsla,
    /// antd `colorWarning`（`text/bg-ant-warning`）。
    pub warning: Hsla,
    /// antd `colorWarningHover`（`text-ant-warning-hover`）。
    pub warning_hover: Hsla,
    /// antd `colorWarningBg`。
    pub warning_bg: Hsla,
    /// antd `colorWarningBorder`。
    pub warning_border: Hsla,
    /// antd `colorError`（`text/bg-ant-error`）。
    pub error: Hsla,
    /// antd `colorErrorHover`。
    pub error_hover: Hsla,
    /// antd `colorErrorBg`。
    pub error_bg: Hsla,
    /// antd `colorErrorBorder`。
    pub error_border: Hsla,
    /// antd `colorInfo`。
    pub info: Hsla,
    /// antd `colorInfoHover`。
    pub info_hover: Hsla,
    /// antd `colorInfoActive`。
    pub info_active: Hsla,
    /// antd `colorInfoBg`。
    pub info_bg: Hsla,
    /// antd `colorInfoBorder`。
    pub info_border: Hsla,

    /// antd `colorBgContainer`（`bg-ant-container`）。
    pub bg_container: Hsla,
    /// antd `colorBgElevated`（`bg-ant-elevated`，浮层底色）。
    pub bg_elevated: Hsla,
    /// antd `colorBgLayout`（`bg-ant-bg-layout`）。
    pub bg_layout: Hsla,
    /// antd `colorBgSpotlight`（`bg-ant-bg-spotlight`，Tooltip 与 KeyHint 底色）。
    pub bg_spotlight: Hsla,
    /// antd `colorBgMask`（`bg-ant-mask`，弹窗遮罩）。
    pub mask: Hsla,
    /// antd `colorBgTextHover`（`bg-ant-text-hover`）。
    pub text_hover: Hsla,
    /// antd `colorWhite`（`--ant-color-white`，材质高光）。
    pub white: Hsla,

    /// antd `colorBorder`（`border-ant-border`）。
    pub border: Hsla,
    /// antd `colorBorderSecondary`（`border-ant-border-secondary`，卡片描边）。
    pub border_secondary: Hsla,
    /// antd `colorSplit`（`border/bg/stroke-ant-split`）。
    pub split: Hsla,
    /// antd `colorFill`。
    pub fill: Hsla,
    /// antd `colorFillSecondary`（`bg-ant-fill-secondary`）。
    pub fill_secondary: Hsla,
    /// antd `colorFillTertiary`（`bg/fill-ant-fill-tertiary`）。
    pub fill_tertiary: Hsla,
    /// antd `colorFillQuaternary`（`bg-ant-fill-quaternary`）。
    pub fill_quaternary: Hsla,

    /// antd `blue-1`（`bg-ant-blue-1`）。
    pub blue_1: Hsla,
    /// antd `blue-6`（`bg/fill-ant-blue-6`）。
    pub blue_6: Hsla,
    /// antd `cyan-6`（`bg/fill-ant-cyan-6`）。
    pub cyan_6: Hsla,
    /// antd `orange-6`（`bg/fill-ant-orange-6`）。
    pub orange_6: Hsla,
    /// antd `gold-3`（`bg-ant-gold-3`）。
    pub gold_3: Hsla,

    /// antd `boxShadow`：message、确认框、下拉等浮层。
    pub shadow_elevated: [BoxShadow; 3],
    /// antd `boxShadowTertiary`。
    pub shadow_card: [BoxShadow; 3],
}

impl KpTokens {
    /// 以材质不透明度生成面板内容层颜色。
    pub fn material_surface(&self, alpha: f32) -> Hsla {
        self.bg_container.opacity(alpha)
    }

    /// 从新的角色语义层构建第三阶段前的兼容视图。
    pub fn from_semantic(s: &super::semantic::SemanticTokens) -> Self {
        Self {
            text: s.text.primary,
            secondary: s.text.secondary,
            tertiary: s.text.muted,
            quaternary: s.text.faint,
            disabled: s.border_disabled,
            description: s.text.muted,
            placeholder: s.text.placeholder,
            light_solid: s.text.on_accent,
            primary: s.accent.solid,
            primary_hover: s.accent.hover,
            primary_active: s.accent.active,
            primary_bg: s.accent.subtle,
            primary_border: s.accent.border,
            success: s.status.success.solid,
            success_bg: s.status.success.subtle,
            success_border: s.status.success.border,
            warning: s.status.warning.solid,
            warning_hover: s.status.warning.hover,
            warning_bg: s.status.warning.subtle,
            warning_border: s.status.warning.border,
            error: s.status.danger.solid,
            error_hover: s.status.danger.hover,
            error_bg: s.status.danger.subtle,
            error_border: s.status.danger.border,
            info: s.status.info.solid,
            info_hover: s.status.info.hover,
            info_active: s.status.info.active,
            info_bg: s.status.info.subtle,
            info_border: s.status.info.border,
            bg_container: s.surface.panel,
            bg_elevated: s.surface.raised,
            bg_layout: s.surface.window,
            bg_spotlight: s.surface.spotlight,
            mask: s.surface.mask,
            text_hover: s.item.text_hover,
            white: s.white,
            border: s.border.default,
            border_secondary: s.border.subtle,
            split: s.border.divider,
            fill: s.fill.strong,
            fill_secondary: s.fill.default,
            fill_tertiary: s.fill.subtle,
            fill_quaternary: s.fill.faint,
            blue_1: s.hues.blue_1,
            blue_6: s.hues.blue_6,
            cyan_6: s.hues.cyan_6,
            orange_6: s.hues.orange_6,
            gold_3: s.hues.gold_3,
            shadow_elevated: s.shadow.overlay.clone(),
            shadow_card: s.shadow.card.clone(),
        }
    }

    /// 色板展示用的 `(字段名, antd token 名, 颜色)` 列表，顺序与字段一致。
    pub fn swatches(&self) -> [(&'static str, &'static str, Hsla); 38] {
        [
            ("text", "colorText", self.text),
            ("secondary", "colorTextSecondary", self.secondary),
            ("tertiary", "colorTextTertiary", self.tertiary),
            ("quaternary", "colorTextQuaternary", self.quaternary),
            ("disabled", "colorBorderDisabled", self.disabled),
            ("description", "colorTextDescription", self.description),
            ("placeholder", "colorTextPlaceholder", self.placeholder),
            ("light_solid", "colorTextLightSolid", self.light_solid),
            ("primary", "colorPrimary", self.primary),
            ("primary_hover", "colorPrimaryHover", self.primary_hover),
            ("primary_active", "colorPrimaryActive", self.primary_active),
            ("primary_bg", "colorPrimaryBg", self.primary_bg),
            ("primary_border", "colorPrimaryBorder", self.primary_border),
            ("success", "colorSuccess", self.success),
            ("warning", "colorWarning", self.warning),
            ("warning_hover", "colorWarningHover", self.warning_hover),
            ("error", "colorError", self.error),
            ("error_hover", "colorErrorHover", self.error_hover),
            ("info", "colorInfo", self.info),
            ("bg_container", "colorBgContainer", self.bg_container),
            ("bg_elevated", "colorBgElevated", self.bg_elevated),
            ("bg_layout", "colorBgLayout", self.bg_layout),
            ("bg_spotlight", "colorBgSpotlight", self.bg_spotlight),
            ("mask", "colorBgMask", self.mask),
            ("text_hover", "colorBgTextHover", self.text_hover),
            ("white", "colorWhite", self.white),
            ("border", "colorBorder", self.border),
            (
                "border_secondary",
                "colorBorderSecondary",
                self.border_secondary,
            ),
            ("split", "colorSplit", self.split),
            ("fill", "colorFill", self.fill),
            ("fill_secondary", "colorFillSecondary", self.fill_secondary),
            ("fill_tertiary", "colorFillTertiary", self.fill_tertiary),
            (
                "fill_quaternary",
                "colorFillQuaternary",
                self.fill_quaternary,
            ),
            ("blue_1", "blue-1", self.blue_1),
            ("blue_6", "blue-6", self.blue_6),
            ("cyan_6", "cyan-6", self.cyan_6),
            ("orange_6", "orange-6", self.orange_6),
            ("gold_3", "gold-3", self.gold_3),
        ]
    }
}
/// 标准语义字号，与 1.x UnoCSS wind4 一致：字号 / 行高（rem）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSize {
    /// `text-xs`：12 / 16 px。
    Xs,
    /// `text-sm`：14 / 20 px，正文（antd `fontSize`）。
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

/// 把 px 设计值换成 rem（基准 16 px）。
const fn rem_of(px: f32) -> Rems {
    Rems(px / 16.)
}

/// 圆角（[`super::palette`] 的 px 值换算成 rem，跟随文本缩放）。
pub mod radius {
    use gpui::Rems;

    use super::rem_of;
    use crate::theme::palette;

    /// 3 px：标记、细小色块。
    pub const XS: Rems = rem_of(palette::RADIUS_XS);
    /// 5 px：图片缩略图、小标签。
    pub const SM: Rems = rem_of(palette::RADIUS_SM);
    /// 7 px：控件默认圆角。
    pub const MD: Rems = rem_of(palette::RADIUS);
    /// 10 px：卡片、浮层。
    pub const LG: Rems = rem_of(palette::RADIUS_LG);
}

/// 控件高度（antd `controlHeight*`）。
pub mod control_height {
    use gpui::Rems;

    use super::{antd, rem_of};

    /// antd `controlHeightXS`：16 px。
    pub const XS: Rems = rem_of(antd::CONTROL_HEIGHT_XS);
    /// antd `controlHeightSM`：24 px。
    pub const SM: Rems = rem_of(antd::CONTROL_HEIGHT_SM);
    /// antd `controlHeight`：32 px。
    pub const MD: Rems = rem_of(antd::CONTROL_HEIGHT);
    /// antd `controlHeightLG`：40 px。
    pub const LG: Rems = rem_of(antd::CONTROL_HEIGHT_LG);
}

/// UnoCSS wind4 间距：`n` 个 0.25 rem（`p-1.5` 写成 `space(1.5)`）。
pub const fn space(n: f32) -> Rems {
    Rems(n * 0.25)
}

/// 动效时长与缓动。系统关闭动画（`cx.reduce_motion()`）时一律按 0 处理。
pub mod motion {
    use std::time::Duration;

    use super::{CubicBezier, antd};

    /// antd `motionDurationFast`：100 ms。
    pub const FAST: Duration = antd::MOTION_DURATION_FAST;
    /// antd `motionDurationMid`：200 ms（message 进出场）。
    pub const MID: Duration = antd::MOTION_DURATION_MID;
    /// antd `motionDurationSlow`：300 ms。
    pub const SLOW: Duration = antd::MOTION_DURATION_SLOW;
    /// 1.x `transition-colors`：卡片底色、边框、选中环 150 ms ease-out。
    pub const COLORS: Duration = Duration::from_millis(150);
    /// 1.x 快捷动作按钮弹出、拆词浮层：160 ms easeOut。
    pub const POP: Duration = Duration::from_millis(160);
    /// 1.x 便签与原文切换、材质过渡、预览窗开合：180 ms。
    pub const SWITCH: Duration = Duration::from_millis(180);
    /// antd `message` 默认停留 3 s。
    pub const TOAST: Duration = Duration::from_secs(3);

    /// antd `motionEaseOut`。
    pub const EASE_OUT: CubicBezier = antd::MOTION_EASE_OUT;
    /// antd `motionEaseInOut`。
    pub const EASE_IN_OUT: CubicBezier = antd::MOTION_EASE_IN_OUT;
    /// antd `motionEaseOutCirc`。
    pub const EASE_OUT_CIRC: CubicBezier = antd::MOTION_EASE_OUT_CIRC;
    /// motion/react 的 `easeOut`（1.x 自写动效）。
    pub const MOTION_EASE_OUT: CubicBezier = [0., 0., 0.58, 1.];
    /// 1.x 预览窗开合 `[0.22, 1, 0.36, 1]`。
    pub const PANEL: CubicBezier = [0.22, 1., 0.36, 1.];

    /// 按系统“减少动画”设置折算时长。
    pub fn duration(duration: Duration, reduce_motion: bool) -> Duration {
        if reduce_motion {
            return Duration::ZERO;
        }

        duration
    }
}
