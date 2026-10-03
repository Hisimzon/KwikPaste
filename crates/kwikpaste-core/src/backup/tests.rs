//! 备份的格式测试（从 1.4.0 原样移植）与端到端导出导入测试（真实的 core 与临时目录）。

use std::io::Cursor;

use serde_json::json;
use tempfile::tempdir;
use zip::ZipArchive;

use super::*;
use crate::clipboard::{MemoryClipboard, MemoryState};
use crate::db::models::{ClipboardItemQuery, ClipboardKind};
use crate::ops::ClipboardGroupInput;
use crate::settings::Theme;
use crate::testing::{block_on, sample_png, Fixture};

const PASSWORD: &str = "correct horse battery";

#[test]
fn backup_extension_validation_accepts_expected_suffix() {
    let path = normalize_backup_path(PathBuf::from("demo.kwikpastebak")).unwrap();
    assert_eq!(path, PathBuf::from("demo.kwikpastebak"));
}

#[test]
fn backup_extension_validation_appends_missing_suffix() {
    let path = normalize_backup_path(PathBuf::from("demo")).unwrap();
    assert_eq!(path, PathBuf::from("demo.kwikpastebak"));
}

#[test]
fn backup_extension_validation_rejects_other_suffix() {
    assert!(normalize_backup_path(PathBuf::from("demo.zip")).is_err());
}

#[test]
fn encrypted_payload_header_round_trips() {
    let encrypted = encrypt_payload(b"hello", "password-123").unwrap();
    let mut bytes = Vec::new();
    write_header(&mut bytes, &encrypted.header).unwrap();
    bytes.extend_from_slice(&encrypted.ciphertext);

    let mut cursor = Cursor::new(bytes);

    assert_eq!(
        inspect_backup_reader(&mut cursor).unwrap(),
        BackupContainerMode::Encrypted
    );
}

#[test]
fn oversized_kdf_parameters_are_rejected_before_deriving() {
    let encrypted = encrypt_payload(b"hello", PASSWORD).unwrap();
    let kdf = encrypted.header.kdf.as_ref().unwrap();
    let cipher = encrypted.header.cipher.as_ref().unwrap();
    assert_eq!(
        decrypt_payload(&encrypted.ciphertext, PASSWORD, kdf, cipher).unwrap(),
        b"hello"
    );

    for (memory_kib, time_cost, parallelism) in [
        (u32::MAX, ARGON2_TIME_COST, ARGON2_PARALLELISM),
        (ARGON2_MEMORY_KIB, u32::MAX, ARGON2_PARALLELISM),
        (ARGON2_MEMORY_KIB, ARGON2_TIME_COST, 0x00ff_ffff),
    ] {
        let crafted = KdfHeader {
            algorithm: kdf.algorithm.clone(),
            memory_kib,
            time_cost,
            parallelism,
            salt: kdf.salt.clone(),
        };
        assert!(decrypt_payload(&encrypted.ciphertext, PASSWORD, &crafted, cipher).is_err());
    }
}

/// 用 `build` 写出一个 zip，返回字节。
fn zip_bytes(build: impl FnOnce(&mut zip::ZipWriter<Cursor<Vec<u8>>>)) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    build(&mut writer);
    writer.finish().unwrap().into_inner()
}

#[test]
fn payload_extraction_keeps_files_and_rejects_links_and_escapes() {
    use std::io::Write as _;

    let options = zip::write::SimpleFileOptions::default();
    let payload = zip_bytes(|writer| {
        writer.add_directory("resources/", options).unwrap();
        writer.start_file("db/clipboard.db", options).unwrap();
        writer.write_all(b"db").unwrap();
    });
    let dir = extract_payload_zip(&payload).unwrap();
    assert_eq!(
        std::fs::read(dir.path().join("db").join("clipboard.db")).unwrap(),
        b"db"
    );
    assert!(dir.path().join("resources").is_dir());

    let link = zip_bytes(|writer| {
        writer
            .add_symlink("config/settings.json", "/etc/passwd", options)
            .unwrap();
    });
    assert!(extract_payload_zip(&link).is_err());

    let escape = zip_bytes(|writer| {
        writer.start_file("../outside.txt", options).unwrap();
        writer.write_all(b"x").unwrap();
    });
    assert!(extract_payload_zip(&escape).is_err());
}

