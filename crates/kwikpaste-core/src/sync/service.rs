//! 局域网同步运行时：监听端口、mDNS 发现、拨号、配对与同步会话。
//!
//! 每台设备既监听也主动连接，没有主机角色；同一对设备只保留一条连接
//! （两边同时连上时，保留设备 id 较小一方发起的那条）。连上后先互相请求补齐，
//! 之后本机每次复制都实时推给所有在线设备。
//!
//! 监听地址、端口与要不要 mDNS 由 [`LanSyncNetwork`] 决定：正式运行监听所有网卡并广播，
//! 自动测试只用本机回环地址、不广播。

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant};

use anyhow::anyhow;
use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, watch, Notify, OwnedSemaphorePermit, Semaphore};

use super::discovery::{Discovery, DiscoveryEvent};
use super::engine::{self, EchoFilter};
use super::identity::{self, device_id_for, DeviceIdentity};
use super::pairing::{self, PairingCode, Role, Transcript};
use super::peers::{PeerStore, TrustedPeer};
use super::protocol::{
    from_hex, to_hex, DeviceInfo, Intent, Message, RejectReason, PROTOCOL_VERSION,
};
use super::transport::{handshake, Handshaken, SecureReader, SecureWriter, HANDSHAKE_TIMEOUT};
use super::{current_platform, default_device_name, effective_device_name};
use crate::db::models::Platform;
use crate::error::{AppError, Result};
use crate::events::CoreEvent;
use crate::i18n::commands::Key;
use crate::root::CoreInner;

/// 默认监听端口，被占用时改用随机端口（mDNS 广播实际端口）。
pub const DEFAULT_PORT: u16 = 41573;

const DIAL_INTERVAL: Duration = Duration::from_secs(15);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(4);
const PING_INTERVAL: Duration = Duration::from_secs(20);
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const OUTGOING_QUEUE: usize = 64;
const MAX_PENDING_HANDSHAKES: usize = 16;
const BACKOFF_BASE_SECS: u64 = 5;
const BACKOFF_MAX_SECS: u64 = 120;
const UNPAIR_FLUSH_DELAY: Duration = Duration::from_millis(300);

type Outgoing = (Message, Arc<Vec<u8>>);

/// 同步服务的网络行为。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LanSyncNetwork {
    /// 监听地址。
    pub listen_ip: IpAddr,
    /// 首选端口，被占用时改用随机端口；0 表示直接用随机端口。
    pub port: u16,
    /// 是否在局域网里用 mDNS 广播本机并发现附近设备。
    pub discovery: bool,
}

impl Default for LanSyncNetwork {
    /// 正式运行：监听所有 IPv4 网卡的 41573 端口并用 mDNS 广播。
    fn default() -> Self {
        Self {
            listen_ip: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            port: DEFAULT_PORT,
            discovery: true,
        }
    }
}

impl LanSyncNetwork {
    /// 只在本机回环地址上监听、随机端口、不广播，供自动测试与自测使用。
    pub fn loopback() -> Self {
        Self {
            listen_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            port: 0,
            discovery: false,
        }
    }

    /// 界面上给用户看的本机地址：回环模式只有回环地址，否则列出可被别的设备连接的地址。
    fn advertised_addresses(&self) -> Vec<String> {
        if self.listen_ip.is_loopback() {
            return vec![self.listen_ip.to_string()];
        }

        local_addresses()
    }
}

