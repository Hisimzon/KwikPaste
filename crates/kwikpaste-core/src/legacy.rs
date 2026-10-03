//! 1.x 留下、2.0 不再用的东西，在 2.0 第一次正常启动后清理一次。
//!
//! 现在只有 WebView2 的用户数据目录 `EBWebView`（1.x 界面的缓存，几十 MB）。日志目录不动：
//! 2.0 也往那里写日志。

use std::path::Path;

use anyhow::Context;
use chrono::Utc;

use crate::error::Result;
use crate::paths::CorePaths;

/// 清理过的标记，放在启动锚点（[`CorePaths::bootstrap_dir`]）：不随自定义数据目录搬走。
const WEBVIEW_MARKER: &str = "legacy-webview-removed";

/// [`remove_webview_data_once`] 做了什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebviewCleanup {
    /// 以前清理过（有标记），这次没有动。
    AlreadyDone,
    /// 没有 1.x 的 WebView2 数据，记下标记。
    NothingToRemove,
    /// 删掉了 1.x 的 WebView2 数据，记下标记。
    Removed,
}

/// 删掉 1.x 的 WebView2 数据目录，删完（或本来就没有）写下标记，以后的启动不再检查。
/// 删除失败时返回错误、不写标记，下次启动再试；1.x 又跑过、重新建了目录也不会再删。
pub fn remove_webview_data_once(paths: &CorePaths) -> Result<WebviewCleanup> {
    let marker = paths.bootstrap_dir().join(WEBVIEW_MARKER);
    if marker.exists() {
        return Ok(WebviewCleanup::AlreadyDone);
    }

    let outcome = match legacy_webview_dir(paths) {
        Some(dir) if dir.exists() => {
            std::fs::remove_dir_all(&dir)
                .with_context(|| format!("failed to remove {}", dir.display()))?;
            WebviewCleanup::Removed
        }
        _ => WebviewCleanup::NothingToRemove,
    };
    write_marker(&marker)?;
    Ok(outcome)
}

/// 1.x 的 WebView2 数据目录：安装版在 `<app_local_data>\EBWebView`，便携版在便携数据根下，
/// 与 `<env>\`、`logs\` 同级。
#[cfg(target_os = "windows")]
fn legacy_webview_dir(paths: &CorePaths) -> Option<std::path::PathBuf> {
    Some(paths.legacy_webview_dir())
}

// TODO(macos): 1.x 在 macOS 上的 WKWebView 数据在 `~/Library/WebKit/<id>` 与
// `~/Library/Caches/<id>/WebKit`，需要在 Mac 上确认布局后再清理。
#[cfg(not(target_os = "windows"))]
fn legacy_webview_dir(_paths: &CorePaths) -> Option<std::path::PathBuf> {
    None
}

fn write_marker(marker: &Path) -> Result<()> {
    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::write(marker, Utc::now().to_rfc3339())
        .with_context(|| format!("failed to write {}", marker.display()))?;
    Ok(())
}

#[cfg(all(test, target_os = "windows"))]
mod tests {
    use super::*;
    use crate::env::AppEnv;

    fn paths(root: &Path) -> CorePaths {
        CorePaths::new(AppEnv::Prod, root.to_path_buf(), root.join("logs"), None)
    }

    #[test]
    fn removes_the_webview_data_once_and_leaves_everything_else() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());
        let webview = root.path().join("EBWebView").join("Default");
        std::fs::create_dir_all(&webview).unwrap();
        std::fs::write(webview.join("Cookies"), b"x").unwrap();
        std::fs::create_dir_all(root.path().join("logs")).unwrap();
        std::fs::write(root.path().join("logs").join("KwikPaste.log"), b"log").unwrap();
        std::fs::create_dir_all(root.path().join("prod").join("db")).unwrap();

        assert_eq!(
            remove_webview_data_once(&paths).unwrap(),
            WebviewCleanup::Removed
        );
        assert!(!root.path().join("EBWebView").exists());
        assert!(root.path().join("logs").join("KwikPaste.log").exists());
        assert!(root.path().join("prod").join("db").exists());
        assert!(root.path().join("prod").join(WEBVIEW_MARKER).exists());

        // 1.x 又跑过、重新建了目录：有标记，不再删。
        std::fs::create_dir_all(&webview).unwrap();
        assert_eq!(
            remove_webview_data_once(&paths).unwrap(),
            WebviewCleanup::AlreadyDone
        );
        assert!(webview.exists());
    }

    #[test]
    fn records_the_marker_when_there_is_nothing_to_remove() {
        let root = tempfile::tempdir().unwrap();
        let paths = paths(root.path());

        assert_eq!(
            remove_webview_data_once(&paths).unwrap(),
            WebviewCleanup::NothingToRemove
        );
        assert_eq!(
            remove_webview_data_once(&paths).unwrap(),
            WebviewCleanup::AlreadyDone
        );
    }

    #[test]
    fn portable_mode_cleans_next_to_the_portable_data() {
        let root = tempfile::tempdir().unwrap();
        let portable = root.path().join("data");
        let paths = CorePaths::new(
            AppEnv::Prod,
            root.path().join("local"),
            root.path().join("logs"),
            Some(portable.clone()),
        );
        std::fs::create_dir_all(portable.join("EBWebView")).unwrap();

        assert_eq!(
            remove_webview_data_once(&paths).unwrap(),
            WebviewCleanup::Removed
        );
        assert!(!portable.join("EBWebView").exists());
        assert!(portable.join("prod").join(WEBVIEW_MARKER).exists());
    }
}
