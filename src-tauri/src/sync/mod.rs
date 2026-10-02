//! 局域网同步：同一网络里的 KwikPaste 互相发现、用配对码配对，之后复制的文本和图片实时互通。
//!
//! - [`identity`]：本机 X25519 身份，绑定这台电脑；
//! - [`pairing`]：6 位配对码 + SPAKE2，配对后固定对方公钥；
//! - [`transport`]：Noise XX 加密通道；
//! - [`discovery`]：mDNS 广播与发现；
//! - [`service`]：监听、拨号、会话与状态；
//! - [`engine`]：记录与线上格式互转、收到后入库和写剪贴板。

mod discovery;
mod engine;
mod identity;
mod pairing;
mod peers;
mod protocol;
mod service;
mod transport;

use std::sync::{Arc, OnceLock};

use tauri::{AppHandle, Emitter, Manager};

use crate::core::Result;
use crate::db::models::{ClipboardItem, Platform};
use crate::settings::{LanSync, SettingsStore};
use protocol::Message;

pub use service::{LanSyncService, LanSyncState, PairTarget};

/// 1.x 不开放局域网同步（2.0 原生版正式推出）：设置页入口隐藏，服务也不启动，
/// 不监听端口、不广播 mDNS。与前端 `LAN_SYNC_AVAILABLE` 保持一致。
pub const AVAILABLE: bool = false;

/// 注册同步服务并按当前设置启动。应在数据库和剪贴板监听就绪后调用。
pub fn init(app: &AppHandle) -> Result<()> {
    let dir = crate::core::paths::sync_dir(app)?;
    let peers = Arc::new(peers::PeerStore::load(&dir));
    app.manage(LanSyncService::new(peers));
    apply_settings(app);
    Ok(())
}

/// 设置变化后启动 / 停止 / 更新局域网同步。
pub fn apply_settings(app: &AppHandle) {
    if !AVAILABLE {
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(service) = app.try_state::<LanSyncService>() else {
            return;
        };
        service.apply(&app).await;
        emit_state(&app);
    });
}

/// 广播最新状态给前端。
pub fn emit_state(app: &AppHandle) {
    let Some(service) = app.try_state::<LanSyncService>() else {
        return;
    };
    let state = service.snapshot(app);
    if let Err(err) = app.emit(service::LAN_SYNC_STATE_EVENT, state) {
        log::warn!("emit {} failed: {err}", service::LAN_SYNC_STATE_EVENT);
    }
}

/// 本机采集到一条记录后调用：在线设备实时收到，离线设备下次连上时补齐。
pub fn on_local_capture(app: &AppHandle, item: &ClipboardItem) {
    let Some(service) = app.try_state::<LanSyncService>() else {
        return;
    };
    let Some(shared) = service.shared() else {
        return;
    };
    if !shared.has_connections() {
        return;
    }

    let lan = app.state::<SettingsStore>().snapshot().sync.lan;
    if !engine::is_syncable(item, &lan) || !shared.should_send(&item.content_hash) {
        return;
    }

    let app = app.clone();
    let item = item.clone();
    tauri::async_runtime::spawn(async move {
        match engine::wire_from_item(&app, &item, &lan).await {
            Ok(Some((wire, attachment))) => {
                let push = Message::Push {
                    item: wire,
                    live: true,
                };
                shared.broadcast(&push, Arc::new(attachment));
            }
            Ok(None) => {}
            Err(err) => log::warn!("lan sync send failed: {err:#}"),
        }
    });
}

/// 记录来源设备的显示名称（含已取消配对的设备）。
pub fn device_display_name(app: &AppHandle, device_id: &str) -> Option<String> {
    app.try_state::<LanSyncService>()?.display_name(device_id)
}

fn current_platform() -> Platform {
    #[cfg(target_os = "macos")]
    {
        Platform::Macos
    }
    #[cfg(target_os = "windows")]
    {
        Platform::Windows
    }
}

/// 设置里填的设备名；留空时用系统电脑名。
fn effective_device_name(lan: &LanSync) -> String {
    let name = lan.device_name.trim();
    if name.is_empty() {
        default_device_name()
    } else {
        name.to_owned()
    }
}

fn default_device_name() -> String {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(system_device_name).clone()
}

#[cfg(target_os = "windows")]
fn system_device_name() -> String {
    std::env::var("COMPUTERNAME")
        .ok()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Windows".to_owned())
}

#[cfg(target_os = "macos")]
fn system_device_name() -> String {
    // 「电脑名称」（系统设置 → 通用 → 关于本机），比 hostname 更像用户认得的名字。
    std::process::Command::new("scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "Mac".to_owned())
}