/// 偏好页展示的状态快照。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanSyncState {
    pub running: bool,
    pub error: Option<String>,
    pub device_id: Option<String>,
    pub device_name: String,
    pub default_device_name: String,
    pub platform: Platform,
    pub port: Option<u16>,
    pub addresses: Vec<String>,
    pub pairing_code: Option<String>,
    pub pairing_attempts_left: u8,
    pub devices: Vec<LanDeviceView>,
    pub nearby: Vec<LanNearbyView>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanDeviceView {
    pub id: String,
    pub name: String,
    pub platform: Platform,
    pub online: bool,
    pub address: Option<String>,
    pub last_seen_at: Option<DateTime<Utc>>,
    pub paired_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanNearbyView {
    pub id: String,
    pub name: String,
    pub platform: Platform,
    pub address: String,
}

/// 配对目标：附近设备列表里的一台，或手动输入的地址。
pub enum PairTarget {
    Device(String),
    Address(String),
}

/// 由 `Core` 持有。已配对设备启动时就读入（列表显示「来自 xxx」要用）；网络部分要宿主调用
/// [`crate::Core::start_lan_sync`] 后才按设置启动。
pub struct LanSyncService {
    core: OnceLock<Weak<CoreInner>>,
    network: Mutex<Option<LanSyncNetwork>>,
    peers: Arc<PeerStore>,
    lifecycle: tokio::sync::Mutex<Option<Runtime>>,
    shared: RwLock<Option<Arc<Shared>>>,
    error: Mutex<Option<String>>,
}

struct Runtime {
    shared: Arc<Shared>,
    discovery: Option<Discovery>,
    shutdown: watch::Sender<bool>,
    advertised_name: String,
}

impl LanSyncService {
    pub fn new(peers: Arc<PeerStore>) -> Self {
        Self {
            core: OnceLock::new(),
            network: Mutex::new(None),
            peers,
            lifecycle: tokio::sync::Mutex::new(None),
            shared: RwLock::new(None),
            error: Mutex::new(None),
        }
    }

    /// `Core` 建好后登记自己，运行时的会话据此访问设置、数据库与剪贴板。
    pub(crate) fn bind(&self, core: Weak<CoreInner>) {
        let _ = self.core.set(core);
    }

    pub(crate) fn peers(&self) -> &PeerStore {
        &self.peers
    }

    pub(super) fn shared(&self) -> Option<Arc<Shared>> {
        self.shared.read().expect("lan sync poisoned").clone()
    }

    /// 宿主启用同步服务，之后按设置启停；返回 `false` 表示之前已经启用过。
    pub(super) fn enable(&self, network: LanSyncNetwork) -> bool {
        let mut current = self.network.lock().expect("lan sync poisoned");
        if current.is_some() {
            return false;
        }
        *current = Some(network);
        true
    }

    fn network(&self) -> Option<LanSyncNetwork> {
        self.network.lock().expect("lan sync poisoned").clone()
    }

    /// 宿主启用了同步服务时返回 core，供设置变化后在后台重新应用。
    pub(super) fn bound_core(&self) -> Option<Arc<CoreInner>> {
        self.network()?;
        self.core.get()?.upgrade()
    }

    /// 按当前设置启动、停止或更新运行时；宿主还没启用同步服务时什么都不做。
    pub(super) async fn apply(&self, core: &Arc<CoreInner>) {
        let Some(network) = self.network() else {
            return;
        };
        let lan = core.settings.snapshot().sync.lan;
        let mut lifecycle = self.lifecycle.lock().await;

        if !lan.enabled {
            if let Some(runtime) = lifecycle.take() {
                self.stop_runtime(runtime).await;
            }
            self.set_error(None);
            return;
        }

        if let Some(runtime) = lifecycle.as_mut() {
            let name = effective_device_name(&lan);
            if runtime.advertised_name != name {
                if let Some(discovery) = runtime.discovery.as_mut() {
                    discovery.update_name(&name, current_platform());
                }
                runtime.advertised_name = name;
            }
            return;
        }

        match self.start_runtime(core, &network).await {
            Ok(runtime) => {
                *lifecycle = Some(runtime);
                self.set_error(None);
            }
            Err(err) => {
                log::error!("lan sync start failed: {err:#}");
                self.set_error(Some(format!("{err:#}")));
            }
        }
    }

    async fn start_runtime(
        &self,
        core: &Arc<CoreInner>,
        network: &LanSyncNetwork,
    ) -> anyhow::Result<Runtime> {
        let dir = core.paths.sync_dir();
        let loaded = identity::load_or_create(&dir)?;
        if loaded.regenerated {
            self.peers.forget_all();
        }

        let listener = bind_listener(network).await?;
        let port = listener.local_addr()?.port();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let shared = Arc::new(Shared {
            core: Arc::downgrade(core),
            app_version: core.info.version.to_string(),
            identity: loaded.identity,
            peers: self.peers.clone(),
            port,
            connections: Mutex::new(HashMap::new()),
            nearby: Mutex::new(HashMap::new()),
            pairing: Mutex::new(PairingCode::new()),
            echo: Mutex::new(EchoFilter::default()),
            dials: Mutex::new(HashMap::new()),
            dial_now: Notify::new(),
            handshakes: Arc::new(Semaphore::new(MAX_PENDING_HANDSHAKES)),
            shutdown: shutdown_rx,
            next_conn_id: AtomicU64::new(1),
        });

        tokio::spawn(accept_loop(shared.clone(), listener));
        tokio::spawn(dial_loop(shared.clone()));

        let lan = core.settings.snapshot().sync.lan;
        let name = effective_device_name(&lan);
        let weak = Arc::downgrade(&shared);
        // mDNS 起不来（如端口被系统服务独占）不影响手动输入地址配对和已知地址重连。
        let discovery = if network.discovery {
            match Discovery::start(
                shared.identity.device_id(),
                &name,
                current_platform(),
                port,
                move |event| {
                    if let Some(shared) = weak.upgrade() {
                        shared.on_discovery(event);
                    }
                },
            ) {
                Ok(discovery) => Some(discovery),
                Err(err) => {
                    log::warn!("lan sync discovery unavailable: {err:#}");
                    None
                }
            }
        } else {
            None
        };

        *self.shared.write().expect("lan sync poisoned") = Some(shared.clone());
        log::info!(
            "lan sync started as {} on port {port}",
            shared.identity.device_id()
        );

        Ok(Runtime {
            shared,
            discovery,
            shutdown: shutdown_tx,
            advertised_name: name,
        })
    }

    /// 关掉运行时（core 关闭时调用）；之后设置变化不再启动它。
    pub(super) async fn stop(&self) {
        *self.network.lock().expect("lan sync poisoned") = None;
        if let Some(runtime) = self.lifecycle.lock().await.take() {
            self.stop_runtime(runtime).await;
        }
    }

    async fn stop_runtime(&self, runtime: Runtime) {
        *self.shared.write().expect("lan sync poisoned") = None;
        let _ = runtime.shutdown.send(true);
        runtime.shared.kill_all();
        if let Some(discovery) = runtime.discovery {
            let _ = tokio::task::spawn_blocking(move || discovery.stop()).await;
        }
        log::info!("lan sync stopped");
    }

    fn set_error(&self, error: Option<String>) {
        *self.error.lock().expect("lan sync poisoned") = error;
    }

    pub(super) fn snapshot(&self, core: &CoreInner) -> LanSyncState {
        let lan = core.settings.snapshot().sync.lan;
        let shared = self.shared();
        let error = self.error.lock().expect("lan sync poisoned").clone();

        let devices = self
            .peers
            .list()
            .into_iter()
            .map(|peer| LanDeviceView {
                online: shared
                    .as_ref()
                    .is_some_and(|shared| shared.is_connected(&peer.device_id)),
                id: peer.device_id,
                name: peer.name,
                platform: peer.platform,
                address: peer.address,
                last_seen_at: peer.last_seen_at,
                paired_at: peer.paired_at,
            })
            .collect();

        let (pairing_code, pairing_attempts_left, nearby) = match &shared {
            Some(shared) => {
                let pairing = shared.pairing.lock().expect("lan sync poisoned");
                let code = pairing.code().map(str::to_owned);
                let attempts = pairing.attempts_left();
                drop(pairing);
                (code, attempts, shared.nearby_views())
            }
            None => (None, 0, Vec::new()),
        };

        LanSyncState {
            running: shared.is_some(),
            error,
            device_id: shared
                .as_ref()
                .map(|shared| shared.identity.device_id().to_owned()),
            device_name: effective_device_name(&lan),
            default_device_name: default_device_name(),
            platform: current_platform(),
            port: shared.as_ref().map(|shared| shared.port),
            addresses: self
                .network()
                .map(|network| network.advertised_addresses())
                .unwrap_or_default(),
            pairing_code,
            pairing_attempts_left,
            devices,
            nearby,
        }
    }

    pub(super) fn refresh_pairing_code(&self, core: &CoreInner) -> Result<()> {
        let shared = self.require_running(core)?;
        *shared.pairing.lock().expect("lan sync poisoned") = PairingCode::new();
        Ok(())
    }

    /// 用对方显示的配对码配对，返回对方设备名。
    pub(super) async fn pair(
        &self,
        core: &CoreInner,
        target: PairTarget,
        code: &str,
    ) -> Result<String> {
        let lang = core.language();
        let shared = self.require_running(core)?;
        let Some(code) = pairing::normalize_code(code) else {
            return Err(sync_error(core, Key::SyncInvalidCode));
        };

        let addresses = match target {
            PairTarget::Device(device_id) => shared
                .nearby_addresses(&device_id)
                .ok_or_else(|| sync_error(core, Key::SyncDeviceNotFound))?,
            PairTarget::Address(text) => resolve_address(&text)
                .await
                .ok_or_else(|| sync_error(core, Key::SyncInvalidAddress))?,
        };

        let goal = DialGoal::Pair { code };
        for addr in addresses {
            let key = match dial(&shared, addr, &goal).await {
                Ok(name) => {
                    shared.emit_state();
                    return Ok(name);
                }
                Err(DialError::Unreachable(err)) => {
                    log::info!("lan sync pairing could not reach {addr}: {err:#}");
                    continue;
                }
                Err(DialError::SelfConnection) => Key::SyncSelfPairing,
                Err(DialError::WrongCode) | Err(DialError::Rejected(RejectReason::WrongCode)) => {
                    Key::SyncWrongCode
                }
                Err(DialError::Rejected(RejectReason::PairingUnavailable)) => {
                    Key::SyncPairingUnavailable
                }
                Err(DialError::Rejected(RejectReason::Incompatible)) => Key::SyncIncompatible,
                Err(DialError::Rejected(RejectReason::NotPaired)) => continue,
                Err(DialError::Other(err)) => {
                    log::info!("lan sync pairing with {addr} failed: {err:#}");
                    continue;
                }
            };
            return Err(AppError::Sync(
                crate::i18n::commands::label(lang, key).to_owned(),
            ));
        }

        Err(sync_error(core, Key::SyncUnreachable))
    }

    /// 取消配对：先从本机移除（避免断开后又自动重连），在线时再通知对方。
    pub(super) async fn remove_device(&self, device_id: &str) {
        self.peers.remove(device_id);

        let Some(shared) = self.shared() else {
            return;
        };
        let Some((tx, kill)) = shared.connection_handles(device_id) else {
            return;
        };

        let unpair = (Message::Unpair, Arc::new(Vec::new()));
        let _ = tokio::time::timeout(Duration::from_secs(1), tx.send(unpair)).await;
        tokio::time::sleep(UNPAIR_FLUSH_DELAY).await;
        let _ = kill.send(true);
    }

    fn require_running(&self, core: &CoreInner) -> Result<Arc<Shared>> {
        self.shared()
            .ok_or_else(|| sync_error(core, Key::SyncNotRunning))
    }
}

fn sync_error(core: &CoreInner, key: Key) -> AppError {
    AppError::Sync(crate::i18n::commands::label(core.language(), key).to_owned())
}

struct Connection {
    id: u64,
    dialer_is_me: bool,
    tx: mpsc::Sender<Outgoing>,
    kill: Arc<watch::Sender<bool>>,
}

struct Nearby {
    name: String,
    platform: Platform,
    addresses: Vec<SocketAddr>,
}

#[derive(Default)]
struct DialState {
    dialing: bool,
    failures: u32,
    next_at: Option<Instant>,
}

pub(super) struct Shared {
    /// 弱引用：会话任务不该让已经关闭的 core 一直活着。
    core: Weak<CoreInner>,
    app_version: String,
    identity: DeviceIdentity,
    peers: Arc<PeerStore>,
    port: u16,
    connections: Mutex<HashMap<String, Connection>>,
    nearby: Mutex<HashMap<String, Nearby>>,
    pairing: Mutex<PairingCode>,
    echo: Mutex<EchoFilter>,
    dials: Mutex<HashMap<String, DialState>>,
    dial_now: Notify,
    handshakes: Arc<Semaphore>,
    shutdown: watch::Receiver<bool>,
    next_conn_id: AtomicU64,
}

impl Shared {
    pub(super) fn core(&self) -> Option<Arc<CoreInner>> {
        self.core.upgrade()
    }

    fn my_info(&self) -> DeviceInfo {
        let name = self
            .core()
            .map(|core| effective_device_name(&core.settings.snapshot().sync.lan))
            .unwrap_or_else(default_device_name);
        DeviceInfo {
            name,
            platform: current_platform(),
            app_version: self.app_version.clone(),
            protocol: PROTOCOL_VERSION,
            listen_port: self.port,
        }
    }

    fn emit(&self, event: CoreEvent) {
        if let Some(core) = self.core() {
            core.events.emit(event);
        }
    }

    fn emit_state(&self) {
        self.emit(CoreEvent::LanSyncChanged);
    }

    pub(super) fn has_connections(&self) -> bool {
        !self
            .connections
            .lock()
            .expect("lan sync poisoned")
            .is_empty()
    }

    fn is_connected(&self, device_id: &str) -> bool {
        self.connections
            .lock()
            .expect("lan sync poisoned")
            .contains_key(device_id)
    }

    pub(super) fn should_send(&self, content_hash: &str) -> bool {
        self.echo
            .lock()
            .expect("lan sync poisoned")
            .should_send(content_hash)
    }

    /// 推给所有在线设备；某台设备的发送队列满了就丢掉这一条（它重连后会补齐）。
    pub(super) fn broadcast(&self, message: &Message, attachment: Arc<Vec<u8>>) {
        let connections = self.connections.lock().expect("lan sync poisoned");
        for (peer_id, connection) in connections.iter() {
            if connection
                .tx
                .try_send((message.clone(), attachment.clone()))
                .is_err()
            {
                log::warn!("lan sync queue to {peer_id} is full, dropping an item");
            }
        }
    }

    /// 登记一条已认证的连接。同一台设备已有连接时，保留设备 id 较小一方发起的那条；
    /// 发起方相同（旧连接还没发现断开）时新的替换旧的。
    fn register(&self, peer_id: &str, connection: Connection) -> bool {
        let preferred_dialer_is_me = self.identity.device_id() < peer_id;
        let mut connections = self.connections.lock().expect("lan sync poisoned");
        if let Some(existing) = connections.get(peer_id) {
            let existing_preferred = existing.dialer_is_me == preferred_dialer_is_me;
            let new_preferred = connection.dialer_is_me == preferred_dialer_is_me;
            if existing_preferred && !new_preferred {
                return false;
            }
            let _ = existing.kill.send(true);
        }
        connections.insert(peer_id.to_owned(), connection);
        true
    }

    fn unregister(&self, peer_id: &str, connection_id: u64) {
        let mut connections = self.connections.lock().expect("lan sync poisoned");
        if connections
            .get(peer_id)
            .is_some_and(|connection| connection.id == connection_id)
        {
            connections.remove(peer_id);
        }
    }

    fn connection_handles(
        &self,
        peer_id: &str,
    ) -> Option<(mpsc::Sender<Outgoing>, Arc<watch::Sender<bool>>)> {
        self.connections
            .lock()
            .expect("lan sync poisoned")
            .get(peer_id)
            .map(|connection| (connection.tx.clone(), connection.kill.clone()))
    }

    fn kill_all(&self) {
        let connections = std::mem::take(&mut *self.connections.lock().expect("lan sync poisoned"));
        for connection in connections.into_values() {
            let _ = connection.kill.send(true);
        }
    }

    fn on_discovery(&self, event: DiscoveryEvent) {
        let changed = match event {
            DiscoveryEvent::Found(mut found) => {
                rank_addresses(&mut found.addresses, &local_ipv4s());
                let mut nearby = self.nearby.lock().expect("lan sync poisoned");
                let changed = nearby.get(&found.device_id).is_none_or(|existing| {
                    existing.name != found.name || existing.addresses != found.addresses
                });
                nearby.insert(
                    found.device_id.clone(),
                    Nearby {
                        name: found.name,
                        platform: found.platform,
                        addresses: found.addresses,
                    },
                );
                drop(nearby);

                if self.peers.get(&found.device_id).is_some()
                    && !self.is_connected(&found.device_id)
                {
                    self.reset_backoff(&found.device_id);
                    self.dial_now.notify_one();
                }
                changed
            }
            DiscoveryEvent::Lost(device_id) => self
                .nearby
                .lock()
                .expect("lan sync poisoned")
                .remove(&device_id)
                .is_some(),
        };

        if changed {
            self.emit_state();
        }
    }

    fn nearby_views(&self) -> Vec<LanNearbyView> {
        let nearby = self.nearby.lock().expect("lan sync poisoned");
        let mut views: Vec<LanNearbyView> = nearby
            .iter()
            .filter(|(id, _)| self.peers.get(id).is_none())
            .filter_map(|(id, device)| {
                Some(LanNearbyView {
                    id: id.clone(),
                    name: device.name.clone(),
                    platform: device.platform,
                    address: device.addresses.first()?.to_string(),
                })
            })
            .collect();
        views.sort_by(|a, b| a.name.cmp(&b.name));
        views
    }

    fn nearby_addresses(&self, device_id: &str) -> Option<Vec<SocketAddr>> {
        self.nearby
            .lock()
            .expect("lan sync poisoned")
            .get(device_id)
            .map(|device| device.addresses.clone())
    }

    /// 取出这一轮要拨的设备和候选地址，并标记为拨号中。
    fn dial_candidates(&self) -> Vec<(TrustedPeer, Vec<SocketAddr>)> {
        let peers = self.peers.list();
        let connections = self.connections.lock().expect("lan sync poisoned");
        let nearby = self.nearby.lock().expect("lan sync poisoned");
        let mut dials = self.dials.lock().expect("lan sync poisoned");
        let now = Instant::now();

        peers
            .into_iter()
            .filter_map(|peer| {
                if connections.contains_key(&peer.device_id) {
                    return None;
                }
                let state = dials.entry(peer.device_id.clone()).or_default();
                if state.dialing || state.next_at.is_some_and(|at| at > now) {
                    return None;
                }

                let mut addresses = nearby
                    .get(&peer.device_id)
                    .map(|device| device.addresses.clone())
                    .unwrap_or_default();
                if let Some(address) = peer
                    .address
                    .as_deref()
                    .and_then(|text| text.parse::<SocketAddr>().ok())
                {
                    if !addresses.contains(&address) {
                        addresses.push(address);
                    }
                }
                if addresses.is_empty() {
                    return None;
                }

                state.dialing = true;
                Some((peer, addresses))
            })
            .collect()
    }

    fn finish_dial(&self, device_id: &str, connected: bool) {
        let mut dials = self.dials.lock().expect("lan sync poisoned");
        let state = dials.entry(device_id.to_owned()).or_default();
        state.dialing = false;
        if connected {
            state.failures = 0;
            state.next_at = None;
            return;
        }

        state.failures = state.failures.saturating_add(1);
        let shift = state.failures.saturating_sub(1).min(5);
        let secs = (BACKOFF_BASE_SECS << shift).min(BACKOFF_MAX_SECS);
        state.next_at = Some(Instant::now() + Duration::from_secs(secs));
    }

    fn reset_backoff(&self, device_id: &str) {
        if let Some(state) = self
            .dials
            .lock()
            .expect("lan sync poisoned")
            .get_mut(device_id)
        {
            state.failures = 0;
            state.next_at = None;
        }
    }
}

async fn bind_listener(network: &LanSyncNetwork) -> anyhow::Result<TcpListener> {
    let ip = network.listen_ip;
    match TcpListener::bind((ip, network.port)).await {
        Ok(listener) => Ok(listener),
        Err(err) => {
            log::info!(
                "lan sync port {} unavailable ({err}), using a random port",
                network.port
            );
            Ok(TcpListener::bind((ip, 0)).await?)
        }
    }
}

async fn accept_loop(shared: Arc<Shared>, listener: TcpListener) {
    let mut shutdown = shared.shutdown.clone();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((stream, addr)) => {
                    let Ok(permit) = shared.handshakes.clone().try_acquire_owned() else {
                        log::warn!("lan sync drops {addr}: too many pending handshakes");
                        continue;
                    };
                    let shared = shared.clone();
                    tokio::spawn(async move {
                        if let Err(err) = handle_incoming(shared, stream, addr, permit).await {
                            log::info!("lan sync incoming connection from {addr} ended: {err:#}");
                        }
                    });
                }
                Err(err) => {
                    log::warn!("lan sync accept failed: {err}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            },
            _ = shutdown.changed() => break,
        }
    }
}

async fn handle_incoming(
    shared: Arc<Shared>,
    stream: TcpStream,
    addr: SocketAddr,
    permit: OwnedSemaphorePermit,
) -> anyhow::Result<()> {
    let Handshaken {
        mut reader,
        mut writer,
        remote_public_key,
        handshake_hash,
    } = handshake(stream, &shared.identity, false).await?;
    let remote_id = device_id_for(&remote_public_key);
    if remote_id == shared.identity.device_id() {
        return Err(anyhow!("connected to itself"));
    }

    let (message, _) = recv_within(&mut reader).await?;
    let Message::Hello { device, intent } = message else {
        return Err(anyhow!("expected hello"));
    };
    if device.protocol != PROTOCOL_VERSION {
        reject(&mut writer, RejectReason::Incompatible).await;
        return Err(anyhow!("incompatible protocol {}", device.protocol));
    }

    match intent {
        Intent::Sync => {
            if !shared
                .peers
                .is_trusted(&remote_id, &to_hex(&remote_public_key))
            {
                reject(&mut writer, RejectReason::NotPaired).await;
                return Err(anyhow!("unpaired device {remote_id} asked to sync"));
            }
            writer
                .send_message(
                    &Message::Welcome {
                        device: shared.my_info(),
                    },
                    &[],
                )
                .await?;
        }
        Intent::Pair { spake } => {
            let request = PairRequest {
                remote_id: &remote_id,
                remote_key: &remote_public_key,
                handshake_hash: &handshake_hash,
                device: &device,
                spake: &spake,
                addr,
            };
            accept_pairing(&shared, &mut reader, &mut writer, request).await?;
        }
    }

    drop(permit);
    run_session(shared, remote_id, device, reader, writer, false, addr).await;
    Ok(())
}

struct PairRequest<'a> {
    remote_id: &'a str,
    remote_key: &'a [u8; 32],
    handshake_hash: &'a [u8],
    device: &'a DeviceInfo,
    spake: &'a str,
    addr: SocketAddr,
}

