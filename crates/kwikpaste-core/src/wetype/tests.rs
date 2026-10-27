//! 假扩展端测试：只用内存 channel、临时数据库和内存剪贴板，不启动进程或联网。
use super::*;
use crate::clipboard::{ClipboardPayload, ImagePayload, TextPayload};
use crate::db::models::ClipboardItemQuery;
use crate::testing::{block_on, sample_png, Fixture};
use serde_json::json;

fn fake(core: &Core) -> mpsc::Receiver<Command> {
    let (sender, receiver) = mpsc::sync_channel(16);
    *lock(&core.0.wetype.sender) = Some(sender);
    core.0.wetype.ready.store(true, Ordering::Release);
    *lock(&core.0.wetype.state) = WeTypeState {
        running: true,
        paired: true,
        ..WeTypeState::default()
    };
    receiver
}

fn text(core: &Core, value: &str) -> anyhow::Result<ClipboardItem> {
    core.build_item(&ClipboardPayload::Text(TextPayload {
        text: value.into(),
        html: None,
        rtf: None,
    }))?
    .context("text fixture skipped")
}

fn event(core: &Core, value: Event) -> anyhow::Result<()> {
    let core = core.clone();
    block_on(core.clone().hop(async move {
        core.0
            .wetype
            .event(&core.0, value)
            .await
            .map_err(Into::into)
    }))?;
    Ok(())
}

#[test]
fn local_capture_gates_and_limits_are_independent_of_lan() -> anyhow::Result<()> {
    let fixture = Fixture::new();
    let core = fixture.start();
    let receiver = fake(&core);
    let mut item = text(&core, "local text")?;
    assert!(!core.settings().sync.lan.enabled);
    on_local_capture(&core.0, &item, Some(1));
    assert_eq!(
        receiver.try_recv()?,
        Command::SendText {
            text: "local text".into()
        }
    );
    on_local_capture(&core.0, &item, None);
    assert!(receiver.try_recv().is_err());
    item.is_sensitive = true;
    on_local_capture(&core.0, &item, Some(2));
    assert!(receiver.try_recv().is_err());
    item.is_sensitive = false;
    item.kind = ClipboardKind::Files;
    on_local_capture(&core.0, &item, Some(3));
    assert!(receiver.try_recv().is_err());
    item.kind = ClipboardKind::Text;
    block_on(core.update_settings(json!({"extensions":{"wetype":{"sendText":false}}})))?;
    on_local_capture(&core.0, &item, Some(4));
    assert!(receiver.try_recv().is_err());
    block_on(core.update_settings(json!({"extensions":{"wetype":{"sendText":true}}})))?;
    item.content = "x".repeat(WETYPE_MAX_TEXT_BYTES + 1);
    on_local_capture(&core.0, &item, Some(5));
    assert!(receiver.try_recv().is_err());
    item.content = "<b>plain</b>".into();
    item.content_hash = "html".into();
    item.sub_kind = Some(ClipboardSubKind::Html);
    item.search_text = Some("plain".into());
    on_local_capture(&core.0, &item, Some(6));
    assert_eq!(
        receiver.try_recv()?,
        Command::SendText {
            text: "plain".into()
        }
    );
    lock(&core.0.wetype.state).paired = false;
    on_local_capture(&core.0, &item, Some(7));
    assert!(receiver.try_recv().is_err());
    lock(&core.0.wetype.state).paired = true;
    lock(&core.0.wetype.state).running = false;
    on_local_capture(&core.0, &item, Some(8));
    assert!(receiver.try_recv().is_err());
    block_on(core.shutdown())?;
    Ok(())
}

