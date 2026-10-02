//! 把剪贴板里的 CSS 颜色串（core 的 `colorPreview`）解析成 GPUI 颜色，供颜色卡片的色块使用。
//!
//! 1.x 把这个串直接塞给 CSS `background`；原生版只认 core 检测器放行的常见写法：
//! `#rgb` / `#rgba` / `#rrggbb` / `#rrggbbaa`，`rgb()` / `rgba()`，`hsl()` / `hsla()`（逗号或空格语法，
//! 支持百分比和 `/ alpha`）。`hwb()`、`lab()`、`oklch()`、`color-mix()`、渐变等返回 `None`，
//! 卡片退回显示文本正文。颜色值是剪贴板数据，不是主题色，所以和主题一起放在唯一允许构造颜色的目录里。

use gpui::{Hsla, Rgba};

/// 解析一个 CSS 颜色值；不认识或越界的写法返回 `None`。
pub fn css_color(value: &str) -> Option<Hsla> {
    let value = value.trim();
    if let Some(hex) = value.strip_prefix('#') {
        return parse_hex(hex).map(Into::into);
    }

    let open = value.find('(')?;
    let name = value.get(..open)?.trim().to_ascii_lowercase();
    let body = value.get(open + 1..)?.strip_suffix(')')?;
    let (channels, alpha) = split_arguments(body)?;
    let alpha = match alpha {
        Some(alpha) => parse_alpha(alpha)?,
        None => 1.,
    };

    match name.as_str() {
        "rgb" | "rgba" => {
            let [r, g, b] = channels;
            Some(
                Rgba {
                    r: parse_rgb_channel(r)?,
                    g: parse_rgb_channel(g)?,
                    b: parse_rgb_channel(b)?,
                    a: alpha,
                }
                .into(),
            )
        }
        "hsl" | "hsla" => {
            let [h, s, l] = channels;
            Some(Hsla {
                h: parse_hue(h)?,
                s: parse_percent(s)?,
                l: parse_percent(l)?,
                a: alpha,
            })
        }
        _ => None,
    }
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }

    let digit = |ix: usize| -> Option<f32> {
        let text = hex.get(ix..ix + 1)?;
        let value = u8::from_str_radix(text, 16).ok()?;
        Some(f32::from(value * 17) / 255.)
    };
    let pair = |ix: usize| -> Option<f32> {
        let text = hex.get(ix..ix + 2)?;
        let value = u8::from_str_radix(text, 16).ok()?;
        Some(f32::from(value) / 255.)
    };

    let (r, g, b, a) = match hex.len() {
        3 => (digit(0)?, digit(1)?, digit(2)?, 1.),
        4 => (digit(0)?, digit(1)?, digit(2)?, digit(3)?),
        6 => (pair(0)?, pair(2)?, pair(4)?, 1.),
        8 => (pair(0)?, pair(2)?, pair(4)?, pair(6)?),
        _ => return None,
    };

    Some(Rgba { r, g, b, a })
}

/// 拆出三个通道和可选的 alpha：`a, b, c[, d]` 或 `a b c[ / d]`。
fn split_arguments(body: &str) -> Option<([&str; 3], Option<&str>)> {
    let (main, slash_alpha) = match body.split_once('/') {
        Some((main, alpha)) => (main, Some(alpha.trim())),
        None => (body, None),
    };

    let parts: Vec<&str> = if main.contains(',') {
        main.split(',').map(str::trim).collect()
    } else {
        main.split_whitespace().collect()
    };

    match (parts.as_slice(), slash_alpha) {
        ([a, b, c], alpha) => Some(([a, b, c], alpha)),
        ([a, b, c, d], None) => Some(([a, b, c], Some(d))),
        _ => None,
    }
}

fn parse_number(text: &str) -> Option<f32> {
    let value: f32 = text.trim().parse().ok()?;
    value.is_finite().then_some(value)
}

/// `0–255` 或百分比，夹到 0–1。
fn parse_rgb_channel(text: &str) -> Option<f32> {
    let value = match text.strip_suffix('%') {
        Some(percent) => parse_number(percent)? / 100.,
        None => parse_number(text)? / 255.,
    };

    Some(value.clamp(0., 1.))
}

