//! OCR 的队列、搜索、复用与 generation 端到端测试，不使用系统剪贴板。
use super::*;
use crate::{
    clipboard::ClipboardPayload,
    db::{items, models::ClipboardItemQuery},
    testing::{block_on, sample_png, Fixture},
};

fn image(core: &Core, id: &str, time: i64) -> crate::db::models::ClipboardItem {
    let payload = ClipboardPayload::Image(crate::clipboard::ImagePayload {
        bytes: sample_png(24, 24),
        width: 24,
        height: 24,
    });
    let mut item = core.build_item(&payload).unwrap().unwrap();
    item.id = id.into();
    item.content_hash = format!("test-{id}");
    item.created_at = chrono::DateTime::from_timestamp(time, 0).unwrap();
    item.updated_at = item.created_at;
    item
}

fn seed(fixture: &Fixture, core: &Core, id: &str, time: i64) -> crate::db::models::ClipboardItem {
    let item = image(core, id, time);
    block_on(core.store_item(item.clone(), None)).unwrap();
    // fixture 保持 runtime 和临时数据目录存活。
    let _ = fixture;
    item
}

fn enable(core: &Core) {
    block_on(core.update_settings(serde_json::json!({"clipboard":{"ocr":{"enabled":true}}})))
        .unwrap();
}

fn done(text: &str) -> Outcome {
    Outcome::Done {
        text: text.into(),
        language: "zh-Hans-CN".into(),
    }
}

#[test]
fn queue_is_newest_first_and_retries_twice_across_connections() {
    let fixture = Fixture::new();
    let core = fixture.start();
    let old = seed(&fixture, &core, "old", 1_800_000_001);
    let newest = seed(&fixture, &core, "new", 1_800_000_002);
    fixture.runtime.handle().block_on(async {
        let mut connection = connect(&core.0).await.unwrap();
        let next: (String, String, String) = sqlx::query_as(db::NEXT_JOB)
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(next.0, newest.id);
        db::save(
            &mut connection,
            &newest.id,
            &newest.content_hash,
            &Outcome::Failed {
                reason: "failure".into(),
            },
        )
        .await
        .unwrap();
        connection.close().await.unwrap();
        let mut connection = connect(&core.0).await.unwrap();
        for _ in 0..2 {
            let next: (String, String, String) = sqlx::query_as(db::NEXT_JOB)
                .fetch_one(&mut connection)
                .await
                .unwrap();
            assert_eq!(next.0, newest.id);
            db::save(
                &mut connection,
                &newest.id,
                &newest.content_hash,
                &Outcome::Failed {
                    reason: "failure".into(),
                },
            )
            .await
            .unwrap();
        }
        let next: (String, String, String) = sqlx::query_as(db::NEXT_JOB)
            .fetch_one(&mut connection)
            .await
            .unwrap();
        assert_eq!(next.0, old.id);
        db::save(
            &mut connection,
            &old.id,
            &old.content_hash,
            &Outcome::Skipped {
                reason: "large".into(),
            },
        )
        .await
        .unwrap();
        let next: Option<(String, String, String)> = sqlx::query_as(db::NEXT_JOB)
            .fetch_optional(&mut connection)
            .await
            .unwrap();
        assert!(next.is_none());
        connection.close().await.unwrap();
    });
    let status = block_on(core.ocr_status()).unwrap();
    assert_eq!(
        (status.total_images, status.failed, status.pending),
        (2, 2, 0)
    );
    assert!(!status.running);
    block_on(core.shutdown()).unwrap();
}

#[test]
fn ocr_search_matches_rows_totals_select_all_and_flags_with_same_predicate() {
    let fixture = Fixture::new();
    let core = fixture.start();
    let image = seed(&fixture, &core, "image", 1_800_000_001);
    fixture.runtime.handle().block_on(async {
        let mut connection = connect(&core.0).await.unwrap();
        db::save(
            &mut connection,
            &image.id,
            &image.content_hash,
            &done("中文识别 English words 50%_test"),
        )
        .await
        .unwrap();
        connection.close().await.unwrap();
    });
    let query = |keyword: &str| ClipboardItemQuery {
        keyword: Some(keyword.into()),
        ..Default::default()
    };
    assert_eq!(
        block_on(core.list_items(query("English"))).unwrap().total,
        0
    );
    enable(&core);
    for keyword in [
        "中文",
        "中文识别",
        "English",
        "words",
        "50%_",
        "English words",
        "sh wo",
    ] {
        let page = block_on(core.list_items(query(keyword))).unwrap();
        let refs = block_on(core.list_item_refs(query(keyword))).unwrap();
        assert_eq!(page.total, 1, "{keyword}");
        assert_eq!(refs.len(), 1, "{keyword}");
        assert_eq!(page.list[0].item.id, refs[0].id);
        assert!(page.list[0].has_image_text);
        assert!(page.list[0].image_text_matched);
        let actions = &page.list[0].available_actions;
        let save = actions
            .iter()
            .position(|action| *action == crate::presenter::ClipboardAction::SaveImage)
            .unwrap();
        assert_eq!(
            actions[save + 1],
            crate::presenter::ClipboardAction::CopyImageText
        );
    }
    let all = block_on(core.list_items(ClipboardItemQuery::default())).unwrap();
    assert!(!all.list[0].image_text_matched);
    assert!(
        block_on(core.list_item(&image.id))
            .unwrap()
            .unwrap()
            .has_image_text
    );
    block_on(core.update_settings(serde_json::json!({"clipboard":{"ocr":{"enabled":false}}})))
        .unwrap();
    assert_eq!(
        block_on(core.list_items(query("English"))).unwrap().total,
        0
    );
    assert!(block_on(core.image_text(&image.id)).unwrap().is_some());
    block_on(core.shutdown()).unwrap();
}

