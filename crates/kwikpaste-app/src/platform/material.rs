//! 窗口材质：设置 `appearance.material`（default / mica / acrylic），与 1.x 的
//! `src-tauri/src/window/material.rs` 相同的契约和门槛，DWM 细节在 `kwikpaste_os::win::material`。
//!
//! 设置值先收敛成「这台机器上真正生效的材质」（[`WindowMaterial::effective`]）：
//! - 系统不支持（Mica 要 Windows 11，Acrylic 要 Windows 10 1809）→ default，与 1.x 相同；
//! - 系统关了「透明效果」、开了高对比度 → default（1.x 前端的 `prefers-reduced-transparency`）；
//! - DirectComposition 被关掉（崩溃后的降级模式，或 `GPUI_DISABLE_DIRECT_COMPOSITION`）→ default：
//!   那时交换链没有 alpha。
//!
//! 生效的材质套到面板上：default 用 GPUI `Opaque`（拿回 ClearType）加圆角；mica / acrylic 用
//! `Transparent`，清掉 GPUI 的 accent 再设 DWM 背板（Windows 11 22H2 之前的 acrylic 用 GPUI
//! `Blurred`）。窗口深浅色跟随 `appearance.theme`（auto 跟系统），与 1.x `setTheme` 相同。
//! 设置变化、系统设置变化（透明效果、深浅色、高对比度）时重新套。这里是全应用唯一调用
//! `set_background_appearance` 的地方：别处调用会重设 accent，打掉 DWM 背板。
//!
//! # UI 怎么接
//! 窗口外层 gpui-component `Root` 的不透明底色在材质下已经去掉（`kwikpaste_ui::set_root_translucent`），
//! 根元素的底色按 [`current`]`(cx).effective` 选，订阅 `cx.observe_global::<WindowMaterial>()`：
//! - `Default`：不透明的 token 底色（如 `bg_layout`），窗口本身不透明；
//! - `Mica`：`bg_container` 58% 不透明度，叠 135° 渐变（`primary` 10% → 透明 42% → 白 5%），
//!   顶部 1 px 内阴影白 12%（1.x `global.scss` 的 `.kp-material-surface[data-material="mica"]`）；
//! - `Acrylic`：`bg_container` 34%，145° 渐变（`primary` 16% → `bg_container` 20% @48% → 白 10%），
//!   内阴影白 18%。
//!
//! 材质下不要用不透明的浮层盖住滚动内容：不透明底色在材质上是一块实色，模糊也盖不住下面滚过的行；
//! 需要一直可见的东西（置顶行、表头）放在滚动容器外面。
//!
//! macOS：default 用不透明窗口，mica / acrylic 用 `Blurred` 加 `NSVisualEffectView`；系统关闭
//! 透明效果或高对比度时回退 default。

use gpui::{App, Global, Window, WindowBackgroundAppearance};
use kwikpaste_core::settings::{Material, Theme};

use super::panel::Panel;
use super::system::SystemSignals;
use crate::core_host;

/// 当前窗口材质。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowMaterial {
    /// 设置值。
    pub requested: Material,
    /// 收敛后真正生效的材质：UI 按它选底色。
    pub effective: Material,
    /// 窗口是否用深色外观（Mica / Acrylic 的底色深浅）。
    pub dark: bool,
}

impl Global for WindowMaterial {}

impl WindowMaterial {
    /// 窗口是半透明的（有系统背板），根元素要用半透明底色。
    #[allow(dead_code, reason = "UI 接线用的接口，见本模块文档")]
    pub fn is_translucent(&self) -> bool {
        self.effective != Material::Default
    }
}

/// 当前窗口材质；平台层启动之前为 default。
#[allow(dead_code, reason = "UI 接线用的接口，见本模块文档")]
pub fn current(cx: &App) -> WindowMaterial {
    cx.try_global::<WindowMaterial>()
        .copied()
        .unwrap_or(WindowMaterial {
            requested: Material::Default,
            effective: Material::Default,
            dark: false,
        })
}

