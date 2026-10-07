//! 应用注册表：把「运行中应用」「监听过程中捕获的前台应用」「用户手动添加的应用」
//! 统一物化为可展示的应用记录，并维护一份 id → 应用的内存缓存。
//! 运行中应用只进缓存；复制捕获、手动添加和默认忽略物化的应用才写入 `clipboard_apps` 表。
//!
//! 扫描应用用的平台 API 由 [`PlatformServices`] 提供（`kwikpaste-os`），这里只管缓存、图标与入库。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use anyhow::anyhow;
use chrono::Utc;

use super::app_store::AppIconStore;
use super::icon::icon_png;
use crate::app_ids::app_family_key;
use crate::db::apps;
use crate::db::models::ClipboardApp;
use crate::error::{AppError, Result};
use crate::platform::{FrontmostApp, ScannedApp};
use crate::root::CoreInner;

#[derive(Clone, Default)]
pub struct AppsRegistry {
    cache: Arc<RwLock<HashMap<String, ClipboardApp>>>,
}

impl AppsRegistry {
    /// 把 DB 中已有的应用全部装进缓存，覆盖任何旧缓存内容。
    pub(crate) async fn load_from_db(&self, core: &CoreInner) -> Result<()> {
        let pool = core.db.pool().await;
        let all = apps::list_all_apps(&pool).await?;
        let mut cache = self.cache.write().expect("apps registry cache poisoned");
        cache.clear();
        for app in all {
            cache.insert(app.id.clone(), app);
        }
        Ok(())
    }

    /// 按 id 从内存缓存读取来源应用记录。
    pub fn get(&self, id: &str) -> Option<ClipboardApp> {
        self.cache
            .read()
            .expect("apps registry cache poisoned")
            .get(id)
            .cloned()
    }

    /// 写入或替换内存缓存中的来源应用记录。
    pub fn insert_into_cache(&self, app: ClipboardApp) {
        self.cache
            .write()
            .expect("apps registry cache poisoned")
            .insert(app.id.clone(), app);
    }

    /// 从内存缓存移除来源应用记录。
    pub fn remove_from_cache(&self, id: &str) {
        self.cache
            .write()
            .expect("apps registry cache poisoned")
            .remove(id);
    }
}

/// 把同步抓到的 [`FrontmostApp`] 拼成可入库的 [`ClipboardApp`]。
/// 图标抽取或落盘失败不阻断（仍保留应用名），仅 warn。
///
/// `registry` 命中缓存时直接复用，不抽图标；缓存未命中才从 `icon_source` 抽一次图标
/// （256px 抽取 + PNG 编码 + 落盘），并把结果回写缓存，让首次见到的应用后续直接命中。
pub fn materialize_source(
    store: &AppIconStore,
    registry: Option<&AppsRegistry>,
    src: FrontmostApp,
) -> ClipboardApp {
    if let Some(reg) = registry {
        if let Some(cached) = reg.get(&src.id) {
            return cached;
        }
    }

    let icon_file = src
        .icon_source
        .as_deref()
        .and_then(|path| icon_png(path, None))
        .and_then(|bytes| match store.store(&bytes) {
            Ok(name) => Some(name),
            Err(err) => {
                log::warn!("app icon store failed for {}: {err}", src.id);
                None
            }
        });
    let now = Utc::now();
    let app = ClipboardApp {
        id: src.id,
        name: src.name,
        icon_file,
        platform: src.platform,
        created_at: now,
        updated_at: now,
    };
    if let Some(reg) = registry {
        reg.insert_into_cache(app.clone());
    }
    app
}

/// 刷新当前运行中的用户应用，并返回本次枚举到的应用列表。必须在 core runtime 里调用。
pub(crate) async fn refresh_running_apps(core: &CoreInner) -> Result<Vec<ClipboardApp>> {
    let platform = core.platform();
    let metas = tokio::task::spawn_blocking(move || platform.running_apps())
        .await
        .map_err(|err| AppError::Other(anyhow!("running app refresh task join failed: {err}")))?;

    Ok(materialize_metas(core, metas))
}

/// 从用户选择的应用路径构建来源应用并写入注册表。必须在 core runtime 里调用。
pub(crate) async fn add_app_from_path(core: &CoreInner, path: PathBuf) -> Result<ClipboardApp> {
    let platform = core.platform();
    let meta = tokio::task::spawn_blocking(move || platform.app_from_path(&path))
        .await
        .map_err(|err| AppError::Other(anyhow!("app add task join failed: {err}")))??;
    let mut apps = upsert_metas(core, vec![meta]).await?;

    apps.pop()
        .ok_or_else(|| AppError::Clipboard("app metadata is empty".to_owned()))
}