#[test]
fn inspect_backup_reader_recognizes_plain_zip() {
    let mut cursor = Cursor::new(b"PK\x03\x04demo".to_vec());

    assert_eq!(
        inspect_backup_reader(&mut cursor).unwrap(),
        BackupContainerMode::Plain
    );
}

#[test]
fn inspect_backup_reader_recognizes_encrypted_container() {
    let encrypted = encrypt_payload(b"hello", "password-123").unwrap();
    let mut bytes = Vec::new();
    write_header(&mut bytes, &encrypted.header).unwrap();
    bytes.extend_from_slice(&encrypted.ciphertext);
    let mut cursor = Cursor::new(bytes);

    assert_eq!(
        inspect_backup_reader(&mut cursor).unwrap(),
        BackupContainerMode::Encrypted
    );
}

#[test]
fn payload_zip_contains_only_backup_whitelist() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let resources = root.join("resources");
    fs::create_dir_all(resources.join("clipboard-images/origin")).unwrap();
    fs::write(root.join("clipboard.db"), b"db").unwrap();
    fs::write(root.join("settings.json"), b"{}").unwrap();
    fs::write(root.join("window-state.json"), b"skip").unwrap();
    fs::write(root.join("settings.json.bak"), b"skip").unwrap();
    fs::write(resources.join("clipboard-images/origin/demo.png"), b"image").unwrap();
    // 单图文件记录的缩略图是缓存，不进备份。
    fs::create_dir_all(resources.join("clipboard-images/file-thumbnails/ab")).unwrap();
    fs::write(
        resources.join("clipboard-images/file-thumbnails/ab/ab12.png"),
        b"cache",
    )
    .unwrap();

    let payload = root.join("payload.zip");
    let target = root.join("backup.kwikpastebak");
    let manifest = BackupManifest {
        format_version: FORMAT_VERSION,
        app_name: "KwikPaste".to_owned(),
        app_version: "0.0.0".to_owned(),
        exported_at: Utc::now(),
        platform: "macos".to_owned(),
        encryption: ManifestEncryption::None,
        item_count: 1,
        text_count: 1,
        image_count: 0,
        files_count: 0,
        resource_bytes: 5,
    };
    let source_paths = BackupSourcePaths {
        db_path: root.join("clipboard.db"),
        resources_dir: resources,
        settings_path: root.join("settings.json"),
    };

    write_payload_zip(&source_paths, &payload, &manifest, &target).unwrap();

    let file = File::open(payload).unwrap();
    let mut archive = ZipArchive::new(file).unwrap();
    let mut names = Vec::new();
    for index in 0..archive.len() {
        let file = archive.by_index(index).unwrap();
        names.push(file.name().to_owned());
    }
    names.sort();

    assert_eq!(
        names,
        vec![
            "config/settings.json",
            "db/clipboard.db",
            "manifest.json",
            "resources/clipboard-images/origin/demo.png",
        ]
    );
}