#[test]
fn image_capture_sends_stored_path_and_obeys_gate_and_size() -> anyhow::Result<()> {
    let fixture = Fixture::new();
    let core = fixture.start();
    let receiver = fake(&core);
    let item = core
        .build_item(&ClipboardPayload::Image(ImagePayload {
            bytes: sample_png(3, 2),
            width: 3,
            height: 2,
        }))?
        .context("image skipped")?;
    let path = core.0.images.origin_path(&item.content);
    on_local_capture(&core.0, &item, Some(1));
    assert_eq!(
        receiver.try_recv()?,
        Command::SendImage {
            path: path.to_string_lossy().into_owned()
        }
    );
    block_on(core.update_settings(json!({"extensions":{"wetype":{"sendImage":false}}})))?;
    on_local_capture(&core.0, &item, Some(2));
    assert!(receiver.try_recv().is_err());
    block_on(core.update_settings(json!({"extensions":{"wetype":{"sendImage":true}}})))?;
    let file = std::fs::OpenOptions::new().write(true).open(&path)?;
    file.set_len(WETYPE_MAX_IMAGE_BYTES + 1)?;
    on_local_capture(&core.0, &item, Some(3));
    assert!(receiver.try_recv().is_err());
    block_on(core.shutdown())?;
    Ok(())
}

#[test]
fn received_text_and_jpeg_use_shared_storage_and_retained_origin_names() -> anyhow::Result<()> {
    let fixture = Fixture::new();
    let core = fixture.start();
    let receiver = fake(&core);
    block_on(core.update_settings(json!({"extensions":{"wetype":{"sendText":false,"sendImage":false,"writeClipboard":false}}})))?;
    event(
        &core,
        Event::State {
            paired: true,
            self_name: "KwikPaste".into(),
            devices: vec![WeTypeDevice {
                uin: 42,
                name: "Phone".into(),
            }],
            error: None,
        },
    )?;
    event(
        &core,
        Event::Received {
            from_uin: 42,
            from_name: "Owner's phone".into(),
            content: Content::Text {
                text: "received text".into(),
            },
        },
    )?;
    assert_eq!(receiver.try_recv()?, Command::Acknowledge);
    let inbox = fixture.paths.extensions_dir().join("wetype/state/inbox");
    std::fs::create_dir_all(&inbox)?;
    let path = inbox.join("test.image");
    let image =
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(3, 2, image::Rgb([30, 40, 50])));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Jpeg)?;
    std::fs::write(&path, bytes.into_inner())?;
    event(
        &core,
        Event::Received {
            from_uin: 42,
            from_name: "Owner's phone".into(),
            content: Content::Image {
                path: path.to_string_lossy().into_owned(),
            },
        },
    )?;
    assert_eq!(receiver.try_recv()?, Command::Acknowledge);
    assert!(!path.exists());
    event(
        &core,
        Event::State {
            paired: true,
            self_name: "KwikPaste".into(),
            devices: Vec::new(),
            error: None,
        },
    )?;
    let list = block_on(core.list_items(ClipboardItemQuery::default()))?.list;
    assert_eq!(list.len(), 2);
    for view in &list {
        assert_eq!(view.item.origin_device_id.as_deref(), Some("wetype:42"));
        assert_eq!(view.item.platform, crate::sync::current_platform());
        assert_eq!(view.origin_device_name.as_deref(), Some("Owner's phone"));
        let seq: Option<i64> = block_on(core.clone().hop({
            let core = core.clone();
            let id = view.item.id.clone();
            async move {
                Ok(
                    sqlx::query_scalar("SELECT sync_seq FROM clipboard_items WHERE id = ?")
                        .bind(id)
                        .fetch_one(&core.0.db.pool().await)
                        .await
                        .map_err(anyhow::Error::from)?,
                )
            }
        }))?;
        assert!(seq.is_none());
        on_local_capture(&core.0, &view.item, seq);
        assert!(receiver.try_recv().is_err());
    }
    let image = list
        .iter()
        .find(|v| v.item.kind == ClipboardKind::Image)
        .context("image absent")?;
    assert_eq!((image.item.width, image.item.height), (Some(3), Some(2)));
    let cache = DeviceNames::load(fixture.paths.extensions_dir().join("wetype/devices.json"));
    assert_eq!(cache.display_name("42").as_deref(), Some("Owner's phone"));
    block_on(core.shutdown())?;
    Ok(())
}

