//! 几个 core 实例在本机回环地址上配对、同步：只监听 127.0.0.1 与 ::1 的随机端口，不广播 mDNS，
//! 剪贴板一律是内存剪贴板。

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::protocol::VersionRange;
use super::*;
use crate::clipboard::{MemoryClipboard, MemoryState};
use crate::db::models::{ClipboardItemQuery, ClipboardKind};
use crate::error::AppError;
use crate::i18n::commands::{label, Key};
use crate::presenter::ClipboardItemView;
use crate::settings::Language;
use crate::testing::{block_on, sample_png, Fixture};

struct Device {
    fixture: Fixture,
    core: Core,
}

/// 启动一个开启了同步的 core，设备名固定，不依赖本机电脑名。
fn device(name: &str) -> Device {
    device_with(name, json!({}), None)
}

/// 同 [`device`]，另外合并一段同步设置，并可以假装支持另一个协议版本范围。
fn device_with(name: &str, lan: Value, supported: Option<VersionRange>) -> Device {
    let fixture = Fixture::new();
    let core = fixture.start();
    let mut settings = json!({"enabled": true, "deviceName": name});
    if let (Some(settings), Value::Object(extra)) = (settings.as_object_mut(), lan) {
        settings.extend(extra);
    }
    block_on(core.update_settings(json!({
        "appearance": {"language": "en-US"},
        "sync": {"lan": settings}
    })))
    .unwrap();
    if let Some(range) = supported {
        core.0.sync.override_supported(range);
    }
    let state = block_on(core.start_lan_sync(LanSyncNetwork::loopback()));
    assert!(state.running, "{:?}", state.error);
    assert_eq!(state.addresses, ["127.0.0.1", "::1"]);
    Device { fixture, core }
}

/// 有的 CI 容器没有配置 `::1`，这时跳过 IPv6 部分。
fn ipv6_loopback_available() -> bool {
    std::net::TcpListener::bind("[::1]:0").is_ok()
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(40));
    }
}

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