#[tokio::test]
async fn merge_preserves_is_sensitive_flag() {
    use crate::db::items::{content_hash, insert_item};
    use crate::db::models::{ClipboardItem, Platform};
    use crate::db::test_support::memory_pool;

    // The backup pool stands in for a real backup database, which is a
    // snapshot of the live clipboard.db and therefore already carries the
    // `is_sensitive` flag written by the production insert path.
    let backup = memory_pool().await;
    let current = memory_pool().await;

    let ts = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
    let content = "AKIAIOSFODNN7EXAMPLE".to_owned();
    let item = ClipboardItem {
        id: "secret-1".to_owned(),
        kind: ClipboardKind::Text,
        sub_kind: None,
        group_id: None,
        source_app_id: None,
        content_hash: content_hash(ClipboardKind::Text, &content),
        content,
        search_text: None,
        summary: None,
        file_types: None,
        size: None,
        width: None,
        height: None,
        use_count: 1,
        is_favorite: false,
        is_pinned: false,
        is_sensitive: true,
        platform: Platform::Macos,
        note: None,
        created_at: ts,
        updated_at: ts,
        origin_device_id: None,
        source_app_name: None,
        source_app_icon_file: None,
    };
    insert_item(&backup, &item).await.unwrap();

    let outcome = merge_history(&current, &backup).await.unwrap();
    assert_eq!(
        outcome.imported_items, 1,
        "sensitive item should be imported"
    );

    // Regression: the merge SELECT/INSERT used to omit `is_sensitive`, so
    // every imported row silently fell back to the column DEFAULT (0) and
    // secrets were rendered in plaintext. The flag must survive the merge.
    let imported: bool =
        sqlx::query_scalar("SELECT is_sensitive FROM clipboard_items WHERE id = ?")
            .bind("secret-1")
            .fetch_one(&current)
            .await
            .unwrap();
    assert!(
        imported,
        "merged sensitive item must keep is_sensitive = true"
    );
}

// ---- 端到端：两个 core 实例之间导出、导入 ----

fn store(core: &Core, state: MemoryState) -> String {
    let clipboard = MemoryClipboard::with_state(state);
    let item = core
        .build_item(&core.read_payload(&clipboard).unwrap().unwrap())
        .unwrap()
        .unwrap();
    block_on(core.store_item(item, None)).unwrap().id
}

fn text(value: &str) -> MemoryState {
    MemoryState {
        text: Some(value.to_owned()),
        ..MemoryState::default()
    }
}

fn image(width: u32) -> MemoryState {
    MemoryState {
        png: Some(sample_png(width, 6)),
        ..MemoryState::default()
    }
}

fn contents(core: &Core) -> Vec<String> {
    let mut contents: Vec<_> = block_on(core.list_items(ClipboardItemQuery {
        limit: 100,
        ..ClipboardItemQuery::default()
    }))
    .unwrap()
    .list
    .into_iter()
    .filter(|view| view.item.kind == ClipboardKind::Text)
    .map(|view| {
        block_on(core.find_item(&view.item.id))
            .unwrap()
            .unwrap()
            .content
    })
    .collect();
    contents.sort();
    contents
}

/// 带同步序号的记录条数与计数器：只有本机复制的记录有序号。
fn sync_state(core: &Core) -> (i64, i64) {
    block_on(core.hop({
        let core = core.clone();
        async move {
            let pool = core.0.db.pool().await;
            let local: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM clipboard_items WHERE sync_seq IS NOT NULL",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            Ok((local, crate::db::sync::sync_counter(&pool).await?))
        }
    }))
    .unwrap()
}

fn input(path: &Path, password: Option<&str>) -> ImportHistoryBackupInput {
    ImportHistoryBackupInput {
        path: path.to_path_buf(),
        password: password.map(str::to_owned),
    }
}

/// 源实例：一条收藏的文本、一张图片、一个分组，深色主题。
struct Source {
    fixture: Fixture,
    core: Core,
    image_file: String,
}

fn source() -> Source {
    let fixture = Fixture::new();
    let core = fixture.start();
    block_on(core.update_settings(json!({"appearance": {"theme": "dark"}}))).unwrap();
    let favorite = store(&core, text("shared text"));
    block_on(core.toggle_favorite(&favorite)).unwrap();
    store(&core, text("only in the backup"));
    let image_id = store(&core, image(10));
    let group = block_on(core.create_group(ClipboardGroupInput {
        name: "Work".to_owned(),
        icon: "folder".to_owned(),
        is_hidden: false,
    }))
    .unwrap();
    block_on(core.set_item_group(&favorite, &group.id)).unwrap();
    let image_file = block_on(core.find_item(&image_id))
        .unwrap()
        .unwrap()
        .content;

    Source {
        fixture,
        core,
        image_file,
    }
}