/// 显示配对码的一方：校验对方的确认值，通过后记为已配对设备并换一个新配对码。
async fn accept_pairing(
    shared: &Arc<Shared>,
    reader: &mut SecureReader,
    writer: &mut SecureWriter,
    request: PairRequest<'_>,
) -> anyhow::Result<()> {
    let code = shared
        .pairing
        .lock()
        .expect("lan sync poisoned")
        .begin_attempt();
    shared.emit_state();
    let Some(code) = code else {
        reject(writer, RejectReason::PairingUnavailable).await;
        return Err(anyhow!("pairing attempts used up"));
    };

    let (spake, outbound) = pairing::start_listener(&code);
    let Ok(key) = spake.finish(&from_hex(request.spake)?) else {
        reject(writer, RejectReason::WrongCode).await;
        return Err(anyhow!("invalid spake message"));
    };
    writer
        .send_message(
            &Message::PairChallenge {
                device: shared.my_info(),
                spake: to_hex(&outbound),
            },
            &[],
        )
        .await?;

    let (message, _) = recv_within(reader).await?;
    let Message::PairConfirm { mac } = message else {
        return Err(anyhow!("expected pairing confirmation"));
    };
    let transcript = Transcript {
        handshake_hash: request.handshake_hash,
        dialer_key: request.remote_key,
        listener_key: shared.identity.public_key(),
    };
    let expected = pairing::confirmation(&key, Role::Dialer, &transcript);
    if !pairing::verify_confirmation(&expected, &mac) {
        reject(writer, RejectReason::WrongCode).await;
        return Err(anyhow!("wrong pairing code from {}", request.addr));
    }

    let ours = pairing::confirmation(&key, Role::Listener, &transcript);
    writer
        .send_message(
            &Message::PairDone {
                mac: to_hex(ours.as_bytes()),
            },
            &[],
        )
        .await?;

    let now = Utc::now();
    shared.peers.add(TrustedPeer {
        device_id: request.remote_id.to_owned(),
        public_key: to_hex(request.remote_key),
        name: request.device.name.clone(),
        platform: request.device.platform,
        address: Some(SocketAddr::new(request.addr.ip(), request.device.listen_port).to_string()),
        paired_at: now,
        last_seen_at: Some(now),
        cursor: None,
    });
    *shared.pairing.lock().expect("lan sync poisoned") = PairingCode::new();
    log::info!(
        "lan sync paired with {} ({})",
        request.device.name,
        request.remote_id
    );
    shared.emit(CoreEvent::LanDevicePaired {
        name: request.device.name.clone(),
    });
    Ok(())
}