/// 按应用 id 批量补全应用信息，返回成功物化的应用。必须在 core runtime 里调用。
pub(crate) async fn add_apps_from_ids(
    core: &CoreInner,
    ids: Vec<String>,
) -> Result<Vec<ClipboardApp>> {
    let platform = core.platform();
    let metas = tokio::task::spawn_blocking(move || {
        ids.into_iter()
            .filter_map(|id| platform.app_from_id(&id))
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|err| AppError::Other(anyhow!("app lookup task join failed: {err}")))?;

    upsert_metas(core, metas).await
}

/// 删除未被历史记录引用的来源应用，并同步移除注册表缓存。
pub(crate) async fn delete_unreferenced_apps(
    core: &CoreInner,
    ids: Vec<String>,
) -> Result<Vec<String>> {
    let pool = core.db.pool().await;
    let deleted = apps::delete_unreferenced_apps(&pool, &ids).await?;

    for id in &deleted {
        core.apps.remove_from_cache(id);
    }

    Ok(deleted)
}

/// 将元数据列表物化为应用记录，写入内存缓存并同步抽取图标。
fn materialize_metas(core: &CoreInner, metas: Vec<ScannedApp>) -> Vec<ClipboardApp> {
    let now = Utc::now();
    let mut apps_out = Vec::with_capacity(metas.len());

    for meta in metas {
        let existing_icon = core.apps.get(&meta.id).and_then(|app| app.icon_file);
        let icon_file = match existing_icon {
            Some(icon_file) => Some(icon_file),
            None => meta
                .path
                .as_deref()
                .and_then(|path| icon_png(path, None))
                .as_deref()
                .and_then(|bytes| match core.app_icons.store(bytes) {
                    Ok(name) => Some(name),
                    Err(err) => {
                        log::warn!("app icon store failed for {}: {err}", meta.id);
                        None
                    }
                }),
        };
        let app = ClipboardApp {
            id: meta.id,
            name: meta.name,
            icon_file,
            platform: meta.platform,
            created_at: now,
            updated_at: now,
        };
        core.apps.insert_into_cache(app.clone());
        apps_out.push(app);
    }

    apps_out
}

/// 将元数据列表写入 DB 与缓存。
async fn upsert_metas(core: &CoreInner, metas: Vec<ScannedApp>) -> Result<Vec<ClipboardApp>> {
    let apps = materialize_metas(core, metas);
    let pool = core.db.pool().await;

    for app in &apps {
        apps::upsert_app(&pool, app).await?;
    }

    Ok(apps)
}

/// 合并 DB 已知应用与运行中临时应用，同 id 时保留 DB 行（DB 行没有图标时借用运行中应用的图标）。
pub(crate) fn merge_clipboard_apps(
    known_apps: Vec<ClipboardApp>,
    running_apps: Vec<ClipboardApp>,
) -> Vec<ClipboardApp> {
    let mut merged = HashMap::with_capacity(known_apps.len() + running_apps.len());

    for app in running_apps {
        merged.insert(app.id.clone(), app);
    }
    for mut app in known_apps {
        if app.icon_file.is_none() {
            app.icon_file = merged
                .get(&app.id)
                .and_then(|running: &ClipboardApp| running.icon_file.clone());
        }
        merged.insert(app.id.clone(), app);
    }

    merged.into_values().collect()
}

/// 同一应用的多个安装版本（见 [`app_family_key`]）只留一项：最近更新的那个版本，运行中的应用
/// 带着本次枚举的时间，因而就是当前装着的版本；它没有图标时借同一应用其他版本的图标。
pub(crate) fn merge_app_versions(apps: Vec<ClipboardApp>) -> Vec<ClipboardApp> {
    let mut merged: Vec<ClipboardApp> = Vec::with_capacity(apps.len());
    let mut by_family: HashMap<String, usize> = HashMap::with_capacity(apps.len());

    for mut app in apps {
        let family = app_family_key(&app.id);
        let Some(&index) = by_family.get(&family) else {
            by_family.insert(family, merged.len());
            merged.push(app);
            continue;
        };
        let kept = &mut merged[index];
        if (app.updated_at, &app.id) > (kept.updated_at, &kept.id) {
            std::mem::swap(kept, &mut app);
        }
        if kept.icon_file.is_none() {
            kept.icon_file = app.icon_file;
        }
    }

    merged
}

/// 按名称（不区分大小写）和 id 稳定排序来源应用列表。
pub(crate) fn sort_clipboard_apps(apps: &mut [ClipboardApp]) {
    apps.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.id.cmp(&right.id))
    });
}

#[cfg(test)]
mod tests {
    use chrono::DateTime;

