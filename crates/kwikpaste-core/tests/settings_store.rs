//! 设置文件损坏或部分读不懂时 [`SettingsStore`] 的写盘行为：先备份再覆盖、历史设置回落时暂停自动清理。

use std::fs;
use std::path::PathBuf;

use kwikpaste_core::settings::{SettingsStore, Theme};
use kwikpaste_core::{AppEnv, CorePaths};
use serde_json::json;

struct Fixture {
    _temp: tempfile::TempDir,
    paths: CorePaths,
}

impl Fixture {
    fn with_settings(content: &str) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("local");
        let paths = CorePaths::new(AppEnv::Prod, local.clone(), local.join("logs"), None);
        let config = paths.config_dir().unwrap();
        fs::create_dir_all(&config).unwrap();
        fs::write(config.join("settings.json"), content).unwrap();

        Self { _temp: temp, paths }
    }

    fn config_dir(&self) -> PathBuf {
        self.paths.config_dir().unwrap()
    }

    fn settings_file(&self) -> String {
        fs::read_to_string(self.config_dir().join("settings.json")).unwrap()
    }

    /// 配置目录里的损坏备份：文件名与内容。
    fn backups(&self) -> Vec<(String, String)> {
        let mut backups: Vec<(String, String)> = fs::read_dir(self.config_dir())
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter_map(|path| {
                let name = path.file_name()?.to_string_lossy().into_owned();
                name.starts_with("settings.json.corrupt-")
                    .then(|| (name, fs::read_to_string(&path).unwrap()))
            })
            .collect();
        backups.sort();
        backups
    }

    fn store(&self) -> SettingsStore {
        SettingsStore::new(&self.paths, None).unwrap()
    }
}

const CORRUPT: &str = "{\"general\": {\"autoStart\": true}, \"appearance\": ";

#[test]
fn corrupt_file_is_backed_up_once_before_the_first_save() {
    let fixture = Fixture::with_settings(CORRUPT);
    let store = fixture.store();

    assert!(store.load_report().unreadable);
    // 读盘不写盘：用户什么都不改，原文件原样留着。
    assert_eq!(fixture.settings_file(), CORRUPT);
    assert!(fixture.backups().is_empty());

    store
        .update(json!({"appearance": {"theme": "dark"}}))
        .unwrap();

    let backups = fixture.backups();
    assert_eq!(backups.len(), 1);
    assert_eq!(backups[0].1, CORRUPT);
    let name = &backups[0].0;
    let stamp = name.trim_start_matches("settings.json.corrupt-");
    assert_eq!(stamp.len(), "20261002-153045".len(), "{name}");
    assert!(stamp.chars().all(|c| c.is_ascii_digit() || c == '-'));

    let saved: serde_json::Value = serde_json::from_str(&fixture.settings_file()).unwrap();
    assert_eq!(saved["appearance"]["theme"], "dark");

    store
        .update(json!({"appearance": {"theme": "light"}}))
        .unwrap();
    assert_eq!(fixture.backups().len(), 1, "only the original is backed up");
    assert_eq!(store.snapshot().appearance.theme, Theme::Light);
}

#[test]
fn restoring_defaults_also_backs_up_a_corrupt_file() {
    let fixture = Fixture::with_settings("not json at all");
    let store = fixture.store();

    store.reset().unwrap();

    assert_eq!(fixture.backups().len(), 1);
    assert_eq!(fixture.backups()[0].1, "not json at all");
}

#[test]
fn field_level_fallback_does_not_create_a_backup() {
    let fixture =
        Fixture::with_settings(r#"{"appearance": {"theme": "sepia", "language": "en-US"}}"#);
    let store = fixture.store();
    assert!(!store.load_report().unreadable);

    store
        .update(json!({"general": {"autoStart": true}}))
        .unwrap();

    assert!(fixture.backups().is_empty());
    let saved: serde_json::Value = serde_json::from_str(&fixture.settings_file()).unwrap();
    assert_eq!(saved["appearance"]["language"], "en-US");
    assert_eq!(saved["appearance"]["theme"], "auto");
}

#[test]
fn cleanup_stays_paused_until_history_settings_are_saved() {
    let fixture = Fixture::with_settings(
        r#"{"clipboard": {"history": {"storageLimitMb": "lots", "storageLimitAction": "cleanup"}}}"#,
    );
    let store = fixture.store();
    assert!(store.cleanup_paused());

    store
        .update(json!({"general": {"trayIcon": false}}))
        .unwrap();
    assert!(store.cleanup_paused(), "unrelated saves keep the pause");

    store
        .update(json!({"clipboard": {"history": {"storageLimitMb": 2048}}}))
        .unwrap();
    assert!(!store.cleanup_paused());
    assert!(
        store.load_report().history_degraded(),
        "the load report still describes the file as it was read"
    );
}

#[test]
fn whole_file_corruption_pauses_cleanup_and_reset_resumes_it() {
    let fixture = Fixture::with_settings(CORRUPT);
    let store = fixture.store();
    assert!(store.cleanup_paused());

    store.reset().unwrap();

    assert!(!store.cleanup_paused());
}

#[test]
fn clean_files_never_pause_cleanup() {
    let fixture = Fixture::with_settings(r#"{"clipboard": {"history": {"maxCount": 10}}}"#);

    assert!(!fixture.store().cleanup_paused());
}
