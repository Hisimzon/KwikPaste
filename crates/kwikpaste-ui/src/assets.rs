//! 资源源：gpui-kit-assets 的默认图标集（组件自带的勾选、下拉箭头等）加上 `icons/` 下从 1.x
//! iconify 包导出的图标（见 `icons/export-icons.mjs`）。只内嵌用到的 SVG，不整套打包 lucide。

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

/// 自带图标的路径前缀，与 gpui-kit-assets 的 `icons/` 区分开。
pub(crate) const PREFIX: &str = "kp-icons/";

/// `icons/` 目录下内嵌的 SVG：`(资源路径, 内容)`。
const EMBEDDED: &[(&str, &[u8])] = &[
    (
        "kp-icons/lucide-image-off.svg",
        include_bytes!("../icons/lucide-image-off.svg"),
    ),
    (
        "kp-icons/lucide-key-round.svg",
        include_bytes!("../icons/lucide-key-round.svg"),
    ),
    (
        "kp-icons/lucide-laptop.svg",
        include_bytes!("../icons/lucide-laptop.svg"),
    ),
    (
        "kp-icons/lucide-monitor.svg",
        include_bytes!("../icons/lucide-monitor.svg"),
    ),
    (
        "kp-icons/lucide-notebook-pen.svg",
        include_bytes!("../icons/lucide-notebook-pen.svg"),
    ),
    (
        "kp-icons/ph-push-pin-bold.svg",
        include_bytes!("../icons/ph-push-pin-bold.svg"),
    ),
];

/// 应用的资源源。创建 `Application` 时用 `.with_assets(kwikpaste_ui::Assets)` 装上，
/// 否则组件和卡片里的图标是空的。
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if path.starts_with(PREFIX) {
            return Ok(EMBEDDED
                .iter()
                .find(|(embedded, _)| *embedded == path)
                .map(|(_, bytes)| Cow::Borrowed(*bytes)));
        }

        gpui_kit_assets::Assets.load(path)
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit_assets::Assets.list(path)?;
        paths.extend(
            EMBEDDED
                .iter()
                .filter(|(embedded, _)| embedded.starts_with(path))
                .map(|(embedded, _)| SharedString::from(*embedded)),
        );

        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icons_load_and_parse_as_svg() {
        for (path, _) in EMBEDDED {
            let bytes = Assets
                .load(path)
                .ok()
                .flatten()
                .expect("embedded icon loads");
            let text = std::str::from_utf8(&bytes).expect("svg is utf-8");
            assert!(text.starts_with("<svg"), "{path} is an svg");
        }
    }

    #[test]
    fn kit_icons_still_load() {
        let bytes = Assets.load("icons/check.svg").ok().flatten();
        assert!(bytes.is_some_and(|bytes| !bytes.is_empty()));
    }
}