fn export(source: &Source, mode: BackupExportMode, password: Option<&str>) -> PathBuf {
    let target = source.fixture.root().join("exports").join("history");
    let result = block_on(source.core.export_history_backup(
        target,
        ExportHistoryBackupOptions {
            mode,
            password: password.map(str::to_owned),
        },
    ))
    .unwrap();

    assert_eq!(result.mode, mode);
    assert_eq!(result.item_count, 3);
    assert_eq!(result.text_count, 2);
    assert_eq!(result.image_count, 1);
    assert!(result.resource_bytes > 0);
    let path = PathBuf::from(result.path);
    assert_eq!(path.extension().unwrap(), BACKUP_EXTENSION);
    assert_eq!(fs::metadata(&path).unwrap().len(), result.total_bytes);
    path
}

#[test]
fn plain_export_merges_into_another_instance() {
    let source = source();
    let backup = export(&source, BackupExportMode::Plain, None);
    assert_eq!(
        inspect_backup_file(&backup).unwrap(),
        BackupContainerMode::Plain
    );

    let fixture = Fixture::new();
    let core = fixture.start();
    store(&core, text("shared text"));
    store(&core, text("only here"));
    fixture.take_events();

    let result =
        block_on(core.import_history_backup(input(&backup, None), BackupImportStrategy::Merge))
            .unwrap();

    assert_eq!(result.strategy, BackupImportStrategy::Merge);
    assert_eq!(result.imported_items, 2);
    assert_eq!(result.skipped_items, 1);
    assert!(result.imported_resources >= 1);
    assert_eq!(
        contents(&core),
        ["only here", "only in the backup", "shared text"]
    );
    assert!(core
        .image_origin_path(&source.image_file)
        .unwrap()
        .is_file());
    assert_eq!(block_on(core.list_groups()).unwrap().len(), 1);
    assert_eq!(core.settings().appearance.theme, Theme::Dark);
    // 合并进来的记录不是这台电脑复制的，没有同步序号，不会补齐给已配对设备。
    assert_eq!(sync_state(&core), (2, 2));

    let events = fixture.take_events();
    assert!(events
        .iter()
        .any(|event| matches!(event, CoreEvent::ClipboardReloaded)));
    assert!(events.iter().any(|event| matches!(
        event,
        CoreEvent::SettingsUpdated { delta, .. } if delta.touches("appearance.theme")
    )));
    block_on(core.shutdown()).unwrap();
    block_on(source.core.shutdown()).unwrap();
}