#[test]
fn delayed_official_pc_write_is_not_sent_back_even_without_writeback() -> anyhow::Result<()> {
    let fixture = Fixture::new();
    let core = fixture.start();
    let receiver = fake(&core);
    block_on(core.update_settings(json!({"extensions":{"wetype":{"writeClipboard":false}}})))?;
    event(
        &core,
        Event::Received {
            from_uin: 7,
            from_name: "Phone".into(),
            content: Content::Text {
                text: "echo".into(),
            },
        },
    )?;
    assert_eq!(receiver.try_recv()?, Command::Acknowledge);
    let item = text(&core, "echo")?;
    lock(&core.0.wetype.echo).remember_elapsed_for_test(&item.content_hash, Duration::from_secs(8));
    let other = text(&core, "other local content")?;
    on_local_capture(&core.0, &other, Some(1));
    assert!(matches!(receiver.try_recv()?, Command::SendText { .. }));
    on_local_capture(&core.0, &item, Some(2));
    assert!(receiver.try_recv().is_err());
    lock(&core.0.wetype.echo)
        .remember_elapsed_for_test(&item.content_hash, Duration::from_secs(61));
    on_local_capture(&core.0, &item, Some(3));
    assert!(matches!(receiver.try_recv()?, Command::SendText { .. }));
    block_on(core.shutdown())?;
    Ok(())
}

#[test]
fn intentional_skip_acknowledges_but_import_failure_does_not() -> anyhow::Result<()> {
    let fixture = Fixture::new();
    let core = fixture.start();
    let receiver = fake(&core);
    block_on(core.update_settings(
        json!({"clipboard":{"capture":{"text":false,"html":false,"rtf":false}}}),
    ))?;
    event(
        &core,
        Event::Received {
            from_uin: 8,
            from_name: "Phone".into(),
            content: Content::Text {
                text: "skipped".into(),
            },
        },
    )?;
    assert_eq!(receiver.try_recv()?, Command::Acknowledge);
    assert!(block_on(core.list_items(ClipboardItemQuery::default()))?
        .list
        .is_empty());
    assert!(event(
        &core,
        Event::Received {
            from_uin: 8,
            from_name: "Phone".into(),
            content: Content::Image {
                path: "missing".into()
            }
        }
    )
    .is_err());
    assert!(receiver.try_recv().is_err());
    block_on(core.shutdown())?;
    Ok(())
}

#[test]
fn pairing_api_waits_for_success_and_failure_results() -> anyhow::Result<()> {
    for ok in [true, false] {
        let fixture = Fixture::new();
        let core = fixture.start();
        let receiver = fake(&core);
        lock(&core.0.wetype.state).paired = false;
        let peer = core.clone();
        let thread = std::thread::Builder::new().spawn(move || -> anyhow::Result<()> {
            let command = receiver.recv_timeout(Duration::from_secs(5))?;
            assert!(matches!(command, Command::Pair { code, device_name } if code == "123456" && device_name.starts_with("KwikPaste · ")));
            event(&peer, Event::PairResult { ok, error: (!ok).then(|| "invalid code".into()) })
        })?;
        let result = block_on(core.wetype_pair("123456".into()));
        assert_eq!(result.is_ok(), ok);
        if !ok {
            assert_eq!(
                result.err().map(|e| e.to_string()).as_deref(),
                Some("invalid code")
            );
        }
        thread
            .join()
            .map_err(|_| anyhow::anyhow!("fake extension thread failed"))??;
        block_on(core.shutdown())?;
    }
    Ok(())
}

