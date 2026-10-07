//! 冻结表比对：Rust 常量必须与 `theme/antd_tokens_light_dark.json` 精确相等。

use std::time::Duration;

use serde_json::Value;

use super::antd::{self, ShadowLayer, TokenColor};
use super::*;

const FROZEN: &str = include_str!("../../theme/antd_tokens_light_dark.json");

fn frozen() -> Value {
    serde_json::from_str(FROZEN).expect("the frozen token table is valid JSON")
}

fn token<'a>(json: &'a Value, mode: &str, name: &str) -> &'a Value {
    json.get(mode)
        .and_then(|tokens| tokens.get(name))
        .unwrap_or_else(|| panic!("{mode}.{name} is in the frozen table"))
}

/// 解析 antd 输出的颜色：`#rgb`、`#rrggbb`、`rgb(r,g,b)`、`rgba(r,g,b,a)`。
fn parse_color(value: &str) -> TokenColor {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        let hex = match hex.len() {
            3 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
            6 => hex.to_string(),
            _ => panic!("unexpected hex color {value}"),
        };
        let rgb = u32::from_str_radix(&hex, 16).expect("hex digits");
        return TokenColor { rgb, alpha: 1. };
    }

    let inner = value
        .strip_prefix("rgba(")
        .or_else(|| value.strip_prefix("rgb("))
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or_else(|| panic!("unexpected color {value}"));
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let channel = |ix: usize| -> u32 { parts[ix].parse().expect("a 0-255 channel") };
    let alpha = parts
        .get(3)
        .map_or(1., |alpha| alpha.parse().expect("alpha"));

    TokenColor {
        rgb: (channel(0) << 16) | (channel(1) << 8) | channel(2),
        alpha,
    }
}

/// 解析 antd 的多层 `box-shadow`（水平偏移恒为 0）。
fn parse_shadow(value: &str) -> Vec<ShadowLayer> {
    let length = |part: &str| -> f32 { part.trim_end_matches("px").parse().expect("a px length") };

    value
        .split("),")
        .map(|layer| {
            let layer = layer.trim();
            let (geometry, color) = layer.split_once("rgba").expect("an rgba() color");
            let numbers: Vec<&str> = geometry.split_whitespace().collect();
            assert_eq!(numbers.len(), 4, "x y blur spread in {layer}");
            assert_eq!(length(numbers[0]), 0., "antd shadows have no x offset");
            let color = format!("rgba{}", color.trim_end_matches(')'));

            ShadowLayer {
                y: length(numbers[1]),
                blur: length(numbers[2]),
                spread: length(numbers[3]),
                color: parse_color(&format!("{color})")),
            }
        })
        .collect()
}

#[test]
fn theme_tokens_match_frozen_json() {
    let json = frozen();
    for (mode, colors) in [("light", antd::LIGHT), ("dark", antd::DARK)] {
        for (name, value) in colors.entries() {
            let frozen = token(&json, mode, name).as_str().expect("a color string");
            assert_eq!(value, parse_color(frozen), "{mode}.{name} = {frozen}");
        }
    }
}

#[test]
fn shadows_match_frozen_json() {
    let json = frozen();
    for (mode, shadows) in [("light", antd::LIGHT_SHADOWS), ("dark", antd::DARK_SHADOWS)] {
        for (name, layers) in [
            ("boxShadow", shadows.box_shadow),
            ("boxShadowSecondary", shadows.box_shadow),
            ("boxShadowTertiary", shadows.box_shadow_tertiary),
        ] {
            let frozen = token(&json, mode, name).as_str().expect("a shadow string");
            assert_eq!(layers.to_vec(), parse_shadow(frozen), "{mode}.{name}");
        }
    }
}

#[test]
fn numbers_match_frozen_json() {
    let json = frozen();
    for mode in ["light", "dark"] {
        for (name, value) in antd::NUMBERS {
            let frozen = token(&json, mode, name).as_f64().expect("a number");
            assert_eq!(*value, frozen, "{mode}.{name}");
        }
    }
}

#[test]
fn motion_matches_frozen_json() {
    let json = frozen();
    for mode in ["light", "dark"] {
        for (name, duration) in antd::DURATIONS {
            let frozen = token(&json, mode, name).as_str().expect("a duration");
            let seconds: f64 = frozen
                .strip_suffix('s')
                .and_then(|seconds| seconds.parse().ok())
                .expect("seconds like 0.1s");
            assert_eq!(*duration, Duration::from_secs_f64(seconds), "{mode}.{name}");
        }
        for (name, easing) in antd::EASINGS {
            let frozen = token(&json, mode, name).as_str().expect("an easing");
            let points: Vec<f32> = frozen
                .trim_start_matches("cubic-bezier(")
                .trim_end_matches(')')
                .split(',')
                .map(|point| point.trim().parse().expect("a control point"))
                .collect();
            assert_eq!(easing.to_vec(), points, "{mode}.{name}");
        }
    }
}

