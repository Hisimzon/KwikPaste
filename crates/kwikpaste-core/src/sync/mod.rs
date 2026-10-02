//! 局域网同步：同一网络里的 KwikPaste 互相发现、用配对码配对，之后复制的文本和图片实时互通。
//!
//! - [`identity`]：本机 X25519 身份，绑定这台电脑；
//! - [`pairing`]：6 位配对码 + SPAKE2，配对后固定对方公钥；
//! - [`transport`]：Noise XX 加密通道；
//! - [`discovery`]：mDNS 广播与发现；
//! - [`service`]：监听、拨号、会话与状态；
//! - [`engine`]：记录与线上格式互转、收到后入库和写剪贴板。
//!
//! 协议照搬 1.x（1.x 从未对外开放同步，2.0 可以重新定协议，见 PLAN F9）。
//! 宿主调用 [`Core::start_lan_sync`] 后服务才按设置启停；本机身份与已配对设备在
//! [`crate::CorePaths::sync_dir`]，不随数据目录搬走，也不进备份。

mod discovery;
mod engine;
mod identity;
mod pairing;
mod peers;
mod protocol;
mod service;
mod transport;

use std::sync::{Arc, OnceLock};

use crate::db::models::{ClipboardItem, Platform};
use crate::error::Result;
use crate::events::CoreEvent;
use crate::root::{Core, CoreInner};
use crate::settings::LanSync;
use protocol::Message;

pub(crate) use peers::PeerStore;
pub(crate) use service::LanSyncService;
pub use service::{
    LanDeviceView, LanNearbyView, LanSyncNetwork, LanSyncState, PairTarget, DEFAULT_PORT,
};

impl Core {
    /// 启用局域网同步服务：按当前设置启动（`sync.lan.enabled` 为假时只是待命），之后设置变化
    /// 自动启停、改名重新广播。`network` 正式运行用 [`LanSyncNetwork::default`]，
    /// 测试与自测用 [`LanSyncNetwork::loopback`]。重复调用只返回当前状态。
    pub async fn start_lan_sync(&self, network: LanSyncNetwork) -> LanSyncState {
        let core = self.clone();
        let started = self
            .hop(async move {
                if core.0.sync.enable(network) {
                    core.0.sync.apply(&core.0).await;
                    core.0.events.emit(CoreEvent::LanSyncChanged);
                }
                Ok(core.0.sync.snapshot(&core.0))
            })
            .await;
        started.unwrap_or_else(|_| self.lan_sync_state())
    }

    /// 偏好页的同步状态：本机身份与端口、配对码、已配对设备（是否在线）、附近设备。
    pub fn lan_sync_state(&self) -> LanSyncState {
        self.0.sync.snapshot(&self.0)
    }

    /// 换一个新的配对码（剩余尝试次数同时重置）。
    pub async fn refresh_lan_pairing_code(&self) -> Result<LanSyncState> {
        let core = self.clone();
        self.hop(async move {
            core.0.sync.refresh_pairing_code(&core.0)?;
            core.0.events.emit(CoreEvent::LanSyncChanged);
            Ok(core.0.sync.snapshot(&core.0))
        })
        .await
    }

    /// 用对方设备上显示的配对码配对附近设备或手动输入的地址；成功返回对方设备名。
    pub async fn pair_lan_device(&self, target: PairTarget, code: String) -> Result<String> {
        let core = self.clone();
        self.hop(async move { core.0.sync.pair(&core.0, target, &code).await })
            .await
    }

    /// 取消与一台设备的配对；对方在线时一并通知它。
    pub async fn remove_lan_device(&self, device_id: String) -> Result<()> {
        let core = self.clone();
        self.hop(async move {
            core.0.sync.remove_device(&device_id).await;
            core.0.events.emit(CoreEvent::LanSyncChanged);
            Ok(())
        })
        .await
    }
}

/// 同步相关设置变了（或整份替换）：已启用时在后台按新设置启停 / 更新运行时。
pub(crate) fn settings_changed(core: &CoreInner) {
    let Some(core) = core.sync.bound_core() else {
        return;
    };
    core.rt.clone().spawn(async move {
        core.sync.apply(&core).await;
        core.events.emit(CoreEvent::LanSyncChanged);
    });
}

/// core 关闭时停掉同步运行时：断开连接、撤销 mDNS 广播。
pub(crate) async fn shutdown(core: &CoreInner) {
    core.sync.stop().await;
}

/// 本机采集到一条记录后调用：在线设备实时收到，离线设备下次连上时补齐。
pub(crate) fn on_local_capture(core: &CoreInner, item: &ClipboardItem) {
    let Some(shared) = core.sync.shared() else {
        return;
    };
    if !shared.has_connections() {
        return;
    }

    let lan = core.settings.snapshot().sync.lan;
    if !engine::is_syncable(item, &lan) || !shared.should_send(&item.content_hash) {
        return;
    }
    let Some(owner) = shared.core() else {
        return;
    };

    let item = item.clone();
    core.rt.spawn(async move {
        match engine::wire_from_item(&owner, &item, &lan).await {
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

fn current_platform() -> Platform {
    #[cfg(target_os = "macos")]
    {
        Platform::Macos
    }
    #[cfg(not(target_os = "macos"))]
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

#[cfg(not(target_os = "macos"))]
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

#[cfg(test)]
mod tests;
