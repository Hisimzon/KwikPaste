use super::*;
use crate::testing::{block_on, Fixture};

fn staged(temp: &tempfile::TempDir, name: &str, bytes: &[u8]) -> PathBuf {
    let path = temp.path().join(name);
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn installed_json_roundtrip() {
    let temp = tempfile::tempdir().unwrap();
    let store = ExtensionStore::load(temp.path().join("extensions"));
    let entries = BTreeMap::from([(
        "ocr".into(),
        InstalledExtension {
            version: "1.0.0".into(),
            protocol: OCR_PROTOCOL,
            enabled: true,
        },
    )]);
    store.save(entries.clone()).unwrap();
    let loaded = ExtensionStore::load(store.dir.clone());
    assert_eq!(loaded.list(), entries);
    let json: serde_json::Value =
        serde_json::from_slice(&fs::read(store.dir.join("installed.json")).unwrap()).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"ocr":{"version":"1.0.0","protocol":1,"enabled":true}})
    );
}

#[test]
fn load_ignores_corrupt_registry_without_failing() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("extensions");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("installed.json"), b"not json").unwrap();

    let store = ExtensionStore::load(dir);

    assert!(store.list().is_empty());
}

#[test]
fn load_accepts_new_fields_and_drops_unknown_ids_and_invalid_versions() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("extensions");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("installed.json"),
        br#"{
            "ocr":{"version":"1.0.0","protocol":1,"enabled":true,"future_field":true},
            "unknown":{"version":"1.0.0","protocol":1,"enabled":true}
        }"#,
    )
    .unwrap();

    let store = ExtensionStore::load(dir);

    assert_eq!(store.list().len(), 1);
    assert_eq!(store.list()["ocr"].version, "1.0.0");

    fs::write(
        store.dir.join("installed.json"),
        br#"{"ocr":{"version":"not-a-version","protocol":1,"enabled":true}}"#,
    )
    .unwrap();
    assert!(ExtensionStore::load(store.dir.clone()).list().is_empty());
}

#[test]
fn resolution_requires_installed_enabled_compatible_and_present_file() {
    let temp = tempfile::tempdir().unwrap();
    let store = ExtensionStore::load(temp.path().join("extensions"));
    assert!(store.resolve("ocr").is_none());
    assert!(store.resolve("../ocr").is_none());
    let source = staged(&temp, "staged", b"first");
    store
        .install("ocr", "1.0.0", OCR_PROTOCOL, &source)
        .unwrap();
    let exe = store.resolve("ocr").unwrap();
    store.set_enabled("ocr", false).unwrap();
    assert!(store.resolve("ocr").is_none());
    store.set_enabled("ocr", true).unwrap();
    let mut entries = store.list();
    entries.get_mut("ocr").unwrap().protocol += 1;
    store.save(entries).unwrap();
    assert!(store.resolve("ocr").is_none());
    let mut entries = store.list();
    entries.get_mut("ocr").unwrap().protocol = OCR_PROTOCOL;
    store.save(entries).unwrap();
    fs::remove_file(exe).unwrap();
    assert!(store.resolve("ocr").is_none());
}

#[test]
fn install_replaces_version_preserves_state_and_consumes_stage() {
    let temp = tempfile::tempdir().unwrap();
    let store = ExtensionStore::load(temp.path().join("extensions"));
    let source = staged(&temp, "first", b"first");
    store
        .install("ocr", "1.0.0", OCR_PROTOCOL, &source)
        .unwrap();
    assert!(!source.exists());
    let old = store.resolve("ocr").unwrap().parent().unwrap().to_owned();
    let state = store.dir.join("ocr/state/identity");
    fs::write(&state, b"retained").unwrap();
    store.set_enabled("ocr", false).unwrap();
    let source = staged(&temp, "second", b"second");
    store
        .install("ocr", "1.1.0", OCR_PROTOCOL, &source)
        .unwrap();
    assert!(!store.list()["ocr"].enabled);
    assert!(store.resolve("ocr").is_none());
    store.set_enabled("ocr", true).unwrap();
    assert_eq!(fs::read(store.resolve("ocr").unwrap()).unwrap(), b"second");
    assert!(!old.exists());
    assert_eq!(fs::read(state).unwrap(), b"retained");
    assert_eq!(store.list()["ocr"].version, "1.1.0");
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(store.resolve("ocr").unwrap())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }
}

#[test]
fn uninstall_removes_state_and_all_versions() {
    let temp = tempfile::tempdir().unwrap();
    let store = ExtensionStore::load(temp.path().join("extensions"));
    let source = staged(&temp, "first", b"first");
    store
        .install("ocr", "1.0.0", OCR_PROTOCOL, &source)
        .unwrap();
    fs::write(store.dir.join("ocr/state/data"), b"private state").unwrap();
    store.uninstall("ocr").unwrap();
    assert!(!store.dir.join("ocr").exists());
    assert!(store.list().is_empty());
    assert!(ExtensionStore::load(store.dir.clone()).list().is_empty());
    store.uninstall("ocr").unwrap();
}