#[test]
fn the_frozen_table_is_antd_6_6_0() {
    let json = frozen();
    assert_eq!(json["antdVersion"], "6.6.0");
    for mode in ["light", "dark"] {
        assert_eq!(json[mode].as_object().map(|tokens| tokens.len()), Some(536));
    }
}

#[test]
fn semantic_tokens_follow_the_palette() {
    for (appearance, colors) in [
        (Appearance::Light, palette::LIGHT),
        (Appearance::Dark, palette::DARK),
    ] {
        let tokens = tokens_for(appearance);
        assert_eq!(tokens.text, colors.text.primary.hsla());
        assert_eq!(tokens.secondary, colors.text.secondary.hsla());
        assert_eq!(tokens.primary, colors.accent.solid.hsla());
        assert_eq!(tokens.bg_spotlight, colors.surfaces.spotlight.hsla());
        // `disabled` 沿用 1.x 的取法：`text-ant-disabled` 实际解析到 `colorBorderDisabled`。
        assert_eq!(tokens.disabled, colors.borders.disabled.hsla());

        let swatches = tokens.swatches();
        let mut names: Vec<&str> = swatches.iter().map(|(name, _, _)| *name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), swatches.len(), "swatch names are unique");
    }
}

#[test]
fn role_tokens_preserve_compatibility_values() {
    let compat = tokens_for(Appearance::Light);
    let semantic = semantic::SemanticTokens::from_palette(&palette::LIGHT);
    assert_eq!(semantic.surface.window, compat.bg_layout);
    assert_eq!(semantic.surface.panel, compat.bg_container);
    assert_eq!(semantic.text.primary, compat.text);
    assert_eq!(semantic.accent.solid, compat.primary);
    assert_eq!(semantic.border.divider, compat.split);
}

#[test]
fn token_colors_convert_exactly() {
    let primary = antd::LIGHT.color_primary.to_hsla();
    let rgba = gpui::Rgba::from(primary);
    assert!((rgba.r - f32::from(0x16_u8) / 255.).abs() < 1e-4);
    assert!((rgba.g - f32::from(0x77_u8) / 255.).abs() < 1e-4);
    assert!((rgba.b - 1.).abs() < 1e-4);
    assert_eq!(antd::DARK.color_text.to_hsla().a, 0.85);
}

#[test]
fn preference_resolves_against_the_system() {
    assert_eq!(
        resolve(ThemePreference::System, Appearance::Dark),
        Appearance::Dark
    );
    assert_eq!(
        resolve(ThemePreference::Light, Appearance::Dark),
        Appearance::Light
    );
    assert_eq!(
        resolve(ThemePreference::Dark, Appearance::Light),
        Appearance::Dark
    );
}

#[test]
fn text_scale_is_clamped_like_1x() {
    assert_eq!(clamp_text_scale(0.5), 1.);
    assert_eq!(clamp_text_scale(1.5), 1.5);
    assert_eq!(clamp_text_scale(3.), 2.25);
    assert_eq!(clamp_text_scale(f32::NAN), 1.);
}

#[test]
fn type_scale_matches_wind4() {
    let pairs: Vec<(f32, f32)> = TextSize::ALL
        .iter()
        .map(|size| (size.font_size().0 * 16., size.line_height().0 * 16.))
        .collect();
    assert_eq!(
        pairs,
        [(12., 16.), (14., 20.), (16., 24.), (18., 28.)].to_vec()
    );
    assert_eq!(TextSize::Sm.font_size().0 * 16., antd::FONT_SIZE);
    assert_eq!(radius::MD.0 * 16., palette::RADIUS);
    assert_eq!(control_height::MD.0 * 16., antd::CONTROL_HEIGHT);
    assert_eq!(space(1.5).0, 0.375);
}

#[test]
fn token_sets_are_interned() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        assert!(std::ptr::eq(tokens_for(appearance), tokens_for(appearance)));
        assert!(std::ptr::eq(
            semantic_for(appearance),
            semantic_for(appearance)
        ));
        assert!(std::ptr::eq(
            components_for(appearance),
            components_for(appearance)
        ));
    }
}