#[test]
fn unpair_waits_until_local_identity_is_dropped_and_stop_fails_pending_pair() -> anyhow::Result<()>
{
    let fixture = Fixture::new();
    let core = fixture.start();
    let receiver = fake(&core);
    let peer = core.clone();
    let thread = std::thread::Builder::new().spawn(move || -> anyhow::Result<()> {
        assert_eq!(
            receiver.recv_timeout(Duration::from_secs(5))?,
            Command::Unpair
        );
        event(
            &peer,
            Event::State {
                paired: false,
                self_name: "Existing identity".into(),
                devices: Vec::new(),
                error: None,
            },
        )?;
        assert!(lock(&peer.0.wetype.unpair).is_some());
        event(
            &peer,
            Event::State {
                paired: false,
                self_name: String::new(),
                devices: Vec::new(),
                error: None,
            },
        )
    })?;
    block_on(core.wetype_unpair())?;
    thread
        .join()
        .map_err(|_| anyhow::anyhow!("fake extension thread failed"))??;
    assert!(!core.wetype_state().paired);
    let (reply, result) = oneshot::channel();
    *lock(&core.0.wetype.pair) = Some(reply);
    block_on(core.clone().hop({
        let core = core.clone();
        async move {
            core.0.wetype.stop().await;
            Ok(())
        }
    }))?;
    assert!(block_on(result)?.is_err());
    assert!(!core.wetype_state().running);
    block_on(core.shutdown())?;
    Ok(())
}

#[test]
fn received_png_duplicate_and_oversize_are_acknowledged() -> anyhow::Result<()> {
    let fixture = Fixture::new();
    let core = fixture.start();
    let receiver = fake(&core);
    block_on(core.update_settings(json!({"extensions":{"wetype":{"writeClipboard":false}}})))?;
    let inbox = fixture.paths.extensions_dir().join("wetype/state/inbox");
    std::fs::create_dir_all(&inbox)?;
    let path = inbox.join("image");
    for _ in 0..2 {
        std::fs::write(&path, sample_png(2, 2))?;
        event(
            &core,
            Event::Received {
                from_uin: 77,
                from_name: "Phone".into(),
                content: Content::Image {
                    path: path.to_string_lossy().into_owned(),
                },
            },
        )?;
        assert_eq!(receiver.try_recv()?, Command::Acknowledge);
        assert!(!path.exists());
    }
    assert_eq!(
        block_on(core.list_items(ClipboardItemQuery::default()))?
            .list
            .len(),
        1
    );
    std::fs::File::create(&path)?.set_len(WETYPE_MAX_IMAGE_BYTES + 1)?;
    event(
        &core,
        Event::Received {
            from_uin: 77,
            from_name: "Phone".into(),
            content: Content::Image {
                path: path.to_string_lossy().into_owned(),
            },
        },
    )?;
    assert_eq!(receiver.try_recv()?, Command::Acknowledge);
    assert!(!path.exists());
    block_on(core.shutdown())?;
    Ok(())
}

#[test]
fn supervision_decision_backoff_and_shutdown() {
    assert_eq!(restart_delay(true, false, 0), Some(Duration::from_secs(1)));
    assert_eq!(restart_delay(true, false, 3), Some(Duration::from_secs(8)));
    assert_eq!(
        restart_delay(true, false, u32::MAX),
        Some(Duration::from_secs(60))
    );
    assert_eq!(restart_delay(false, false, 0), None);
    assert_eq!(restart_delay(true, true, 0), None);
}

#[test]
fn sent_item_written_back_by_the_official_pc_client_is_not_sent_again() -> anyhow::Result<()> {
    let fixture = Fixture::new();
    let core = fixture.start();
    let receiver = fake(&core);
    let item = text(&core, "copied here")?;
    on_local_capture(&core.0, &item, Some(1));
    assert!(matches!(receiver.try_recv()?, Command::SendText { .. }));
    // 官方 PC 客户端收到后写回同样的内容，监听把它当成又一次本机复制。
    on_local_capture(&core.0, &item, Some(2));
    assert!(receiver.try_recv().is_err());
    block_on(core.shutdown())?;
    Ok(())
}