#[test]
fn unknown_ids_and_unsafe_versions_do_not_touch_staged_file() {
    let temp = tempfile::tempdir().unwrap();
    let store = ExtensionStore::load(temp.path().join("extensions"));
    let source = staged(&temp, "first", b"first");
    assert!(store
        .install("../ocr", "1.0.0", OCR_PROTOCOL, &source)
        .is_err());
    assert!(store
        .install("ocr", "../state", OCR_PROTOCOL, &source)
        .is_err());
    assert!(source.exists());
    assert!(!store.dir.exists());
}

/// 此处无法使用依赖 crate 的 CARGO_BIN_EXE 变量，因此从当前测试 profile 目录查找二进制。
fn ocr_binary() -> PathBuf {
    let test = std::env::current_exe().unwrap();
    let deps = test.parent().unwrap();
    let profile = if deps.file_name().is_some_and(|name| name == "deps") {
        deps.parent().unwrap()
    } else {
        deps
    };
    let exe = profile.join(executable_name("ocr").unwrap());
    assert!(
        exe.is_file(),
        "build kwikpaste-ext-ocr for the current profile before testing: {}",
        exe.display()
    );
    exe
}

#[test]
fn real_ocr_process_frames_timeout_and_graceful_stop() {
    use kwikpaste_ext_protocol::{Request, Response};
    use std::time::Duration;
    let mut host = process::ProcessHost::<Request, Response>::start("ocr", &ocr_binary()).unwrap();
    assert!(matches!(
        host.request(Request::Probe, Duration::from_secs(60))
            .unwrap(),
        Response::Probe { .. }
    ));
    let child = host.child.clone();
    drop(host);
    assert!(child.lock().unwrap().try_wait().unwrap().is_some());
    let mut host = process::ProcessHost::<Request, Response>::start("ocr", &ocr_binary()).unwrap();
    let child = host.child.clone();
    assert_eq!(
        host.request(Request::Probe, Duration::ZERO)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::TimedOut
    );
    drop(host);
    assert!(child.lock().unwrap().try_wait().unwrap().is_some());
}