enum DialGoal {
    Sync { device_id: String },
    Pair { code: String },
}

#[derive(Debug)]
enum DialError {
    /// 连不上或握手失败，可以换下一个地址再试。
    Unreachable(anyhow::Error),
    SelfConnection,
    WrongCode,
    Rejected(RejectReason),
    Other(anyhow::Error),
}

/// 主动连接 `addr`；成功时已在后台开始同步会话，返回对方设备名。
async fn dial(
    shared: &Arc<Shared>,
    addr: SocketAddr,
    goal: &DialGoal,
) -> std::result::Result<String, DialError> {
    let stream = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr))
        .await
        .map_err(|_| DialError::Unreachable(anyhow!("connect timed out")))?
        .map_err(|err| DialError::Unreachable(err.into()))?;
    let Handshaken {
        mut reader,
        mut writer,
        remote_public_key,
        handshake_hash,
    } = handshake(stream, &shared.identity, true)
        .await
        .map_err(DialError::Unreachable)?;
    let remote_id = device_id_for(&remote_public_key);
    if remote_id == shared.identity.device_id() {
        return Err(DialError::SelfConnection);
    }

    let device = match goal {
        DialGoal::Sync { device_id } => {
            if &remote_id != device_id
                || !shared
                    .peers
                    .is_trusted(&remote_id, &to_hex(&remote_public_key))
            {
                return Err(DialError::Other(anyhow!(
                    "{addr} now belongs to another device"
                )));
            }

            let hello = Message::Hello {
                device: shared.my_info(),
                intent: Intent::Sync,
            };
            writer
                .send_message(&hello, &[])
                .await
                .map_err(DialError::Unreachable)?;
            match recv_within(&mut reader)
                .await
                .map_err(DialError::Unreachable)?
                .0
            {
                Message::Welcome { device } => device,
                Message::Reject { reason } => return Err(DialError::Rejected(reason)),
                other => return Err(DialError::Other(anyhow!("unexpected reply {other:?}"))),
            }
        }
        DialGoal::Pair { code } => {
            let device = pair_as_dialer(
                shared,
                &mut reader,
                &mut writer,
                code,
                &remote_public_key,
                &handshake_hash,
            )
            .await?;
            let now = Utc::now();
            shared.peers.add(TrustedPeer {
                device_id: remote_id.clone(),
                public_key: to_hex(&remote_public_key),
                name: device.name.clone(),
                platform: device.platform,
                address: Some(SocketAddr::new(addr.ip(), device.listen_port).to_string()),
                paired_at: now,
                last_seen_at: Some(now),
                cursor: None,
            });
            log::info!("lan sync paired with {} ({remote_id})", device.name);
            device
        }
    };

    let name = device.name.clone();
    let shared = shared.clone();
    tokio::spawn(async move {
        run_session(shared, remote_id, device, reader, writer, true, addr).await;
    });
    Ok(name)
}

