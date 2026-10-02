//! 设置文件损坏或部分读不懂时 [`SettingsStore`] 的行为：历史设置回落时暂停自动清理。

use std::fs;

use kwikpaste_core::settings::SettingsStore;
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

    fn store(&self) -> SettingsStore {
        SettingsStore::new(&self.paths, None).unwrap()
    }
}

const CORRUPT: &str = "{\"general\": {\"autoStart\": true}, \"appearance\": ";

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
