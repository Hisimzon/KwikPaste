//! 来源应用：偏好页的应用过滤列表、手动添加与清理。

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::clipboard::apps_registry::{
    self, merge_clipboard_apps, refresh_running_apps, sort_clipboard_apps,
};
use crate::clipboard::AppIconStore;
use crate::db::models::{ClipboardApp, Platform};
use crate::error::Result;
use crate::root::Core;

/// 偏好页来源应用列表项：在数据库模型上补齐图标的绝对路径。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardAppView {
    pub id: String,
    pub name: String,
    pub icon_file: Option<String>,
    pub icon_path: Option<String>,
    pub platform: Platform,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// 把来源应用数据库模型转换为偏好页可直接渲染的视图模型。
fn build_clipboard_app_view(store: &AppIconStore, app: ClipboardApp) -> ClipboardAppView {
    let icon_path = app
        .icon_file
        .as_deref()
        .and_then(|name| store.icon_path(name).to_str().map(str::to_owned));

    ClipboardAppView {
        id: app.id,
        name: app.name,
        icon_file: app.icon_file,
        icon_path,
        platform: app.platform,
        created_at: app.created_at,
        updated_at: app.updated_at,
    }
}

impl Core {
    /// 按 id 批量取来源应用。
    pub async fn list_apps(&self, ids: Vec<String>) -> Result<Vec<ClipboardApp>> {
        let core = self.clone();
        self.hop(
            async move { crate::db::apps::list_apps_by_ids(&core.0.db.pool().await, &ids).await },
        )
        .await
    }

    /// 可过滤的应用：数据库里已知的应用加上本次枚举到的运行中应用，按名称排序。
    pub async fn list_all_apps(&self) -> Result<Vec<ClipboardAppView>> {
        if self.0.fixture_apps {
            return Ok(selftest_apps());
        }

        let core = self.clone();
        self.hop(async move {
            let running = refresh_running_apps(&core.0).await?;
            let known = crate::db::apps::list_all_apps(&core.0.db.pool().await).await?;
            let mut apps = merge_clipboard_apps(known, running);

            sort_clipboard_apps(&mut apps);
            Ok(apps
                .into_iter()
                .map(|app| build_clipboard_app_view(&core.0.app_icons, app))
                .collect())
        })
        .await
    }

    /// 从用户选择的应用路径（macOS `.app` / Windows `.exe`）登记来源应用。
    pub async fn add_app_from_path(&self, path: PathBuf) -> Result<ClipboardAppView> {
        let core = self.clone();
        self.hop(async move {
            let app = apps_registry::add_app_from_path(&core.0, path).await?;
            Ok(build_clipboard_app_view(&core.0.app_icons, app))
        })
        .await
    }

    /// 删除没有被历史记录引用的来源应用，返回实际删除的 id。
    pub async fn delete_unreferenced_apps(&self, ids: Vec<String>) -> Result<Vec<String>> {
        let core = self.clone();
        self.hop(async move { apps_registry::delete_unreferenced_apps(&core.0, ids).await })
            .await
    }
}

/// 自测里的来源应用必须不受宿主机当前进程清单影响，截图才能逐像素复现。
fn selftest_apps() -> Vec<ClipboardAppView> {
    [
        ("Fixture Browser", r"C:\KwikPaste\fixture-browser.exe"),
        ("Fixture Editor", r"C:\KwikPaste\fixture-editor.exe"),
        ("Fixture Mail", r"C:\KwikPaste\fixture-mail.exe"),
        ("Fixture Notes", r"C:\KwikPaste\fixture-notes.exe"),
        ("Fixture Terminal", r"C:\KwikPaste\fixture-terminal.exe"),
        ("Fixture Writer", r"C:\KwikPaste\fixture-writer.exe"),
    ]
    .into_iter()
    .map(|(name, id)| ClipboardAppView {
        id: id.to_owned(),
        name: name.to_owned(),
        icon_file: None,
        icon_path: None,
        platform: Platform::Windows,
        created_at: DateTime::UNIX_EPOCH,
        updated_at: DateTime::UNIX_EPOCH,
    })
    .collect()
}
