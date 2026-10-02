//! 两个 core 实例在本机回环地址上配对、同步：只监听 127.0.0.1 的随机端口，不广播 mDNS，
//! 剪贴板一律是内存剪贴板。

use std::time::{Duration, Instant};

use serde_json::json;

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
    let fixture = Fixture::new();
    let core = fixture.start();
    block_on(core.update_settings(json!({
        "appearance": {"language": "en-US"},
        "sync": {"lan": {"enabled": true, "deviceName": name}}
    })))
    .unwrap();
    let state = block_on(core.start_lan_sync(LanSyncNetwork::loopback()));
    assert!(state.running, "{:?}", state.error);
    assert_eq!(state.addresses, ["127.0.0.1"]);
    Device { fixture, core }
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
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

fn views(core: &Core) -> Vec<ClipboardItemView> {
    block_on(core.list_items(ClipboardItemQuery {
        limit: 100,
        ..ClipboardItemQuery::default()
    }))
    .unwrap()
    .list
}

fn find_text(core: &Core, content: &str) -> Option<ClipboardItemView> {
    views(core).into_iter().find(|view| {
        view.item.kind == ClipboardKind::Text
            && block_on(core.find_item(&view.item.id))
                .unwrap()
                .is_some_and(|item| item.content == content)
    })
}

fn online(core: &Core, device_id: &str) -> bool {
    core.lan_sync_state()
        .devices
        .iter()
        .any(|device| device.id == device_id && device.online)
}

/// `a` 输入 `b` 显示的配对码，等两边都看到对方在线。
fn pair(a: &Device, b: &Device) -> (String, String) {
    let state_b = b.core.lan_sync_state();
    let code = state_b.pairing_code.clone().unwrap();
    let address = format!("127.0.0.1:{}", state_b.port.unwrap());
    let name = block_on(a.core.pair_lan_device(PairTarget::Address(address), code)).unwrap();
    assert_eq!(name, b.core.settings().sync.lan.device_name);

    let a_id = a.core.lan_sync_state().device_id.unwrap();
    let b_id = state_b.device_id.unwrap();
    wait_until("both sides online", || {
        online(&a.core, &b_id) && online(&b.core, &a_id)
    });
    (a_id, b_id)
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

    block_on(alpha.core.shutdown()).unwrap();
    block_on(beta.core.shutdown()).unwrap();
}

/// 一端离线期间的复制，重新连上后补齐；补齐的是历史，不写剪贴板。
#[test]
fn offline_copies_catch_up_after_reconnect() {
    let alpha = device("Alpha");
    let beta = device("Beta");
    let (alpha_id, _) = pair(&alpha, &beta);
    // 等首轮补齐把进度记下，再让 beta 下线。
    wait_until("beta cursor", || {
        beta.core
            .0
            .sync
            .peers()
            .get(&alpha_id)
            .is_some_and(|peer| peer.cursor.is_some())
    });

    block_on(
        beta.core
            .update_settings(json!({"sync": {"lan": {"enabled": false}}})),
    )
    .unwrap();
    wait_until("beta stopped", || !beta.core.lan_sync_state().running);
    std::thread::sleep(Duration::from_millis(50));
    store(&alpha.core, text("copied while beta was away"));

    block_on(
        beta.core
            .update_settings(json!({"sync": {"lan": {"enabled": true}}})),
    )
    .unwrap();
    wait_until("caught up on beta", || {
        find_text(&beta.core, "copied while beta was away").is_some()
    });
    assert_ne!(
        beta.fixture.clipboard.snapshot().text.as_deref(),
        Some("copied while beta was away")
    );

    block_on(alpha.core.shutdown()).unwrap();
    block_on(beta.core.shutdown()).unwrap();
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
    let address = format!("127.0.0.1:{}", state_b.port.unwrap());
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
    let own = alpha.core.lan_sync_state();
    assert_eq!(
        pair(
            PairTarget::Address(format!("127.0.0.1:{}", own.port.unwrap())),
            own.pairing_code.as_deref().unwrap()
        ),
        label(Language::EnUS, Key::SyncSelfPairing)
    );

    // 刷新配对码后次数重置。
    let refreshed = block_on(beta.core.refresh_lan_pairing_code()).unwrap();
    assert_eq!(refreshed.pairing_attempts_left, pairing::PAIRING_ATTEMPTS);

    block_on(
        alpha
            .core
            .update_settings(json!({"sync": {"lan": {"enabled": false}}})),
    )
    .unwrap();
    wait_until("alpha stopped", || !alpha.core.lan_sync_state().running);
    assert_eq!(
        pair(PairTarget::Address(address), &code),
        label(Language::EnUS, Key::SyncNotRunning)
    );

    block_on(alpha.core.shutdown()).unwrap();
    block_on(beta.core.shutdown()).unwrap();
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