/// 必须带 `%` 的饱和度、亮度，夹到 0–1。
fn parse_percent(text: &str) -> Option<f32> {
    let value = parse_number(text.strip_suffix('%')?)? / 100.;

    Some(value.clamp(0., 1.))
}

/// 小数或百分比，夹到 0–1。
fn parse_alpha(text: &str) -> Option<f32> {
    let value = match text.strip_suffix('%') {
        Some(percent) => parse_number(percent)? / 100.,
        None => parse_number(text)?,
    };

    Some(value.clamp(0., 1.))
}

/// 色相，默认单位 deg，另认 `turn` / `rad` / `grad`；换算成 0–1。
fn parse_hue(text: &str) -> Option<f32> {
    let text = text.trim().to_ascii_lowercase();
    let degrees = if let Some(value) = text.strip_suffix("deg") {
        parse_number(value)?
    } else if let Some(value) = text.strip_suffix("grad") {
        parse_number(value)? * 0.9
    } else if let Some(value) = text.strip_suffix("rad") {
        parse_number(value)?.to_degrees()
    } else if let Some(value) = text.strip_suffix("turn") {
        parse_number(value)? * 360.
    } else {
        parse_number(&text)?
    };

    Some(degrees.rem_euclid(360.) / 360.)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba_of(value: &str) -> Option<[u8; 4]> {
        let rgba: Rgba = css_color(value)?.into();
        let byte = |v: f32| (v * 255.).round() as u8;
        Some([byte(rgba.r), byte(rgba.g), byte(rgba.b), byte(rgba.a)])
    }

    #[test]
    fn hex_forms() {
        assert_eq!(rgba_of("#1677ff"), Some([0x16, 0x77, 0xff, 255]));
        assert_eq!(rgba_of("#1677FF80"), Some([0x16, 0x77, 0xff, 0x80]));
        assert_eq!(rgba_of("#f00"), Some([255, 0, 0, 255]));
        assert_eq!(rgba_of("#f008"), Some([255, 0, 0, 0x88]));
        assert_eq!(rgba_of("  #00ff00 "), Some([0, 255, 0, 255]));
        assert_eq!(rgba_of("#12345"), None);
        assert_eq!(rgba_of("#ggg"), None);
    }

    #[test]
    fn rgb_functions() {
        assert_eq!(rgba_of("rgb(22, 119, 255)"), Some([22, 119, 255, 255]));
        assert_eq!(rgba_of("rgba(22,119,255,0.5)"), Some([22, 119, 255, 128]));
        assert_eq!(rgba_of("rgb(22 119 255 / 50%)"), Some([22, 119, 255, 128]));
        assert_eq!(rgba_of("RGB(100%, 0%, 0%)"), Some([255, 0, 0, 255]));
        assert_eq!(rgba_of("rgb(300, -5, 0)"), Some([255, 0, 0, 255]));
        assert_eq!(rgba_of("rgb(1, 2)"), None);
        assert_eq!(rgba_of("rgb(a, b, c)"), None);
    }

    #[test]
    fn hsl_functions() {
        assert_eq!(rgba_of("hsl(0, 100%, 50%)"), Some([255, 0, 0, 255]));
        assert_eq!(
            rgba_of("hsl(120deg 100% 50% / 0.5)"),
            Some([0, 255, 0, 128])
        );
        assert_eq!(
            rgba_of("hsla(0.5turn, 100%, 50%, 1)"),
            Some([0, 255, 255, 255])
        );
        assert_eq!(rgba_of("hsl(-120, 100%, 50%)"), Some([0, 0, 255, 255]));
        assert_eq!(rgba_of("hsl(0, 100, 50)"), None);
    }

    #[test]
    fn unsupported_forms_fall_back() {
        assert_eq!(rgba_of("oklch(70% 0.1 200)"), None);
        assert_eq!(rgba_of("linear-gradient(red, blue)"), None);
        assert_eq!(rgba_of("red"), None);
        assert_eq!(rgba_of(""), None);
    }
}