/// 压缩不了的 PNG：伪随机像素，体积接近原始像素数据。
fn noise_png(w: u32, h: u32) -> Vec<u8> {
    let mut seed = 0x2545_f491_u32;
    let buf = image::RgbaImage::from_fn(w, h, |_, _| {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let [r, g, b, _] = seed.to_le_bytes();
        image::Rgba([r, g, b, 255])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(buf)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn views(core: &Core) -> Vec<ClipboardItemView> {
    block_on(core.list_items(ClipboardItemQuery {
        limit: 1000,
        ..ClipboardItemQuery::default()
    }))
    .unwrap()
    .list
}

fn content(core: &Core, view: &ClipboardItemView) -> String {
    block_on(core.find_item(&view.item.id))
        .unwrap()
        .map(|item| item.content)
        .unwrap_or_default()
}

fn find_text(core: &Core, wanted: &str) -> Option<ClipboardItemView> {
    views(core)
        .into_iter()
        .find(|view| view.item.kind == ClipboardKind::Text && content(core, view) == wanted)
}

fn online(core: &Core, device_id: &str) -> bool {
    core.lan_sync_state()
        .devices
        .iter()
        .any(|device| device.id == device_id && device.online)
}

fn watermark(core: &Core, device_id: &str) -> u64 {
    core.0
        .sync
        .peers()
        .get(device_id)
        .map_or(0, |peer| peer.watermark)
}

fn loopback_v4(device: &Device) -> String {
    format!("127.0.0.1:{}", device.core.lan_sync_state().port.unwrap())
}

/// `a` 输入 `b` 显示的配对码，等两边都看到对方在线。
fn pair(a: &Device, b: &Device) -> (String, String) {
    let state_b = b.core.lan_sync_state();
    let code = state_b.pairing_code.clone().unwrap();
    let name = block_on(
        a.core
            .pair_lan_device(PairTarget::Address(loopback_v4(b)), code),
    )
    .unwrap();
    assert_eq!(name, b.core.settings().sync.lan.device_name);

    let a_id = a.core.lan_sync_state().device_id.unwrap();
    let b_id = state_b.device_id.unwrap();
    wait_until("both sides online", || {
        online(&a.core, &b_id) && online(&b.core, &a_id)
    });
    (a_id, b_id)
}

fn set_lan(core: &Core, lan: Value) {
    block_on(core.update_settings(json!({ "sync": { "lan": lan } }))).unwrap();
}

fn shutdown(devices: &[&Device]) {
    for device in devices {
        block_on(device.core.shutdown()).unwrap();
    }
}

#[test]
fn paired_devices_sync_live_copies_over_loopback() {
    let alpha = device("Alpha");
    let beta = device("Beta");
    let (alpha_id, beta_id) = pair(&alpha, &beta);
    assert!(beta.fixture.take_events().iter().any(|event| matches!(
        event,
        CoreEvent::LanDevicePaired { name } if name == "Alpha"
    )));
    // 配对成功后显示方换了新的配对码。
    assert_eq!(
        beta.core.lan_sync_state().pairing_attempts_left,
        pairing::PAIRING_ATTEMPTS
    );

    store(&alpha.core, text("copied on alpha"));
    wait_until("text on beta", || {
        find_text(&beta.core, "copied on alpha").is_some()
    });
    let received = find_text(&beta.core, "copied on alpha").unwrap();
    assert_eq!(
        received.item.origin_device_id.as_deref(),
        Some(alpha_id.as_str())
    );
    assert_eq!(received.origin_device_name.as_deref(), Some("Alpha"));
    // 实时收到的记录按默认设置写进本机剪贴板（这里是内存剪贴板）。
    wait_until("beta clipboard", || {
        beta.fixture.clipboard.snapshot().text.as_deref() == Some("copied on alpha")
    });
    // 实时推送之后的检查点推进水位线，重连时不再补发这条。
    wait_until("beta watermark after a live copy", || {
        watermark(&beta.core, &alpha_id) == 1
    });

    // 敏感内容不同步；之后的普通记录照常到达，说明敏感的那条是被跳过而不是还在路上。
    store(&alpha.core, text("AKIAIOSFODNN7EXAMPLE"));
    store(&beta.core, text("copied on beta"));
    wait_until("text on alpha", || {
        find_text(&alpha.core, "copied on beta").is_some()
    });
    store(&alpha.core, text("marker after secret"));
    wait_until("marker on beta", || {
        find_text(&beta.core, "marker after secret").is_some()
    });
    assert!(find_text(&beta.core, "AKIAIOSFODNN7EXAMPLE").is_none());
    // 敏感内容占的序号不推送，但检查点照样越过它。
    wait_until("beta watermark past the secret", || {
        watermark(&beta.core, &alpha_id) == 3
    });

    store(
        &alpha.core,
        MemoryState {
            png: Some(sample_png(14, 9)),
            ..MemoryState::default()
        },
    );
    wait_until("image on beta", || {
        views(&beta.core)
            .iter()
            .any(|view| view.item.kind == ClipboardKind::Image)
    });
    let image = views(&beta.core)
        .into_iter()
        .find(|view| view.item.kind == ClipboardKind::Image)
        .unwrap();
    assert!(beta
        .core
        .image_origin_path(&image.item.content)
        .unwrap()
        .is_file());
    assert_eq!((image.item.width, image.item.height), (Some(14), Some(9)));

    // 取消配对：对方在线时一并移除；已收到的记录仍显示来源设备名。
    block_on(alpha.core.remove_lan_device(beta_id.clone())).unwrap();
    assert!(alpha.core.lan_sync_state().devices.is_empty());
    wait_until("beta forgets alpha", || {
        beta.core.lan_sync_state().devices.is_empty()
    });
    assert_eq!(
        find_text(&beta.core, "copied on alpha")
            .unwrap()
            .origin_device_name
            .as_deref(),
        Some("Alpha")
    );

    shutdown(&[&alpha, &beta]);
}

/// 一端离线期间的复制，重新连上后按水位线补齐；补齐的是历史，不写剪贴板。
#[test]
fn offline_copies_catch_up_after_reconnect() {
    let alpha = device("Alpha");
    let beta = device("Beta");
    let (alpha_id, _) = pair(&alpha, &beta);

    set_lan(&beta.core, json!({"enabled": false}));
    wait_until("beta stopped", || !beta.core.lan_sync_state().running);
    store(&alpha.core, text("copied while beta was away"));

    set_lan(&beta.core, json!({"enabled": true}));
    wait_until("caught up on beta", || {
        find_text(&beta.core, "copied while beta was away").is_some()
    });
    wait_until("beta watermark", || watermark(&beta.core, &alpha_id) == 1);
    assert_ne!(
        beta.fixture.clipboard.snapshot().text.as_deref(),
        Some("copied while beta was away")
    );

    shutdown(&[&alpha, &beta]);
}

/// 发送方的数据库重建过（序号从头编、代次变了）：接收方记着的水位线作废，从头补齐。
#[test]
fn catch_up_restarts_when_the_senders_sequence_is_rebuilt() {
    let alpha = device("Alpha");
    for index in 0..3 {
        store(&alpha.core, text(&format!("before rebuild {index}")));
    }
    let beta = device("Beta");
    let (alpha_id, _) = pair(&alpha, &beta);
    wait_until("beta watermark", || watermark(&beta.core, &alpha_id) == 3);

    set_lan(&beta.core, json!({"enabled": false}));
    wait_until("beta stopped", || !beta.core.lan_sync_state().running);
    block_on(alpha.core.hop({
        let core = alpha.core.clone();
        async move {
            let pool = core.0.db.pool().await;
            sqlx::query("UPDATE clipboard_items SET sync_seq = NULL")
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("UPDATE sync_counter SET value = 0, created_at = 'rebuilt' WHERE id = 1")
                .execute(&pool)
                .await
                .unwrap();
            Ok(())
        }
    }))
    .unwrap();
    store(&alpha.core, text("first copy after rebuild"));

    set_lan(&beta.core, json!({"enabled": true}));
    wait_until("caught up after rebuild", || {
        find_text(&beta.core, "first copy after rebuild").is_some()
    });
    wait_until("watermark of the new epoch", || {
        beta.core
            .0
            .sync
            .peers()
            .get(&alpha_id)
            .is_some_and(|peer| peer.epoch.as_deref() == Some("rebuilt") && peer.watermark == 1)
    });

    shutdown(&[&alpha, &beta]);
}

/// 配对前就有的记录分页补齐（每页 50 条），顺序与发送方一致；收到的记录不带同步序号，
/// 不会再被当成本机复制补齐给别人。
#[test]
fn catch_up_pages_through_history_and_never_relays_received_items() {
    const COUNT: usize = 120;
    let alpha = device("Alpha");
    for index in 0..COUNT {
        store(&alpha.core, text(&format!("history {index}")));
    }
    let beta = device("Beta");
    let gamma = device("Gamma");

    let (alpha_id, beta_id) = pair(&alpha, &beta);
    wait_until("history on beta", || views(&beta.core).len() == COUNT);
    wait_until("beta watermark", || {
        watermark(&beta.core, &alpha_id) == COUNT as u64
    });
    let list = views(&beta.core);
    assert_eq!(
        content(&beta.core, &list[0]),
        format!("history {}", COUNT - 1)
    );
    assert_eq!(content(&beta.core, &list[COUNT - 1]), "history 0");
    // alpha 没有从 beta 收到任何东西：beta 收到的记录不算 beta 的本机复制。
    assert_eq!(views(&alpha.core).len(), COUNT);
    assert_eq!(watermark(&alpha.core, &beta_id), 0);

    // gamma 和 beta 配对：beta 的历史全是从 alpha 收来的，一条也不转给 gamma。
    let (_, beta_id_seen_by_gamma) = pair(&gamma, &beta);
    store(&beta.core, text("beta's own copy"));
    wait_until("beta's own copy on gamma", || {
        find_text(&gamma.core, "beta's own copy").is_some()
    });
    set_lan(&gamma.core, json!({"enabled": false}));
    wait_until("gamma stopped", || !gamma.core.lan_sync_state().running);
    set_lan(&gamma.core, json!({"enabled": true}));
    wait_until("gamma reconnected", || {
        online(&gamma.core, &beta_id_seen_by_gamma)
    });
    wait_until("gamma watermark", || {
        watermark(&gamma.core, &beta_id_seen_by_gamma) >= 1
    });
    assert_eq!(views(&gamma.core).len(), 1);

    shutdown(&[&alpha, &beta, &gamma]);
}

/// 附件上限按对方申报的来：超过接收方上限的图片发送方直接跳过，连接不受影响。
#[test]
fn images_over_the_receivers_limit_are_skipped_by_the_sender() {
    let alpha = device("Alpha");
    let beta = device_with("Beta", json!({"maxImageMb": 1}), None);
    let (alpha_id, beta_id) = pair(&alpha, &beta);

    let big = noise_png(800, 800);
    assert!(big.len() > 1024 * 1024);
    store(
        &alpha.core,
        MemoryState {
            png: Some(big),
            ..MemoryState::default()
        },
    );
    store(&alpha.core, text("after the big image"));
    wait_until("marker on beta", || {
        find_text(&beta.core, "after the big image").is_some()
    });
    assert!(online(&alpha.core, &beta_id) && online(&beta.core, &alpha_id));
    assert!(!views(&beta.core)
        .iter()
        .any(|view| view.item.kind == ClipboardKind::Image));

    shutdown(&[&alpha, &beta]);
}

/// 没有共同的协议版本：拒绝时说明是哪一边旧了。
#[test]
fn incompatible_versions_tell_which_side_to_upgrade() {
    let current = device("Current");
    let future = device_with("Future", json!({}), Some(VersionRange { min: 3, max: 3 }));
    let pair = |from: &Device, to: &Device| {
        let code = to.core.lan_sync_state().pairing_code.unwrap();
        sync_message(
            block_on(
                from.core
                    .pair_lan_device(PairTarget::Address(loopback_v4(to)), code),
            )
            .unwrap_err(),
        )
    };

    assert_eq!(
        pair(&current, &future),
        label(Language::EnUS, Key::SyncSelfOutdated)
    );
    assert_eq!(
        pair(&future, &current),
        label(Language::EnUS, Key::SyncPeerOutdated)
    );
    // 版本不兼容不消耗配对次数。
    assert_eq!(
        current.core.lan_sync_state().pairing_attempts_left,
        pairing::PAIRING_ATTEMPTS
    );

    shutdown(&[&current, &future]);
}

/// 两边都换了端口、没有 mDNS 时，按地址手动连回已配对设备（IPv6 回环）；未配对的设备连不上。
#[test]
fn paired_devices_reconnect_by_address_over_ipv6() {
    let alpha = device("Alpha");
    let beta = device("Beta");
    let stranger = device("Stranger");
    let (alpha_id, beta_id) = pair(&alpha, &beta);

    for device in [&alpha, &beta] {
        set_lan(&device.core, json!({"enabled": false}));
        wait_until("stopped", || !device.core.lan_sync_state().running);
    }
    for device in [&alpha, &beta] {
        set_lan(&device.core, json!({"enabled": true}));
        wait_until("running", || device.core.lan_sync_state().running);
    }

    let beta_port = beta.core.lan_sync_state().port.unwrap();
    let address = if ipv6_loopback_available() {
        format!("[::1]:{beta_port}")
    } else {
        format!("127.0.0.1:{beta_port}")
    };
    let name = block_on(alpha.core.connect_lan_device(address.clone())).unwrap();
    assert_eq!(name, "Beta");
    wait_until("reconnected", || {
        online(&alpha.core, &beta_id) && online(&beta.core, &alpha_id)
    });
    // 记下新地址，之后断线自动重连用它。
    wait_until("new address remembered", || {
        alpha
            .core
            .0
            .sync
            .peers()
            .get(&beta_id)
            .and_then(|peer| peer.address)
            == Some(address.clone())
    });
    store(&beta.core, text("after manual connect"));
    wait_until("text on alpha", || {
        find_text(&alpha.core, "after manual connect").is_some()
    });

    let connect = |address: String| {
        sync_message(block_on(alpha.core.connect_lan_device(address)).unwrap_err())
    };
    assert_eq!(
        connect(loopback_v4(&stranger)),
        label(Language::EnUS, Key::SyncNotPaired)
    );
    assert_eq!(
        connect("[fe80::1%no-such-interface]:1".to_owned()),
        label(Language::EnUS, Key::SyncInvalidAddress)
    );

    shutdown(&[&alpha, &beta, &stranger]);
}

fn sync_message(err: AppError) -> String {
    match err {
        AppError::Sync(message) => message,
        other => panic!("expected a sync error, got {other:?}"),
    }
}

#[test]
fn pairing_errors_are_reported_in_the_ui_language() {
    let alpha = device("Alpha");
    let beta = device("Beta");
    let state_b = beta.core.lan_sync_state();
    let address = loopback_v4(&beta);
    let code = state_b.pairing_code.unwrap();
    let wrong = if code == "000000" { "111111" } else { "000000" };
    let pair = |target: PairTarget, code: &str| {
        sync_message(block_on(alpha.core.pair_lan_device(target, code.to_owned())).unwrap_err())
    };

    assert_eq!(
        pair(PairTarget::Address(address.clone()), "12-34"),
        label(Language::EnUS, Key::SyncInvalidCode)
    );
    assert_eq!(
        pair(PairTarget::Address(address.clone()), wrong),
        label(Language::EnUS, Key::SyncWrongCode)
    );
    assert_eq!(
        beta.core.lan_sync_state().pairing_attempts_left,
        pairing::PAIRING_ATTEMPTS - 1
    );
    assert_eq!(
        pair(PairTarget::Address("not an address:x".to_owned()), &code),
        label(Language::EnUS, Key::SyncInvalidAddress)
    );
    assert_eq!(
        pair(PairTarget::Device("nobody".to_owned()), &code),
        label(Language::EnUS, Key::SyncDeviceNotFound)
    );
    assert_eq!(
        pair(PairTarget::Address(loopback_v4(&alpha)), {
            &alpha.core.lan_sync_state().pairing_code.unwrap()
        }),
        label(Language::EnUS, Key::SyncSelfPairing)
    );

    // 刷新配对码后次数重置。
    let refreshed = block_on(beta.core.refresh_lan_pairing_code()).unwrap();
    assert_eq!(refreshed.pairing_attempts_left, pairing::PAIRING_ATTEMPTS);

    set_lan(&alpha.core, json!({"enabled": false}));
    wait_until("alpha stopped", || !alpha.core.lan_sync_state().running);
    assert_eq!(
        pair(PairTarget::Address(address), &code),
        label(Language::EnUS, Key::SyncNotRunning)
    );

    shutdown(&[&alpha, &beta]);
}

/// 宿主没启用同步服务时：不监听、不连接，偏好页状态显示未运行，设备名照常回填。
#[test]
fn service_stays_idle_until_the_host_starts_it() {
    let fixture = Fixture::new();
    let core = fixture.start();
    block_on(core.update_settings(json!({"sync": {"lan": {"enabled": true}}}))).unwrap();

    let state = core.lan_sync_state();
    assert!(!state.running);
    assert!(state.port.is_none());
    assert!(state.addresses.is_empty());
    assert!(matches!(
        block_on(core.refresh_lan_pairing_code()),
        Err(AppError::Sync(_))
    ));
    block_on(core.shutdown()).unwrap();
}
