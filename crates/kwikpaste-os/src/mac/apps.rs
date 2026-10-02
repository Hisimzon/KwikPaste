//! macOS 的应用识别（1.x `clipboard/source.rs` 与 `apps_registry.rs` 的 `mod macos`）。
//! 应用 id 是 bundle id，显示名取本地化名称。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use kwikpaste_core::db::models::Platform;
use kwikpaste_core::platform::{FrontmostApp, ScannedApp};
use kwikpaste_core::{AppError, Result};
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace};

/// 当前前台应用。在剪贴板变化回调里同步调用；没有 bundle id 的进程（命令行子进程等）返回 `None`，
/// 避免主键不稳定。
pub fn frontmost_app() -> Option<FrontmostApp> {
    autoreleasepool(|_| {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        let id = app.bundleIdentifier()?.to_string();
        let name = app
            .localizedName()
            .map(|name| name.to_string())
            .unwrap_or_else(|| id.clone());

        Some(FrontmostApp {
            icon_source: bundle_path(&app),
            id,
            name,
            platform: Platform::Macos,
        })
    })
}

/// activationPolicy 为 Regular 的运行中应用，按 bundle id 去重。
pub fn running_apps() -> Vec<ScannedApp> {
    autoreleasepool(|_| {
        let apps = NSWorkspace::sharedWorkspace().runningApplications();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for app in apps.iter() {
            if app.activationPolicy() != NSApplicationActivationPolicy::Regular {
                continue;
            }
            let Some(id) = app.bundleIdentifier().map(|id| id.to_string()) else {
                continue;
            };
            if !seen.insert(id.clone()) {
                continue;
            }
            let name = app
                .localizedName()
                .map(|name| name.to_string())
                .unwrap_or_else(|| id.clone());
            out.push(ScannedApp {
                path: bundle_path(&app),
                id,
                name,
                platform: Platform::Macos,
            });
        }

        out
    })
}

/// 用户手动选择的应用：只接受 `.app` 包，读 Info.plist 与本地化名称。调用方已确认路径存在。
pub fn app_from_path(path: &Path) -> Result<ScannedApp> {
    if path.extension().and_then(|extension| extension.to_str()) != Some("app") {
        return Err(AppError::Clipboard(
            "please choose a macOS app bundle".to_owned(),
        ));
    }

    scan_app_bundle(path)
        .ok_or_else(|| AppError::Clipboard("app bundle metadata is invalid".to_owned()))
}

/// 按 bundle id 找 `.app`：先问 Spotlight，没建索引时回落到几个已知的系统应用路径。
pub fn app_from_id(id: &str) -> Option<ScannedApp> {
    known_app_bundle_paths(id)
        .iter()
        .find_map(|path| scan_app_bundle(path))
}

fn scan_app_bundle(path: &Path) -> Option<ScannedApp> {
    let info_path = path.join("Contents/Info.plist");
    let info = match plist::Value::from_file(&info_path) {
        Ok(value) => value,
        Err(err) => {
            log::debug!(
                "app bundle {} has no readable Info.plist: {err}",
                path.display()
            );
            return None;
        }
    };
    let dict = info.as_dictionary()?;
    let id = dict.get("CFBundleIdentifier")?.as_string()?.to_owned();
    let fallback_name = dict
        .get("CFBundleDisplayName")
        .and_then(|value| value.as_string())
        .or_else(|| dict.get("CFBundleName").and_then(|value| value.as_string()))
        .map(str::to_owned)
        .unwrap_or_else(|| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or(&id)
                .to_owned()
        });

    Some(ScannedApp {
        name: localized_bundle_name(path).unwrap_or(fallback_name),
        id,
        path: Some(path.to_path_buf()),
        platform: Platform::Macos,
    })
}

fn bundle_path(app: &NSRunningApplication) -> Option<PathBuf> {
    let path = app.bundleURL()?.path()?;

    Some(PathBuf::from(path.to_string()))
}

/// Finder 展示的本地化名称（`mdls kMDItemDisplayName`），拿不到时返回 `None`。
fn localized_bundle_name(path: &Path) -> Option<String> {
    let output = std::process::Command::new("/usr/bin/mdls")
        .args(["-name", "kMDItemDisplayName", "-raw"])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let raw = String::from_utf8_lossy(&output.stdout);
    let text = raw.trim().trim_end_matches('\0').trim();
    if text.is_empty() || text == "(null)" {
        return None;
    }
    let name = text.strip_suffix(".app").unwrap_or(text);

    (!name.is_empty()).then(|| name.to_owned())
}

fn known_app_bundle_paths(id: &str) -> Vec<PathBuf> {
    if let Some(path) = path_from_spotlight(id) {
        return vec![path];
    }

    match id {
        "com.apple.keychainaccess" => vec![PathBuf::from(
            "/System/Library/CoreServices/Applications/Keychain Access.app",
        )],
        "com.apple.Passwords" => vec![PathBuf::from("/System/Applications/Passwords.app")],
        _ => Vec::new(),
    }
}

fn path_from_spotlight(id: &str) -> Option<PathBuf> {
    let query = format!("kMDItemCFBundleIdentifier == \"{id}\"");
    let output = std::process::Command::new("/usr/bin/mdfind")
        .arg(query)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .find(|path| path.extension().and_then(|extension| extension.to_str()) == Some("app"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passwords_app_uses_localized_name() {
        let path = PathBuf::from("/System/Applications/Passwords.app");
        if !path.exists() {
            eprintln!("skipped: Passwords.app is not present on this macOS");
            return;
        }

        let localized = localized_bundle_name(&path);

        println!("Passwords localized: {localized:?}");
        assert!(localized.is_some());
    }

    #[test]
    fn keychain_access_can_be_found_by_bundle_id() {
        let Some(app) = app_from_id("com.apple.keychainaccess") else {
            eprintln!("skipped: Keychain Access.app is neither indexed nor at its known path");
            return;
        };
        let path = app.path.expect("Keychain Access has a bundle path");

        let png = kwikpaste_core::clipboard::icon_png(&path, None).expect("an app icon");

        assert_eq!(app.id, "com.apple.keychainaccess");
        assert!(png.len() > 100);
    }

    #[test]
    fn only_app_bundles_can_be_chosen() {
        let err = app_from_path(Path::new("/bin/ls")).unwrap_err();

        assert_eq!(err.to_string(), "please choose a macOS app bundle");
    }
}