/// 输入配对码的一方：发 SPAKE2 首条消息，先交出自己的确认值，再校验对方的。
async fn pair_as_dialer(
    shared: &Arc<Shared>,
    reader: &mut SecureReader,
    writer: &mut SecureWriter,
    code: &str,
    remote_key: &[u8; 32],
    handshake_hash: &[u8],
) -> std::result::Result<DeviceInfo, DialError> {
    let (spake, outbound) = pairing::start_dialer(code);
    let hello = Message::Hello {
        device: shared.my_info(),
        intent: Intent::Pair {
            spake: to_hex(&outbound),
        },
    };
    writer
        .send_message(&hello, &[])
        .await
        .map_err(DialError::Unreachable)?;

    let (device, spake_hex) = match recv_within(reader).await.map_err(DialError::Unreachable)?.0 {
        Message::PairChallenge { device, spake } => (device, spake),
        Message::Reject { reason } => return Err(DialError::Rejected(reason)),
        other => return Err(DialError::Other(anyhow!("unexpected reply {other:?}"))),
    };
    let inbound = from_hex(&spake_hex).map_err(DialError::Other)?;
    let key = spake.finish(&inbound).map_err(|_| DialError::WrongCode)?;

    let transcript = Transcript {
        handshake_hash,
        dialer_key: shared.identity.public_key(),
        listener_key: remote_key,
    };
    let ours = pairing::confirmation(&key, Role::Dialer, &transcript);
    writer
        .send_message(
            &Message::PairConfirm {
                mac: to_hex(ours.as_bytes()),
            },
            &[],
        )
        .await
        .map_err(DialError::Unreachable)?;

    match recv_within(reader).await.map_err(DialError::Unreachable)?.0 {
        Message::PairDone { mac } => {
            let expected = pairing::confirmation(&key, Role::Listener, &transcript);
            if pairing::verify_confirmation(&expected, &mac) {
                Ok(device)
            } else {
                Err(DialError::WrongCode)
            }
        }
        Message::Reject { reason } => Err(DialError::Rejected(reason)),
        other => Err(DialError::Other(anyhow!("unexpected reply {other:?}"))),
    }
}