#[test]
fn real_ocr_install_disable_reinstall_and_uninstall_take_effect_without_restart() {
    use crate::clipboard::{ClipboardPayload, ImagePayload};
    let fixture = Fixture::new();
    let core = fixture.start();
    assert_eq!(
        block_on(core.ocr_support()).unwrap(),
        crate::OcrSupport::NotInstalled
    );
    let staged = fixture.root().join("ocr-staged");
    fs::copy(ocr_binary(), &staged).unwrap();
    block_on(core.install_extension("ocr", "1.0.0", OCR_PROTOCOL, &staged)).unwrap();
    assert!(core.ocr_enabled());
    assert!(matches!(
        block_on(core.ocr_support()).unwrap(),
        crate::OcrSupport::Available { .. }
    ));
    block_on(core.set_extension_enabled("ocr", false)).unwrap();
    assert!(!core.ocr_enabled());
    let bytes =
        include_bytes!("../../../kwikpaste-ext-ocr/fixtures/ocr/chinese-english.png").to_vec();
    let decoded = image::load_from_memory(&bytes).unwrap();
    let payload = ClipboardPayload::Image(ImagePayload {
        bytes,
        width: decoded.width(),
        height: decoded.height(),
    });
    let item = core.build_item(&payload).unwrap().unwrap();
    block_on(core.store_item(item.clone(), None)).unwrap();
    assert_eq!(block_on(core.ocr_status()).unwrap().pending, 1);
    assert!(!block_on(core.ocr_status()).unwrap().running);
    block_on(core.set_extension_enabled("ocr", true)).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
    loop {
        let status = block_on(core.ocr_status()).unwrap();
        if !status.running && status.pending == 0 {
            assert_eq!(status.with_text, 1);
            break;
        }
        assert!(std::time::Instant::now() < deadline, "{status:?}");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(block_on(core.image_text(&item.id))
        .unwrap()
        .unwrap()
        .to_ascii_lowercase()
        .contains("kwikpaste"));
    block_on(core.set_extension_enabled("ocr", false)).unwrap();
    assert!(block_on(core.image_text_preview(&item.id))
        .unwrap()
        .is_none());
    assert!(block_on(core.copy_image_text(&item.id)).is_err());
    assert!(block_on(core.image_text(&item.id)).unwrap().is_some());
    // 安装前必须回收自己启动的子进程，否则 Windows 会锁定其可执行文件。
    let host = process::ProcessHost::<
        kwikpaste_ext_protocol::Request,
        kwikpaste_ext_protocol::Response,
    >::start(
        "ocr",
        &core.resolve_extension("ocr").unwrap_or_else(|| {
            core.paths()
                .extensions_dir()
                .join("ocr/1.0.0")
                .join(executable_name("ocr").unwrap())
        }),
    )
    .unwrap();
    let child = host.child.clone();
    core.0.ocr.track_child_for_test(child.clone());
    fs::copy(ocr_binary(), &staged).unwrap();
    block_on(core.install_extension("ocr", "1.1.0", OCR_PROTOCOL, &staged)).unwrap();
    assert!(child.lock().unwrap().try_wait().unwrap().is_some());
    drop(host);
    assert!(!core.ocr_enabled());
    assert!(!core.installed_extensions()["ocr"].enabled);
    block_on(core.set_extension_enabled("ocr", true)).unwrap();
    assert!(core.ocr_enabled());
    assert!(block_on(core.image_text_preview(&item.id))
        .unwrap()
        .is_some());
    block_on(core.uninstall_extension("ocr")).unwrap();
    assert!(!core.ocr_enabled());
    assert!(!core.paths().extensions_dir().join("ocr").exists());
    assert!(block_on(core.image_text(&item.id)).unwrap().is_some());
    block_on(core.shutdown()).unwrap();
}

#[cfg(target_os = "windows")]
#[test]
fn ocr_engine_resolution_keeps_feature_gate_and_falls_back() {
    let temp = tempfile::tempdir().unwrap();
    let store = ExtensionStore::load(temp.path().join("extensions"));
    let source = staged(&temp, "tiny", b"tiny");
    store
        .install("ocr-tiny", "1.0.0", OCR_PROTOCOL, &source)
        .unwrap();
    assert!(store.resolve_ocr().is_none());
    let source = staged(&temp, "system", b"system");
    store
        .install("ocr", "1.0.0", OCR_PROTOCOL, &source)
        .unwrap();
    let system = store.resolve("ocr").unwrap();
    let tiny = store.resolve("ocr-tiny").unwrap();
    assert_eq!(store.resolve_ocr(), Some(tiny.clone()));
    store.set_enabled("ocr", false).unwrap();
    assert!(store.resolve_ocr().is_none());
    store.set_enabled("ocr", true).unwrap();
    store.set_enabled("ocr-tiny", false).unwrap();
    assert_eq!(store.resolve_ocr(), Some(system.clone()));
    store.set_enabled("ocr-tiny", true).unwrap();
    let mut entries = store.list();
    entries.get_mut("ocr-tiny").unwrap().protocol += 1;
    store.save(entries).unwrap();
    assert_eq!(store.resolve_ocr(), Some(system.clone()));
    let mut entries = store.list();
    entries.get_mut("ocr-tiny").unwrap().protocol = OCR_PROTOCOL;
    store.save(entries).unwrap();
    fs::remove_file(tiny).unwrap();
    assert_eq!(store.resolve_ocr(), Some(system));
    store.uninstall("ocr").unwrap();
    assert!(store.resolve_ocr().is_none());
}

#[cfg(target_os = "windows")]
#[test]
fn installing_a_tier_replaces_even_disabled_tiers_but_not_system_ocr() {
    let temp = tempfile::tempdir().unwrap();
    let store = ExtensionStore::load(temp.path().join("extensions"));
    let source = staged(&temp, "system", b"system");
    store
        .install("ocr", "1.0.0", OCR_PROTOCOL, &source)
        .unwrap();
    for id in OCR_TIERS {
        let source = staged(&temp, "tier", id.as_bytes());
        store.install(id, "1.0.0", OCR_PROTOCOL, &source).unwrap();
        let installed = ExtensionStore::load(store.dir.clone()).list();
        assert_eq!(installed.len(), 2);
        assert!(installed.contains_key("ocr"));
        assert!(installed.contains_key(id));
        for other in OCR_TIERS.into_iter().filter(|other| *other != id) {
            assert!(!store.dir.join(other).exists());
        }
        assert_eq!(store.resolve_ocr(), store.resolve(id));
        store.set_enabled(id, false).unwrap();
        assert_eq!(store.resolve_ocr(), store.resolve("ocr"));
        let missing = temp.path().join("missing");
        assert!(store
            .install("ocr-tiny", "2.0.0", OCR_PROTOCOL, &missing)
            .is_err());
        assert_eq!(
            store.list(),
            installed
                .into_iter()
                .map(|(key, mut value)| {
                    if key == id {
                        value.enabled = false;
                    }
                    (key, value)
                })
                .collect()
        );
    }
    store.uninstall("ocr-medium").unwrap();
    assert_eq!(store.resolve_ocr(), store.resolve("ocr"));
}