/// 按当前设置和系统设置重新计算材质，并套到面板上。设置或系统设置变化时调用。
pub fn apply(cx: &mut App) {
    let Some(core) = core_host::core(cx) else {
        return;
    };
    let appearance = core.settings().appearance;
    let signals = cx.try_global::<SystemSignals>().copied();
    let dark = match appearance.theme {
        Theme::Light => false,
        Theme::Dark => true,
        Theme::Auto => signals.is_some_and(|signals| signals.dark),
    };
    let effective = resolve(appearance.material, signals.as_ref());
    let material = WindowMaterial {
        requested: appearance.material,
        effective,
        dark,
    };
    let changed = cx.try_global::<WindowMaterial>() != Some(&material);
    cx.set_global(material);
    if changed {
        log::info!("window material: {material:?}");
        super::probe::material(&material);
    }

    if let Some(panel) = cx.try_global::<Panel>() {
        panel.request(super::panel::PanelCommand::SetMaterial(material));
    }
}

/// 设置值收敛成这台机器上能生效的材质。
fn resolve(requested: Material, signals: Option<&SystemSignals>) -> Material {
    if requested == Material::Default {
        return Material::Default;
    }
    if signals.is_some_and(|signals| !signals.transparency || signals.high_contrast)
        || std::env::var_os("GPUI_DISABLE_DIRECT_COMPOSITION").is_some()
        || crate::health::degraded()
    {
        return Material::Default;
    }
    if !supported(requested) {
        return Material::Default;
    }

    requested
}

#[cfg(target_os = "windows")]
fn supported(material: Material) -> bool {
    let support = kwikpaste_os::win::material::support(build());
    match material {
        Material::Default => true,
        Material::Mica => support.mica,
        Material::Acrylic => support.acrylic,
    }
}

#[cfg(target_os = "macos")]
fn supported(material: Material) -> bool {
    matches!(
        material,
        Material::Default | Material::Mica | Material::Acrylic
    )
}

#[cfg(target_os = "windows")]
fn build() -> u32 {
    static BUILD: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *BUILD.get_or_init(kwikpaste_os::win::material::build)
}

#[cfg(target_os = "windows")]
pub(super) fn apply_to_window(window: &mut Window, material: &WindowMaterial) {
    use kwikpaste_os::win::material::{
        Backdrop, acrylic_uses_backdrop, set_backdrop, set_dark_mode, set_round_corners,
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let hwnd = match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => handle.hwnd.get(),
        _ => return,
    };
    let build = build();
    let (appearance, backdrop) = match material.effective {
        Material::Default => (WindowBackgroundAppearance::Opaque, Backdrop::None),
        Material::Mica => (WindowBackgroundAppearance::Transparent, Backdrop::Mica),
        Material::Acrylic if acrylic_uses_backdrop(build) => {
            (WindowBackgroundAppearance::Transparent, Backdrop::Acrylic)
        }
        Material::Acrylic => (WindowBackgroundAppearance::Blurred, Backdrop::None),
    };

    // 顺序要紧：GPUI 先设 accent，再由我们清 accent、设深浅色、设背板。
    window.set_background_appearance(appearance);
    set_dark_mode(hwnd, material.dark);
    set_backdrop(hwnd, backdrop, build);
    set_round_corners(hwnd);
    window.refresh();
}

#[cfg(target_os = "macos")]
pub(super) fn apply_to_window(window: &mut Window, material: &WindowMaterial) {
    let appearance = if material.effective == Material::Default {
        WindowBackgroundAppearance::Opaque
    } else {
        WindowBackgroundAppearance::Blurred
    };
    window.set_background_appearance(appearance);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals(transparency: bool, high_contrast: bool) -> SystemSignals {
        SystemSignals {
            text_scale: 1.0,
            high_contrast,
            reduce_motion: false,
            transparency,
            dark: false,
        }
    }

    #[test]
    fn reduced_transparency_and_high_contrast_fall_back_to_default() {
        assert_eq!(
            resolve(Material::Mica, Some(&signals(false, false))),
            Material::Default
        );
        assert_eq!(
            resolve(Material::Acrylic, Some(&signals(true, true))),
            Material::Default
        );
        assert_eq!(
            resolve(Material::Default, Some(&signals(true, false))),
            Material::Default
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn supported_materials_pass_through_on_this_machine() {
        // 本机是 Windows 11（build ≥ 22000）。
        if build() >= 22000 {
            assert_eq!(
                resolve(Material::Mica, Some(&signals(true, false))),
                Material::Mica
            );
            assert_eq!(
                resolve(Material::Acrylic, Some(&signals(true, false))),
                Material::Acrylic
            );
        }
    }
}