async fn dial_loop(shared: Arc<Shared>) {
    let mut shutdown = shared.shutdown.clone();
    loop {
        for (peer, addresses) in shared.dial_candidates() {
            let shared = shared.clone();
            tokio::spawn(async move {
                dial_peer(shared, peer, addresses).await;
            });
        }

        tokio::select! {
            _ = tokio::time::sleep(DIAL_INTERVAL) => {}
            _ = shared.dial_now.notified() => {}
            _ = shutdown.changed() => return,
        }
    }
}

async fn dial_peer(shared: Arc<Shared>, peer: TrustedPeer, addresses: Vec<SocketAddr>) {
    let goal = DialGoal::Sync {
        device_id: peer.device_id.clone(),
    };
    let mut connected = false;
    for addr in addresses {
        match dial(&shared, addr, &goal).await {
            Ok(_) => {
                connected = true;
                break;
            }
            Err(DialError::Rejected(RejectReason::NotPaired)) => {
                // 对方已经取消了和本机的配对（离线时发不了 Unpair），本机也跟着移除。
                log::info!("{} no longer trusts this device, removing it", peer.name);
                shared.peers.remove(&peer.device_id);
                shared.emit_state();
                break;
            }
            Err(err) => log::debug!("lan sync dial {} at {addr} failed: {err:?}", peer.name),
        }
    }
    shared.finish_dial(&peer.device_id, connected);
}

