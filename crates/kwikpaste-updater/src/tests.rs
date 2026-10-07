//! 更新器整条链路：本机 HTTP 服务上的清单与安装包、一次性密钥验签、假宿主与假启动器交接。
//! 不请求任何线上地址，不运行安装包，不替换本机的任何程序。

use std::sync::Mutex;

use serde_json::json;
use url::Url;

use super::*;
use crate::handoff::testing::{FakeLauncher, Journal, RecordingHost, recorders};
use crate::http::testing::{Reply, Server};
use crate::testing::TestCore;
use crate::verify::testing::TestKey;

#[derive(Default)]
struct NullUi {
    available: Mutex<Vec<UpdateStatus>>,
}

impl UpdaterUi for NullUi {
    fn update_available(&self, status: UpdateStatus) {
        self.available.lock().unwrap().push(status);
    }

    fn show_announcement(
        &self,
        _prompt: AnnouncementPrompt,
    ) -> HostFuture<'_, AnnouncementOutcome> {
        Box::pin(async { AnnouncementOutcome::Closed })
    }

    fn open_url(&self, _url: &str) {}
}

fn platform_keys() -> serde_json::Value {
    let mut platforms = serde_json::Map::new();
    for key in InstallKind::Nsis
        .platform_keys()
        .into_iter()
        .chain(InstallKind::Portable.platform_keys())
    {
        platforms.insert(key, json!({}));
    }
    serde_json::Value::Object(platforms)
}

/// 一份清单：所有 Windows 平台键都指向同一个安装包地址。
fn manifest(version: &str, url: &str, signature: &str) -> Vec<u8> {
    let mut platforms = platform_keys();
    for value in platforms.as_object_mut().unwrap().values_mut() {
        *value = json!({ "url": url, "signature": signature });
    }
    serde_json::to_vec(&json!({
        "version": version,
        "notes": "notes",
        "pub_date": "2027-04-05T08:00:00Z",
        "platforms": platforms,
    }))
    .unwrap()
}

struct Setup {
    test: TestCore,
    updater: Updater,
    journal: Journal,
}

fn setup(kind: InstallKind, endpoint: Url, key: &TestKey, fails: bool) -> Setup {
    let test = TestCore::start("2.0.0");
    let (host, launcher, journal): (RecordingHost, FakeLauncher, Journal) = recorders(fails);
    let updater = Updater::with_parts(
        test.core.clone(),
        Arc::new(NullUi::default()),
        Arc::new(host),
        Parts {
            kind,
            launcher: Box::new(launcher),
            endpoints: Box::new(move || Ok(vec![endpoint.clone()])),
            public_key: key.public_key(),
        },
    )
    .unwrap();
    Setup {
        test,
        updater,
        journal,
    }
}

/// 检查 → 下载（带进度、验签）→ 交接给 NSIS 安装包：假启动器只记下要执行的命令。
#[test]
fn check_download_and_install_through_the_nsis_handoff() {
    let key = TestKey::generate();
    let installer = [b"MZ".as_slice(), &[1u8; 50_000]].concat();
    let files = Server::start(vec![Reply::ok(installer.clone())]);
    let installer_url = files.url("/KwikPaste_2.0.1_x64-setup.exe");
    let manifests = Server::start(vec![Reply::ok(manifest(
        "2.0.1",
        &installer_url,
        &key.sign(&installer, true),
    ))]);
    let stable = Url::parse(&manifests.url("/v2/stable/latest.json")).unwrap();
    let Setup {
        test,
        updater,
        journal,
    } = setup(InstallKind::Nsis, stable, &key, false);

    let status = test.block_on(updater.check(CheckMode::Manual)).unwrap();
    let update = status.update.unwrap();
    assert!(status.supported);
    assert_eq!(update.version, "2.0.1");
    assert_eq!(update.download_url, installer_url);
    assert!(!update.downloaded);
    assert_eq!(
        update.release_notes_url,
        "https://github.com/ManSanDADADA/KwikPaste/releases/tag/v2.0.1"
    );
    assert!(test.core.settings().update.last_checked_at.is_some());

    let progress = Arc::new(Mutex::new(Vec::new()));
    let seen = progress.clone();
    let downloaded = test
        .block_on(updater.download("2.0.1".to_owned(), move |step| {
            seen.lock().unwrap().push(step)
        }))
        .unwrap();
    assert!(downloaded.downloaded);
    assert!(updater.status().update.unwrap().downloaded);
    assert_eq!(
        progress.lock().unwrap().last().unwrap().downloaded,
        installer.len() as u64
    );

    test.block_on(updater.install("2.0.1".to_owned())).unwrap();
    let journal = journal.lock().unwrap().clone();
    assert_eq!(journal[0], "host: stop_input");
    assert!(
        journal[1].starts_with("shell_execute KwikPaste-2.0.1-installer.exe /P /R /UPDATE /ARGS")
    );
    assert_eq!(
        &journal[2..],
        [
            "host: remove_tray",
            "host: release_single_instance",
            "host: exit(0)"
        ]
    );
    let record = take_handoff(test.core.paths()).unwrap();
    assert_eq!(
        (
            record.from.as_str(),
            record.to.as_str(),
            record.kind.as_str()
        ),
        ("2.0.0", "2.0.1", "nsis")
    );

    let package = updater
        .0
        .pending
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .package
        .clone()
        .unwrap();
    std::fs::remove_dir_all(package.dir).ok();
}

