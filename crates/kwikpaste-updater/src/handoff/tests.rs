//! 交接顺序的测试：假宿主与假启动器只记流水账，绝不真的运行安装包、替换本机程序或退出进程。
//! 「当前 exe」「当前 .app」都是临时目录里的假文件。

use std::io::{Cursor, Write};

use super::testing::recorders;
use super::*;
use crate::testing::TestCore;

fn handoff<'a>(
    test: &'a TestCore,
    host: &'a dyn HandoffHost,
    launcher: &'a dyn Launcher,
    exe: PathBuf,
) -> Handoff<'a> {
    Handoff {
        core: &test.core,
        host,
        launcher,
        exe,
        args: vec![OsString::from("--auto-launch")],
        from: "2.0.0".to_owned(),
        to: "2.0.1".to_owned(),
    }
}

/// core 关停后连接池已关闭，查询会失败。
fn core_is_closed(test: &TestCore) -> bool {
    test.block_on(test.core.storage_usage()).is_err()
}

fn record(test: &TestCore) -> Option<HandoffRecord> {
    take_handoff(test.core.paths())
}

#[test]
fn nsis_handoff_runs_the_installer_then_leaves() {
    let test = TestCore::start("2.0.0");
    let (host, launcher, journal) = recorders(false);
    let installer = test.root().join("KwikPaste-2.0.1-installer.exe");
    fs::write(&installer, b"MZ installer").unwrap();
    let exe = test.root().join("KwikPaste.exe");

    test.block_on(handoff(&test, &host, &launcher, exe).nsis(&installer))
        .unwrap();

    assert_eq!(
        *journal.lock().unwrap(),
        [
            "host: stop_input",
            "shell_execute KwikPaste-2.0.1-installer.exe /P /R /UPDATE /ARGS --auto-launch",
            "host: remove_tray",
            "host: release_single_instance",
            "host: exit(0)",
        ]
    );
    assert!(core_is_closed(&test));
    let record = record(&test).unwrap();
    assert_eq!(
        (record.from.as_str(), record.to.as_str()),
        ("2.0.0", "2.0.1")
    );
    assert_eq!(record.kind, "nsis");
    assert!(record.succeeded(&semver::Version::new(2, 0, 1)));
    assert!(!record.succeeded(&semver::Version::new(2, 0, 0)));
    // 读完即删。
    assert!(take_handoff(test.core.paths()).is_none());
}

/// 用户拒绝 UAC：不能让应用就此消失，删掉交接文件后用原参数重新启动自己。
#[test]
fn nsis_handoff_restarts_the_current_version_when_the_installer_does_not_start() {
    let test = TestCore::start("2.0.0");
    let (host, launcher, journal) = recorders(true);
    let installer = test.root().join("KwikPaste-2.0.1-installer.exe");
    fs::write(&installer, b"MZ installer").unwrap();
    let exe = test.root().join("KwikPaste.exe");

    let err = test
        .block_on(handoff(&test, &host, &launcher, exe).nsis(&installer))
        .unwrap_err();

    assert!(format!("{err:#}").contains("did not start"), "{err:#}");
    assert_eq!(
        *journal.lock().unwrap(),
        [
            "host: stop_input",
            "shell_execute KwikPaste-2.0.1-installer.exe /P /R /UPDATE /ARGS --auto-launch",
            "host: remove_tray",
            "host: release_single_instance",
            "spawn KwikPaste.exe --auto-launch",
            "host: exit(0)",
        ]
    );
    assert!(record(&test).is_none());
}

#[test]
fn nsis_handoff_refuses_a_file_that_is_not_an_installer() {
    let test = TestCore::start("2.0.0");
    let (host, launcher, journal) = recorders(false);
    let installer = test.root().join("KwikPaste-2.0.1-installer.exe");
    fs::write(&installer, b"#!/bin/sh").unwrap();

    assert!(
        test.block_on(
            handoff(&test, &host, &launcher, test.root().join("KwikPaste.exe")).nsis(&installer)
        )
        .is_err()
    );
    assert!(journal.lock().unwrap().is_empty());
    assert!(!core_is_closed(&test));
    assert!(record(&test).is_none());
}