/// 已认证连接上的同步会话：登记连接、请求补齐，然后收发直到断开。
async fn run_session(
    shared: Arc<Shared>,
    peer_id: String,
    device: DeviceInfo,
    mut reader: SecureReader,
    writer: SecureWriter,
    dialer_is_me: bool,
    addr: SocketAddr,
) {
    let connection_id = shared.next_conn_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel::<Outgoing>(OUTGOING_QUEUE);
    let (kill_tx, kill_rx) = watch::channel(false);
    let kill_tx = Arc::new(kill_tx);
    let registered = shared.register(
        &peer_id,
        Connection {
            id: connection_id,
            dialer_is_me,
            tx: tx.clone(),
            kill: kill_tx.clone(),
        },
    );
    if !registered {
        log::debug!("lan sync keeps the existing connection to {peer_id}");
        return;
    }

    let address = SocketAddr::new(addr.ip(), device.listen_port).to_string();
    shared
        .peers
        .touch(&peer_id, &device.name, device.platform, Some(address));
    shared.reset_backoff(&peer_id);
    log::info!(
        "lan sync connected to {} ({peer_id}) via {addr}",
        device.name
    );
    shared.emit_state();

    let writer_task = tokio::spawn(write_loop(
        writer,
        rx,
        kill_tx.clone(),
        kill_rx.clone(),
        shared.shutdown.clone(),
    ));

    let since = shared
        .peers
        .get(&peer_id)
        .and_then(|peer| peer.cursor)
        .map(|cursor| cursor.to_rfc3339());
    let _ = tx
        .send((Message::CatchUp { since }, Arc::new(Vec::new())))
        .await;

    read_loop(
        &shared,
        &peer_id,
        device.platform,
        &mut reader,
        &tx,
        kill_rx,
    )
    .await;

    let _ = kill_tx.send(true);
    drop(tx);
    let _ = writer_task.await;
    shared.unregister(&peer_id, connection_id);
    log::info!("lan sync disconnected from {} ({peer_id})", device.name);
    shared.emit_state();
    shared.dial_now.notify_one();
}

async fn read_loop(
    shared: &Arc<Shared>,
    peer_id: &str,
    platform: Platform,
    reader: &mut SecureReader,
    tx: &mpsc::Sender<Outgoing>,
    mut kill: watch::Receiver<bool>,
) {
    let mut shutdown = shared.shutdown.clone();
    loop {
        let received = tokio::select! {
            received = tokio::time::timeout(IDLE_TIMEOUT, reader.recv_message()) => received,
            _ = kill.changed() => return,
            _ = shutdown.changed() => return,
        };
        let (message, attachment) = match received {
            Ok(Ok(message)) => message,
            Ok(Err(err)) => {
                log::info!("lan sync connection to {peer_id} closed: {err:#}");
                return;
            }
            Err(_) => {
                log::info!("lan sync connection to {peer_id} went quiet, closing");
                return;
            }
        };

        match message {
            Message::Ping => {}
            Message::Push { item, live } => {
                let Some(core) = shared.core() else {
                    return;
                };
                let stamp = engine::parse_stamp(&item.stamp);
                if let Err(err) = engine::receive(
                    &core,
                    &shared.echo,
                    peer_id,
                    platform,
                    item,
                    attachment,
                    live,
                )
                .await
                {
                    log::warn!("lan sync receive from {peer_id} failed: {err:#}");
                }
                if let Some(stamp) = stamp {
                    shared.peers.advance_cursor(peer_id, stamp);
                }
            }
            Message::CatchUp { since } => {
                let Some(core) = shared.core() else {
                    return;
                };
                let tx = tx.clone();
                tokio::spawn(async move {
                    send_catch_up(core, tx, since).await;
                });
            }
            Message::CatchUpDone { until } => {
                if let Some(stamp) = engine::parse_stamp(&until) {
                    shared.peers.advance_cursor(peer_id, stamp);
                }
            }
            Message::Unpair => {
                log::info!("lan sync peer {peer_id} removed this device");
                shared.peers.remove(peer_id);
                shared.emit_state();
                return;
            }
            other => log::debug!("lan sync ignores unexpected {other:?}"),
        }
    }
}

async fn send_catch_up(core: Arc<CoreInner>, tx: mpsc::Sender<Outgoing>, since: Option<String>) {
    let (items, until) = match engine::collect_catch_up(&core, since.as_deref()).await {
        Ok(result) => result,
        Err(err) => {
            log::warn!("lan sync catch-up failed: {err:#}");
            return;
        }
    };

    for (item, attachment) in items {
        let push = Message::Push { item, live: false };
        if tx.send((push, Arc::new(attachment))).await.is_err() {
            return;
        }
    }
    let _ = tx
        .send((Message::CatchUpDone { until }, Arc::new(Vec::new())))
        .await;
}