#[test]
fn messages_from_this_computers_official_client_are_not_recorded() -> anyhow::Result<()> {
    let fixture = Fixture::new();
    let core = fixture.start();
    let receiver = fake(&core);
    event(
        &core,
        Event::Received {
            from_uin: 9,
            from_name: crate::sync::default_device_name(),
            content: Content::Text {
                text: "copied on this computer".into(),
            },
        },
    )?;
    assert_eq!(receiver.try_recv()?, Command::Acknowledge);
    assert!(block_on(core.list_items(ClipboardItemQuery::default()))?
        .list
        .is_empty());
    block_on(core.shutdown())?;
    Ok(())
}

#[test]
fn while_the_official_client_runs_the_bridge_only_records() -> anyhow::Result<()> {
    let fixture = Fixture::new();
    fixture
        .platform
        .processes
        .lock()
        .unwrap()
        .push(OFFICIAL_CLIENT_PROCESS.into());
    let core = fixture.start();
    let receiver = fake(&core);
    let item = text(&core, "copied here")?;
    on_local_capture(&core.0, &item, Some(1));
    assert!(receiver.try_recv().is_err());
    event(
        &core,
        Event::Received {
            from_uin: 7,
            from_name: "Phone".into(),
            content: Content::Text {
                text: "from the phone".into(),
            },
        },
    )?;
    assert_eq!(receiver.try_recv()?, Command::Acknowledge);
    let list = block_on(core.list_items(ClipboardItemQuery::default()))?.list;
    // 列表视图不带正文，按来源确认这条已入库。
    assert!(list
        .iter()
        .any(|view| view.item.origin_device_id.as_deref() == Some("wetype:7")));
    assert_eq!(fixture.clipboard.snapshot().text, None);
    block_on(core.shutdown())?;
    Ok(())
}

#[test]
fn official_client_is_recognized_by_its_executable() {
    use std::path::Path;
    assert!(super::is_official_client_exe(Path::new(
        r"C:\Program Files\Tencent\WeType\2.1.4.6\wetype_server.exe"
    )));
    assert!(super::is_official_client_exe(Path::new(
        "c:/program files/tencent/wetype/wetype_server.exe"
    )));
    assert!(!super::is_official_client_exe(Path::new(
        r"C:\Windows\explorer.exe"
    )));
    assert!(!super::is_official_client_exe(Path::new(
        r"C:\Program Files\Tencent\WeChat\WeChat.exe"
    )));
}

#[test]
fn rejected_images_are_cleaned_and_acknowledged_before_the_next_text() -> anyhow::Result<()> {
    let fixture = Fixture::new();
    let core = fixture.start();
    let receiver = fake(&core);
    block_on(core.update_settings(json!({"extensions":{"wetype":{"writeClipboard":false}}})))?;
    let inbox = fixture.paths.extensions_dir().join("wetype/state/inbox");
    std::fs::create_dir_all(&inbox)?;
    let path = inbox.join("rejected");
    for bytes in [
        sample_png(1, crate::imaging::MAX_DIMENSION + 1),
        b"not an image".to_vec(),
        vec![0xff, 0xd8, 0xff, 0xe0],
    ] {
        std::fs::write(&path, bytes)?;
        event(
            &core,
            Event::Received {
                from_uin: 77,
                from_name: "Phone".into(),
                content: Content::Image {
                    path: path.to_string_lossy().into_owned(),
                },
            },
        )?;
        assert_eq!(receiver.try_recv()?, Command::Acknowledge);
        assert!(!path.exists());
    }
    event(
        &core,
        Event::Received {
            from_uin: 77,
            from_name: "Phone".into(),
            content: Content::Text {
                text: "after rejected images".into(),
            },
        },
    )?;
    assert_eq!(receiver.try_recv()?, Command::Acknowledge);
    let list = block_on(core.list_items(ClipboardItemQuery::default()))?.list;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].item.kind, ClipboardKind::Text);
    block_on(core.shutdown())?;
    Ok(())
}