#[test]
fn encrypted_export_overwrites_another_instance() {
    let source = source();
    let backup = export(&source, BackupExportMode::Encrypted, Some(PASSWORD));
    assert_eq!(
        inspect_backup_file(&backup).unwrap(),
        BackupContainerMode::Encrypted
    );
    let bytes = fs::read(&backup).unwrap();
    assert!(bytes.starts_with(MAGIC));

    let fixture = Fixture::new();
    let core = fixture.start();
    store(&core, text("replaced by the backup"));
    let local_image = store(&core, image(20));
    let local_image_file = block_on(core.find_item(&local_image))
        .unwrap()
        .unwrap()
        .content;

    let missing =
        block_on(core.import_history_backup(input(&backup, None), BackupImportStrategy::Overwrite))
            .unwrap_err();
    assert_eq!(missing.to_string(), "请输入备份密码");
    let wrong = block_on(core.import_history_backup(
        input(&backup, Some("wrong password")),
        BackupImportStrategy::Overwrite,
    ))
    .unwrap_err();
    assert_eq!(wrong.to_string(), "备份密码不正确或文件已损坏");
    assert_eq!(contents(&core), ["replaced by the backup"]);

    fixture.take_events();
    let result = block_on(core.import_history_backup(
        input(&backup, Some(PASSWORD)),
        BackupImportStrategy::Overwrite,
    ))
    .unwrap();

    assert_eq!(result.strategy, BackupImportStrategy::Overwrite);
    assert_eq!(contents(&core), ["only in the backup", "shared text"]);
    assert!(core
        .image_origin_path(&source.image_file)
        .unwrap()
        .is_file());
    assert!(!core.image_origin_path(&local_image_file).unwrap().exists());
    assert_eq!(core.settings().appearance.theme, Theme::Dark);
    assert!(!core.0.watcher_pause.is_paused());
    let db_dir = fixture.paths.db_dir().unwrap();
    let mut db_files: Vec<_> = fs::read_dir(&db_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".importing") || name.contains(".previous"))
        .collect();
    db_files.sort();
    assert!(db_files.is_empty(), "leftover files: {db_files:?}");

    let events = fixture.take_events();
    assert!(events
        .iter()
        .any(|event| matches!(event, CoreEvent::ClipboardReloaded)));
    assert!(events.iter().any(|event| matches!(
        event,
        CoreEvent::SettingsUpdated { delta, .. } if delta.touches("shortcuts")
    )));

    // 覆盖进来的记录都没有同步序号；计数器取两边较大的，不回退。
    assert_eq!(sync_state(&source.core).1, 3);
    assert_eq!(sync_state(&core), (0, 3));

    // 换上的库照常可写。
    store(&core, text("after overwrite"));
    assert_eq!(contents(&core).len(), 3);
    assert_eq!(sync_state(&core), (1, 4));
    block_on(core.shutdown()).unwrap();
    block_on(source.core.shutdown()).unwrap();
}

#[test]
fn export_validates_password_and_target() {
    let fixture = Fixture::new();
    let core = fixture.start();
    let target = fixture.root().join("history.kwikpastebak");
    let export = |mode, password: Option<&str>, target: PathBuf| {
        block_on(core.export_history_backup(
            target,
            ExportHistoryBackupOptions {
                mode,
                password: password.map(str::to_owned),
            },
        ))
        .map(|_| ())
        .unwrap_err()
        .to_string()
    };

    assert_eq!(
        export(BackupExportMode::Encrypted, None, target.clone()),
        "请输入备份密码"
    );
    assert_eq!(
        export(BackupExportMode::Encrypted, Some("short"), target.clone()),
        "备份密码至少需要 8 个字符"
    );
    assert_eq!(
        export(BackupExportMode::Plain, Some(PASSWORD), target.clone()),
        "明文备份不应包含密码"
    );
    assert_eq!(
        export(
            BackupExportMode::Plain,
            None,
            fixture.root().join("history.zip")
        ),
        "备份文件后缀必须是 .kwikpastebak"
    );
    assert!(!target.exists());
    block_on(core.shutdown()).unwrap();
}

#[test]
fn backup_paths_are_recognized_from_args() {
    let args = [
        "KwikPaste.exe".to_owned(),
        "--flag".to_owned(),
        "D:/Backups/History.KWIKPASTEBAK".to_owned(),
    ];

    assert_eq!(
        backup_path_from_args(&args),
        Some(PathBuf::from("D:/Backups/History.KWIKPASTEBAK"))
    );
    assert_eq!(backup_path_from_args(&args[..2]), None);
    assert!(inspect_backup_file(Path::new("history.zip")).is_err());
}

#[test]
fn passwords_never_show_up_in_debug_output() {
    let options = ExportHistoryBackupOptions {
        mode: BackupExportMode::Encrypted,
        password: Some(PASSWORD.to_owned()),
    };
    let input = input(Path::new("a.kwikpastebak"), Some(PASSWORD));

    assert!(!format!("{options:?}").contains(PASSWORD));
    assert!(!format!("{input:?}").contains(PASSWORD));
}