async fn write_loop(
    mut writer: SecureWriter,
    mut rx: mpsc::Receiver<Outgoing>,
    kill_tx: Arc<watch::Sender<bool>>,
    mut kill: watch::Receiver<bool>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut ping = tokio::time::interval(PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    ping.tick().await;

    loop {
        let result = tokio::select! {
            outgoing = rx.recv() => match outgoing {
                Some((message, attachment)) => writer.send_message(&message, &attachment).await,
                None => break,
            },
            _ = ping.tick() => writer.send_message(&Message::Ping, &[]).await,
            _ = kill.changed() => break,
            _ = shutdown.changed() => break,
        };
        if let Err(err) = result {
            log::info!("lan sync write failed: {err:#}");
            break;
        }
    }
    let _ = kill_tx.send(true);
}

async fn recv_within(reader: &mut SecureReader) -> anyhow::Result<(Message, Vec<u8>)> {
    tokio::time::timeout(HANDSHAKE_TIMEOUT, reader.recv_message())
        .await
        .map_err(|_| anyhow!("peer did not answer in time"))?
}

async fn reject(writer: &mut SecureWriter, reason: RejectReason) {
    if let Err(err) = writer.send_message(&Message::Reject { reason }, &[]).await {
        log::debug!("lan sync reject not delivered: {err:#}");
    }
}

/// 解析手动输入的地址：`ip`、`ip:端口`、`主机名[:端口]`，缺端口时用默认端口。
async fn resolve_address(text: &str) -> Option<Vec<SocketAddr>> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Ok(addr) = text.parse::<SocketAddr>() {
        return Some(vec![addr]);
    }
    if let Ok(ip) = text.parse::<IpAddr>() {
        return Some(vec![SocketAddr::new(ip, DEFAULT_PORT)]);
    }

    let with_port = if text.contains(':') {
        text.to_owned()
    } else {
        format!("{text}:{DEFAULT_PORT}")
    };
    let addresses: Vec<SocketAddr> = tokio::net::lookup_host(with_port)
        .await
        .ok()?
        .filter(SocketAddr::is_ipv4)
        .collect();
    (!addresses.is_empty()).then_some(addresses)
}

fn local_addresses() -> Vec<String> {
    local_ipv4s().into_iter().map(|ip| ip.to_string()).collect()
}

/// 本机可被别的设备连接的 IPv4 地址，默认路由所在网卡排第一。
/// Hyper-V、WSL、VMware 等虚拟网卡的地址别的电脑连不到，只在没有别的地址时才列出。
fn local_ipv4s() -> Vec<Ipv4Addr> {
    let Ok(interfaces) = if_addrs::get_if_addrs() else {
        return Vec::new();
    };

    let mut physical = Vec::new();
    let mut virtual_adapters = Vec::new();
    for interface in interfaces {
        let IpAddr::V4(ip) = interface.ip() else {
            continue;
        };
        if ip.is_loopback() || ip.is_link_local() {
            continue;
        }
        if is_virtual_adapter(&interface.name) {
            virtual_adapters.push(ip);
        } else {
            physical.push(ip);
        }
    }

    let mut addresses = if physical.is_empty() {
        virtual_adapters
    } else {
        physical
    };
    let primary = primary_ipv4();
    addresses.sort_by_key(|ip| (Some(*ip) != primary, *ip));
    addresses.dedup();
    addresses
}

fn is_virtual_adapter(name: &str) -> bool {
    const MARKERS: [&str; 7] = [
        "vethernet",
        "wsl",
        "hyper-v",
        "vmware",
        "vmnet",
        "virtualbox",
        "vboxnet",
    ];
    let name = name.to_lowercase();
    MARKERS.iter().any(|marker| name.contains(marker))
}

/// 默认路由出口的地址：对 UDP 套接字 connect 只选路由、不发包。
fn primary_ipv4() -> Option<Ipv4Addr> {
    let socket = std::net::UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(8, 8, 8, 8), 53)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(ip) if !ip.is_unspecified() => Some(ip),
        _ => None,
    }
}

/// 和本机同一网段（/24）的地址排前面：拨号先试最可能连通的，界面也显示这一个。
fn rank_addresses(addresses: &mut [SocketAddr], local: &[Ipv4Addr]) {
    addresses.sort_by_key(|addr| {
        let IpAddr::V4(ip) = addr.ip() else {
            return true;
        };
        !local
            .iter()
            .any(|local| local.octets()[..3] == ip.octets()[..3])
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resolves_manual_addresses() {
        assert_eq!(
            resolve_address("192.168.1.8").await,
            Some(vec!["192.168.1.8:41573".parse().unwrap()])
        );
        assert_eq!(
            resolve_address(" 192.168.1.8:5000 ").await,
            Some(vec!["192.168.1.8:5000".parse().unwrap()])
        );
        assert_eq!(resolve_address("").await, None);
    }

    #[test]
    fn same_subnet_addresses_come_first() {
        let mut addresses: Vec<SocketAddr> = vec![
            "172.30.16.1:41573".parse().unwrap(),
            "192.168.1.20:41573".parse().unwrap(),
            "10.0.0.5:41573".parse().unwrap(),
        ];

        rank_addresses(&mut addresses, &[Ipv4Addr::new(192, 168, 1, 8)]);

        assert_eq!(addresses[0], "192.168.1.20:41573".parse().unwrap());
        assert_eq!(addresses[1], "172.30.16.1:41573".parse().unwrap());
    }

    #[test]
    fn recognizes_virtual_adapters() {
        assert!(is_virtual_adapter("vEthernet (WSL (Hyper-V firewall))"));
        assert!(is_virtual_adapter("VMware Network Adapter VMnet8"));
        assert!(!is_virtual_adapter("以太网"));
        assert!(!is_virtual_adapter("en0"));
    }
}