#[test]
fn a_tampered_package_is_never_installed() {
    let key = TestKey::generate();
    let installer = b"MZ genuine".to_vec();
    let files = Server::start(vec![Reply::ok(b"MZ tampered".to_vec())]);
    let manifests = Server::start(vec![Reply::ok(manifest(
        "2.0.1",
        &files.url("/setup.exe"),
        &key.sign(&installer, true),
    ))]);
    let stable = Url::parse(&manifests.url("/latest.json")).unwrap();
    let Setup {
        test,
        updater,
        journal,
    } = setup(InstallKind::Nsis, stable, &key, false);

    test.block_on(updater.check(CheckMode::Manual)).unwrap();
    assert!(
        test.block_on(updater.download("2.0.1".to_owned(), |_| {}))
            .is_err()
    );
    assert!(test.block_on(updater.install("2.0.1".to_owned())).is_err());
    assert!(journal.lock().unwrap().is_empty());
    assert!(!updater.status().update.unwrap().downloaded);
}

/// 测试版与正式版走同一个渠道：发布了测试版就提供，跳过之后不再提供，直到渠道换成更新的版本。
#[test]
fn prereleases_are_offered_and_skipped_versions_stay_hidden() {
    let key = TestKey::generate();
    let server = Server::start(vec![
        Reply::ok(manifest(
            "2.1.0-beta.1",
            "https://dl.example.com/b.exe",
            "c2ln",
        )),
        Reply::ok(manifest(
            "2.1.0-beta.1",
            "https://dl.example.com/b.exe",
            "c2ln",
        )),
        Reply::ok(manifest("2.1.0", "https://dl.example.com/a.exe", "c2ln")),
    ]);
    let Setup { test, updater, .. } = setup(
        InstallKind::Nsis,
        Url::parse(&server.url("/latest.json")).unwrap(),
        &key,
        false,
    );

    let status = test.block_on(updater.check(CheckMode::Manual)).unwrap();
    assert_eq!(status.update.unwrap().version, "2.1.0-beta.1");

    let status = test
        .block_on(updater.skip("2.1.0-beta.1".to_owned()))
        .unwrap();
    assert!(status.update.is_none());
    assert_eq!(
        test.core.settings().update.skipped_version.as_deref(),
        Some("2.1.0-beta.1")
    );
    let status = test.block_on(updater.check(CheckMode::Manual)).unwrap();
    assert!(status.update.is_none());

    let status = test.block_on(updater.check(CheckMode::Manual)).unwrap();
    assert_eq!(status.update.unwrap().version, "2.1.0");
}

/// 自动检查没到频率设定的时间就不发请求。
#[test]
fn automatic_checks_wait_for_the_configured_frequency() {
    let key = TestKey::generate();
    let server = Server::start(vec![Reply::ok(manifest(
        "2.0.1",
        "https://dl.example.com/a.exe",
        "c2ln",
    ))]);
    let Setup { test, updater, .. } = setup(
        InstallKind::Nsis,
        Url::parse(&server.url("/latest.json")).unwrap(),
        &key,
        false,
    );
    let recently =
        json!({"update": {"lastCheckedAt": Utc::now().to_rfc3339(), "frequency": "daily"}});
    test.block_on(test.core.update_settings(recently)).unwrap();

    let status = test.block_on(updater.check(CheckMode::Auto)).unwrap();

    assert!(status.update.is_none());
    assert!(server.requests().is_empty());
}

/// 开发构建、手动拷出来的 exe：不检查也不安装，只说明不支持。
#[test]
fn unmanaged_builds_never_check_or_install() {
    let key = TestKey::generate();
    let server = Server::start(vec![Reply::ok(manifest(
        "2.0.1",
        "https://dl.example.com/a.exe",
        "c2ln",
    ))]);
    let Setup {
        test,
        updater,
        journal,
    } = setup(
        InstallKind::Unmanaged,
        Url::parse(&server.url("/latest.json")).unwrap(),
        &key,
        false,
    );

    let status = test.block_on(updater.check(CheckMode::Manual)).unwrap();

    assert!(!status.supported);
    assert!(status.update.is_none());
    assert!(server.requests().is_empty());
    assert!(test.block_on(updater.install("2.0.1".to_owned())).is_err());
    assert!(journal.lock().unwrap().is_empty());
}

/// 测试进程本身不在安装目录里：按真实位置判定为开发构建，更新器不工作。
#[test]
fn the_test_binary_is_detected_as_unmanaged() {
    let test = TestCore::start("2.0.0");
    let (host, _, _) = recorders(false);
    let updater = Updater::new(
        test.core.clone(),
        Arc::new(NullUi::default()),
        Arc::new(host),
    )
    .unwrap();

    // 不调用 start()：调度器会清理系统临时目录里一天以前的更新残留，测试不碰本机的这些文件。
    assert_eq!(updater.install_kind(), &InstallKind::Unmanaged);
    assert!(!updater.status().supported);
}