#[test]
fn copy_image_text_uses_memory_clipboard_and_existing_reuse_rules() {
    let fixture = Fixture::new();
    let core = fixture.start();
    let image = seed(&fixture, &core, "copy", 1_800_000_001);
    enable(&core);
    fixture.runtime.handle().block_on(async {
        let mut connection = connect(&core.0).await.unwrap();
        db::save(
            &mut connection,
            &image.id,
            &image.content_hash,
            &done("中文\nEnglish text"),
        )
        .await
        .unwrap();
        connection.close().await.unwrap();
    });
    let unchanged = fixture.runtime.handle().block_on(async {
        items::find_item_by_id(&core.0.db.pool().await, &image.id)
            .await
            .unwrap()
            .unwrap()
    });
    assert_eq!(unchanged.updated_at, image.updated_at);
    block_on(core.update_settings(serde_json::json!({"clipboard":{"content":{"copyThenHideWindow":true,"updateOnReuse":true}}}))).unwrap();
    assert!(
        block_on(core.copy_image_text(&image.id))
            .unwrap()
            .hide_window
    );
    assert_eq!(
        fixture.clipboard.snapshot().text.as_deref(),
        Some("中文\nEnglish text")
    );
    let reused = fixture.runtime.handle().block_on(async {
        items::find_item_by_id(&core.0.db.pool().await, &image.id)
            .await
            .unwrap()
            .unwrap()
    });
    assert_eq!(reused.use_count, image.use_count + 1);
    block_on(core.shutdown()).unwrap();
}

#[test]
fn late_results_are_rejected_after_delete_clear_import_and_storage_switch() {
    use crate::backup::{
        BackupExportMode, BackupImportStrategy, BackupScope, ExportHistoryBackupOptions,
        ImportHistoryBackupInput,
    };
    let fixture = Fixture::new();
    let core = fixture.start();
    let item = seed(&fixture, &core, "late", 1_800_000_001);
    enable(&core);
    let try_late = |generation| {
        fixture.runtime.handle().block_on(async {
            let mut connection = connect(&core.0).await.unwrap();
            assert!(!write_if_current(
                &core.0,
                generation,
                &mut connection,
                &item.id,
                &item.content_hash,
                &done("obsolete")
            )
            .await
            .unwrap());
            connection.close().await.unwrap();
            assert!(core.image_text(&item.id).await.unwrap().is_none());
        })
    };
    let generation = core.0.ocr.state().generation;
    block_on(core.delete_item(&item.id)).unwrap();
    block_on(core.store_item(item.clone(), None)).unwrap();
    try_late(generation);
    let generation = core.0.ocr.state().generation;
    block_on(core.clear_ocr_data()).unwrap();
    try_late(generation);
    let backup = block_on(core.export_history_backup(
        fixture.root().join("backup.kwikpastebak"),
        ExportHistoryBackupOptions {
            mode: BackupExportMode::Plain,
            password: None,
            scope: BackupScope::default(),
        },
    ))
    .unwrap();
    let generation = core.0.ocr.state().generation;
    block_on(core.import_history_backup(
        ImportHistoryBackupInput {
            path: backup.path.into(),
            password: None,
            import_settings: false,
        },
        BackupImportStrategy::Overwrite,
    ))
    .unwrap();
    try_late(generation);
    let generation = core.0.ocr.state().generation;
    block_on(core.change_storage_location(fixture.root().join("switched"))).unwrap();
    try_late(generation);
    block_on(core.shutdown()).unwrap();
}

#[test]
fn old_settings_default_to_disabled_and_new_setting_roundtrips() {
    let settings: crate::settings::Settings = serde_json::from_str(r#"{"clipboard":{}}"#).unwrap();
    assert!(!settings.clipboard.ocr.enabled);
    let fixture = Fixture::new();
    fixture.write_settings(r#"{"clipboard":{"ocr":{"enabled":true}}}"#);
    let core = fixture.start();
    assert!(core.settings().clipboard.ocr.enabled);
    let value = serde_json::to_value(core.settings()).unwrap();
    assert_eq!(value["clipboard"]["ocr"]["enabled"], true);
    block_on(core.shutdown()).unwrap();
}

/// 片段围绕第一次命中，换行压成空格，命中范围正好框住关键词。
#[test]
fn snippet_centers_on_the_first_match() {
    let text =
        "增值税电子普通发票\n开票日期 2026-10-02\n购买方 名称：快贴科技有限公司 纳税人识别号 9131";
    let found = snippet(text, "开票").unwrap();
    assert!(!found.text.starts_with('…'));
    assert_eq!(&found.text[found.matched.clone()], "开票");
    assert!(!found.text.contains('\n'));

    let long = format!("{}Invoice NUMBER 0402{}", "前".repeat(40), "后".repeat(80));
    let found = snippet(&long, "number").unwrap();
    assert!(found.text.starts_with('…') && found.text.ends_with('…'));
    assert_eq!(&found.text[found.matched.clone()], "NUMBER");

    let fallback = snippet("只有开头", "不存在").unwrap();
    assert_eq!(
        (fallback.text.as_str(), fallback.matched),
        ("只有开头", 0..0)
    );
    assert!(snippet(" \n ", "x").is_none());
}