    use super::*;
    use crate::db::models::Platform;

    fn app(id: &str, name: &str, icon: Option<&str>) -> ClipboardApp {
        ClipboardApp {
            id: id.to_owned(),
            name: name.to_owned(),
            icon_file: icon.map(str::to_owned),
            platform: Platform::Windows,
            created_at: DateTime::UNIX_EPOCH,
            updated_at: DateTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn known_apps_win_but_borrow_running_icons() {
        let merged = merge_clipboard_apps(
            vec![
                app("a", "Known A", None),
                app("b", "Known B", Some("b.png")),
            ],
            vec![
                app("a", "Running A", Some("a.png")),
                app("c", "Running C", None),
            ],
        );
        let mut merged = merged;
        sort_clipboard_apps(&mut merged);

        let summary: Vec<_> = merged
            .iter()
            .map(|app| (app.id.as_str(), app.name.as_str(), app.icon_file.as_deref()))
            .collect();
        assert_eq!(
            summary,
            [
                ("a", "Known A", Some("a.png")),
                ("b", "Known B", Some("b.png")),
                ("c", "Running C", None),
            ]
        );
    }

    #[test]
    fn versions_of_one_app_merge_into_the_newest() {
        let store = |version: &str| {
            format!(
                r"C:\Program Files\WindowsApps\Claude_{version}_x64__pzs8sxrjxfjjc\app\claude.exe"
            )
        };
        let at = |seconds: i64| DateTime::from_timestamp(seconds, 0).unwrap();
        let merged = merge_app_versions(vec![
            ClipboardApp {
                updated_at: at(10),
                ..app(&store("2.19675.0.0"), "claude", Some("old.png"))
            },
            ClipboardApp {
                updated_at: at(30),
                ..app(&store("2.19675.1.0"), "claude", None)
            },
            ClipboardApp {
                updated_at: at(20),
                ..app(&store("2.19600.0.0"), "Claude", None)
            },
            app(r"C:\Windows\explorer.exe", "explorer", None),
        ]);

        let summary: Vec<_> = merged
            .iter()
            .map(|app| (app.id.clone(), app.icon_file.as_deref()))
            .collect();
        assert_eq!(
            summary,
            [
                (store("2.19675.1.0"), Some("old.png")),
                (r"C:\Windows\explorer.exe".to_owned(), None),
            ]
        );
    }

    #[test]
    fn apps_sort_by_name_case_insensitively_then_id() {
        let mut apps = vec![
            app("2", "beta", None),
            app("1", "Beta", None),
            app("3", "alpha", None),
        ];
        sort_clipboard_apps(&mut apps);

        let ids: Vec<_> = apps.iter().map(|app| app.id.as_str()).collect();
        assert_eq!(ids, ["3", "1", "2"]);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn materialize_source_extracts_icon_from_icon_source_on_cache_miss() {
        let dir = tempfile::tempdir().unwrap();
        let store = AppIconStore::for_test(dir.path().to_path_buf());
        let exe = std::env::current_exe().unwrap();
        let source = FrontmostApp {
            id: exe.display().to_string(),
            name: "test".to_owned(),
            platform: Platform::Windows,
            icon_source: Some(exe),
        };

        let app = materialize_source(&store, None, source.clone());
        let icon_file = app
            .icon_file
            .expect("expected an icon extracted from the exe");
        assert!(store.icon_path(&icon_file).exists());

        let without_path = FrontmostApp {
            icon_source: None,
            ..source
        };
        assert_eq!(
            materialize_source(&store, None, without_path).icon_file,
            None
        );
    }

    #[test]
    fn cached_sources_are_reused_without_icon_extraction() {
        let dir = tempfile::tempdir().unwrap();
        let store = AppIconStore::for_test(dir.path().join("app-icons"));
        let registry = AppsRegistry::default();
        registry.insert_into_cache(app("com.example.editor", "Editor", Some("cached.png")));

        let materialized = materialize_source(
            &store,
            Some(&registry),
            FrontmostApp {
                id: "com.example.editor".to_owned(),
                name: "Renamed".to_owned(),
                platform: Platform::Windows,
                icon_source: Some(PathBuf::from("C:/missing.exe")),
            },
        );
        assert_eq!(materialized.name, "Editor");
        assert_eq!(materialized.icon_file.as_deref(), Some("cached.png"));

        let fresh = materialize_source(
            &store,
            Some(&registry),
            FrontmostApp {
                id: "com.example.viewer".to_owned(),
                name: "Viewer".to_owned(),
                platform: Platform::Windows,
                icon_source: None,
            },
        );
        assert_eq!(fresh.icon_file, None);
        assert!(registry.get("com.example.viewer").is_some());
    }
}