fn portable_package(exe: &[u8]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, content) in [
        ("KwikPaste/portable.txt", b"marker".as_slice()),
        ("KwikPaste/KwikPaste.exe", exe),
    ] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(content).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// 便携版：先换 exe，最后释放单实例之后才启动新 exe。
#[test]
fn portable_handoff_swaps_the_exe_and_releases_the_single_instance_first() {
    let test = TestCore::start("2.0.0");
    let (host, launcher, journal) = recorders(false);
    let exe = test.root().join("portable").join("KwikPaste.exe");
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    fs::write(&exe, b"MZ old").unwrap();
    let package = test.root().join("KwikPaste-2.0.1-portable.zip");
    fs::write(&package, portable_package(b"MZ new")).unwrap();

    test.block_on(handoff(&test, &host, &launcher, exe.clone()).portable(&package))
        .unwrap();

    assert_eq!(fs::read(&exe).unwrap(), b"MZ new");
    assert_eq!(
        *journal.lock().unwrap(),
        [
            "host: stop_input",
            "host: remove_tray",
            "host: release_single_instance",
            "spawn KwikPaste.exe --auto-launch",
            "host: exit(0)",
        ]
    );
    assert!(core_is_closed(&test));
    assert_eq!(record(&test).unwrap().kind, "portable");
}

#[test]
fn portable_handoff_keeps_running_when_the_package_is_bad() {
    let test = TestCore::start("2.0.0");
    let (host, launcher, journal) = recorders(false);
    let exe = test.root().join("KwikPaste.exe");
    fs::write(&exe, b"MZ old").unwrap();
    let package = test.root().join("bad.zip");
    fs::write(&package, portable_package(b"not a program")).unwrap();

    assert!(
        test.block_on(handoff(&test, &host, &launcher, exe.clone()).portable(&package))
            .is_err()
    );
    assert_eq!(fs::read(&exe).unwrap(), b"MZ old");
    assert!(journal.lock().unwrap().is_empty());
    assert!(!core_is_closed(&test));
}

fn app_archive() -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, content) in [
        ("KwikPaste.app/Contents/Info.plist", b"<plist/>".as_slice()),
        ("KwikPaste.app/Contents/MacOS/KwikPaste", b"new binary"),
    ] {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder.append_data(&mut header, path, content).unwrap();
    }
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    gz.write_all(&builder.into_inner().unwrap()).unwrap();
    gz.finish().unwrap()
}

/// macOS：换包在先，退出前排好 `sh … open` 重启。这里只在临时目录里摆一个假的 `.app`。
#[test]
fn mac_handoff_swaps_the_bundle_and_schedules_a_relaunch() {
    let test = TestCore::start("2.0.0");
    let (host, launcher, journal) = recorders(false);
    let bundle = test.root().join("Applications").join("KwikPaste.app");
    fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
    fs::write(bundle.join("Contents/MacOS/KwikPaste"), b"old binary").unwrap();
    let work = test.root().join("KwikPaste-2.0.1-updater-test");
    fs::create_dir_all(&work).unwrap();
    let archive = work.join("KwikPaste-2.0.1.app.tar.gz");
    fs::write(&archive, app_archive()).unwrap();
    let exe = bundle.join("Contents/MacOS/KwikPaste");

    test.block_on(handoff(&test, &host, &launcher, exe).mac_app(&bundle, &archive, &work))
        .unwrap();

    assert_eq!(
        fs::read(bundle.join("Contents/MacOS/KwikPaste")).unwrap(),
        b"new binary"
    );
    let journal = journal.lock().unwrap().clone();
    assert_eq!(journal[0], "run /usr/bin/touch");
    assert_eq!(
        &journal[1..4],
        [
            "host: stop_input",
            "host: remove_tray",
            "host: release_single_instance",
        ]
    );
    assert!(journal[4].starts_with("spawn sh -c "), "{}", journal[4]);
    assert!(journal[4].ends_with(&format!("{} --auto-launch", bundle.display())));
    assert_eq!(journal[5], "host: exit(0)");
    assert_eq!(record(&test).unwrap().kind, "app");
}

#[test]
fn unreadable_handoff_files_are_ignored() {
    let test = TestCore::start("2.0.0");
    let path = test.core.paths().bootstrap_dir().join(HANDOFF_FILENAME);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, b"{broken").unwrap();

    assert!(take_handoff(test.core.paths()).is_none());
    assert!(!path.exists());
}