// ---- 覆盖导入的兼容与失败回滚 ----

/// 按 1.x 的布局手工打一个明文备份：`prepare` 可以在打包前改动备份库。
fn craft_backup<F>(dir: &Path, prepare: F) -> PathBuf
where
    F: FnOnce(&SqlitePool) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + '_>>,
{
    let rt = crate::runtime::CoreRuntime::new().unwrap();
    let source = dir.join("source");
    let db_path = source.join("clipboard.db");
    fs::create_dir_all(source.join("resources")).unwrap();
    fs::write(
        source.join("settings.json"),
        serde_json::to_string(&json!({"appearance": {"theme": "light"}})).unwrap(),
    )
    .unwrap();

    rt.handle().block_on(async {
        let options = SqliteConnectOptions::new()
            .filename(&db_path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        crate::db::MIGRATOR.run(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO clipboard_items (id, kind, content, content_hash, platform, created_at, updated_at) \
             VALUES ('crafted', 'text', 'crafted text', 'crafted-hash', 'windows', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        prepare(&pool).await;
        checkpoint_database(&pool).await.unwrap();
        pool.close().await;
    });

    let manifest = BackupManifest {
        format_version: FORMAT_VERSION,
        app_name: "KwikPaste".to_owned(),
        app_version: "1.4.0".to_owned(),
        exported_at: Utc::now(),
        platform: "macos".to_owned(),
        encryption: ManifestEncryption::None,
        item_count: 1,
        text_count: 1,
        image_count: 0,
        files_count: 0,
        resource_bytes: 0,
    };
    let target = dir.join("crafted.kwikpastebak");
    let payload = dir.join("payload.zip");
    write_payload_zip(
        &BackupSourcePaths {
            db_path,
            resources_dir: source.join("resources"),
            settings_path: source.join("settings.json"),
        },
        &payload,
        &manifest,
        &target,
    )
    .unwrap();
    write_container(&target, &payload, BackupExportMode::Plain, None).unwrap();
    target
}

/// macOS 导出的库记的是 LF 校验和，Windows 记的是 CRLF：覆盖导入时换成本平台的值，否则迁移直接失败。
#[test]
fn overwrite_adopts_migration_checksums_of_the_other_platform() {
    let temp = tempdir().unwrap();
    let backup = craft_backup(temp.path(), |pool| {
        Box::pin(async move {
            for (version, lf, crlf) in crate::db::init::PUBLISHED {
                let embedded = crate::db::MIGRATOR
                    .iter()
                    .find(|migration| migration.version == version)
                    .unwrap();
                let embedded_hex: String = embedded
                    .checksum
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                let other = if embedded_hex == lf { crlf } else { lf };
                let bytes: Vec<u8> = (0..other.len())
                    .step_by(2)
                    .map(|index| u8::from_str_radix(&other[index..index + 2], 16).unwrap())
                    .collect();
                sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?")
                    .bind(bytes)
                    .bind(version)
                    .execute(pool)
                    .await
                    .unwrap();
            }
        })
    });

    let fixture = Fixture::new();
    let core = fixture.start();
    store(&core, text("local"));

    block_on(core.import_history_backup(input(&backup, None), BackupImportStrategy::Overwrite))
        .unwrap();

    assert_eq!(contents(&core), ["crafted text"]);
    assert_eq!(core.settings().appearance.theme, Theme::Light);
    block_on(core.shutdown()).unwrap();
}

/// 备份来自更新的版本（多了本版本不认识的迁移）：导入失败，当前数据与连接池都原样可用。
#[test]
fn overwrite_failure_keeps_the_live_database() {
    let temp = tempdir().unwrap();
    let backup = craft_backup(temp.path(), |pool| {
        Box::pin(async move {
            sqlx::query(
                "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
                 VALUES (9999, 'from the future', 1, x'00', 0)",
            )
            .execute(pool)
            .await
            .unwrap();
        })
    });

    let fixture = Fixture::new();
    let core = fixture.start();
    block_on(core.update_settings(json!({"appearance": {"theme": "dark"}}))).unwrap();
    store(&core, text("still here"));

    let err =
        block_on(core.import_history_backup(input(&backup, None), BackupImportStrategy::Overwrite))
            .unwrap_err();

    assert!(err.to_string().contains("不兼容"), "{err}");
    assert_eq!(contents(&core), ["still here"]);
    assert_eq!(core.settings().appearance.theme, Theme::Dark);
    assert!(!core.0.watcher_pause.is_paused());
    let staged = sibling(
        &crate::db::db_path(&fixture.paths).unwrap(),
        STAGED_DB_SUFFIX,
    );
    assert!(!staged.exists());
    store(&core, text("still writable"));
    assert_eq!(contents(&core).len(), 2);
    block_on(core.shutdown()).unwrap();
}

#[test]
fn overwrite_with_unreadable_settings_changes_nothing() {
    let temp = tempdir().unwrap();
    let backup = craft_backup(temp.path(), |_| Box::pin(async {}));
    // 把设置换成读不懂的内容后重新打包。
    let extracted = extract_payload_zip(&fs::read(&backup).unwrap()).unwrap();
    fs::write(
        extracted.path().join("config").join("settings.json"),
        b"[not settings",
    )
    .unwrap();
    let broken = temp.path().join("broken.kwikpastebak");
    let payload = temp.path().join("broken.zip");
    write_payload_zip(
        &BackupSourcePaths {
            db_path: extracted.path().join("db").join("clipboard.db"),
            resources_dir: extracted.path().join("resources"),
            settings_path: extracted.path().join("config").join("settings.json"),
        },
        &payload,
        &BackupManifest {
            format_version: FORMAT_VERSION,
            app_name: "KwikPaste".to_owned(),
            app_version: "1.4.0".to_owned(),
            exported_at: Utc::now(),
            platform: "windows".to_owned(),
            encryption: ManifestEncryption::None,
            item_count: 1,
            text_count: 1,
            image_count: 0,
            files_count: 0,
            resource_bytes: 0,
        },
        &broken,
    )
    .unwrap();
    write_container(&broken, &payload, BackupExportMode::Plain, None).unwrap();

    let fixture = Fixture::new();
    let core = fixture.start();
    store(&core, text("untouched"));

    assert!(block_on(
        core.import_history_backup(input(&broken, None), BackupImportStrategy::Overwrite,)
    )
    .is_err());
    assert_eq!(contents(&core), ["untouched"]);
    block_on(core.shutdown()).unwrap();
}

// ---- 1.4.0 写出的备份 ----

/// `tests/fixtures/backup/` 里的两份备份由 1.4.0 的导出代码在 Windows 上写出（明文与加密，
/// 密码 `compat-password-1`）：5 条记录，覆盖收藏 + 备注 + 分组 + 来源应用、置顶的敏感内容、
/// 链接、图片（原图与缩略图）、文件，另有文件类型图标与深色主题。
fn v1_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("backup")
        .join(name)
}

const V1_PASSWORD: &str = "compat-password-1";
const V1_IMAGE: &str = "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2.png";

fn assert_v1_history(fixture: &Fixture, core: &Core) {
    let text = block_on(core.find_item("v1-text")).unwrap().unwrap();
    assert_eq!(text.content, "v1 favorite text in a group");
    assert!(text.is_favorite);
    assert_eq!(text.note.as_deref(), Some("a v1 note"));
    assert_eq!(text.group_id.as_deref(), Some("group-work"));
    assert_eq!(
        text.source_app_id.as_deref(),
        Some("C:\\Program Files\\Editor\\editor.exe")
    );
    let secret = block_on(core.find_item("v1-secret")).unwrap().unwrap();
    assert!(secret.is_sensitive && secret.is_pinned);
    let url = block_on(core.find_item("v1-url")).unwrap().unwrap();
    assert_eq!(url.sub_kind, Some(crate::db::models::ClipboardSubKind::Url));
    let image = block_on(core.find_item("v1-image")).unwrap().unwrap();
    assert_eq!(image.content, V1_IMAGE);
    assert!(core.image_origin_path(V1_IMAGE).unwrap().is_file());
    assert!(block_on(core.find_item("v1-files")).unwrap().is_some());

    let listed = block_on(core.list_items(ClipboardItemQuery::default())).unwrap();
    assert_eq!(listed.total, 5);
    let with_app = listed
        .list
        .iter()
        .find(|view| view.item.id == "v1-text")
        .unwrap();
    assert_eq!(with_app.item.source_app_name.as_deref(), Some("Editor"));
    assert!(with_app
        .source_app_icon_path
        .as_deref()
        .is_some_and(|path| Path::new(path).is_file()));
    assert_eq!(block_on(core.list_groups()).unwrap()[0].name, "Work");
    assert!(fixture
        .paths
        .resources_dir()
        .unwrap()
        .join("file-icons")
        .read_dir()
        .unwrap()
        .next()
        .is_some());
    assert_eq!(core.settings().appearance.theme, Theme::Dark);
}

#[test]
fn v1_plain_backup_merges() {
    let backup = v1_fixture("v1-plain.kwikpastebak");
    assert_eq!(
        inspect_backup_file(&backup).unwrap(),
        BackupContainerMode::Plain
    );
    let fixture = Fixture::new();
    let core = fixture.start();

    let result =
        block_on(core.import_history_backup(input(&backup, None), BackupImportStrategy::Merge))
            .unwrap();

    assert_eq!((result.imported_items, result.skipped_items), (5, 0));
    assert_v1_history(&fixture, &core);
    block_on(core.shutdown()).unwrap();
}

/// 加密备份覆盖导入。夹具在 Windows 上生成，迁移账里是 CRLF 校验和：macOS 上跑这条测试
/// 同时验证了跨平台校验和的换算。
#[test]
fn v1_encrypted_backup_overwrites() {
    let backup = v1_fixture("v1-encrypted.kwikpastebak");
    assert_eq!(
        inspect_backup_file(&backup).unwrap(),
        BackupContainerMode::Encrypted
    );
    let fixture = Fixture::new();
    let core = fixture.start();
    store(&core, text("replaced by the 1.x backup"));

    block_on(core.import_history_backup(
        input(&backup, Some(V1_PASSWORD)),
        BackupImportStrategy::Overwrite,
    ))
    .unwrap();

    assert_v1_history(&fixture, &core);
    assert_eq!(
        block_on(core.list_items(ClipboardItemQuery::default()))
            .unwrap()
            .total,
        5
    );
    block_on(core.shutdown()).unwrap();
}

/// 给 1.x 读的样本：在 `KWIKPASTE_V2_BACKUP_DIR` 写出 2.0 导出的明文与加密备份（密码见 `PASSWORD`），
/// 再用 1.4.0 的导入代码去读，确认 2.0 的备份 1.x 能导入。2.0 新增迁移或设置字段后要重跑。
#[test]
#[ignore = "manual 1.x compatibility check"]
fn export_samples_for_v1() {
    let Some(dir) = std::env::var_os("KWIKPASTE_V2_BACKUP_DIR") else {
        return;
    };
    let dir = PathBuf::from(dir);
    let source = source();
    for (name, mode, password) in [
        ("v2-plain", BackupExportMode::Plain, None),
        ("v2-encrypted", BackupExportMode::Encrypted, Some(PASSWORD)),
    ] {
        block_on(source.core.export_history_backup(
            dir.join(name),
            ExportHistoryBackupOptions {
                mode,
                password: password.map(str::to_owned),
            },
        ))
        .unwrap();
    }
    block_on(source.core.shutdown()).unwrap();
}
