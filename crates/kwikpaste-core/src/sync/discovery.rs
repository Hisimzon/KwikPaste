//! mDNS / DNS-SD 广播与发现。
//!
//! 实例名就是设备 id；TXT 里放名称、平台和协议版本，不放任何密钥或内容。
//! 只走 IPv4：局域网里够用，也省掉 IPv6 链路本地地址的 scope 问题。

use std::net::{IpAddr, SocketAddr};

use anyhow::{anyhow, Context};
use mdns_sd::{IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};

use super::protocol::{PROTOCOL_VERSION, SERVICE_TYPE};
use crate::db::models::Platform;

const TXT_ID: &str = "id";
const TXT_NAME: &str = "name";
const TXT_PLATFORM: &str = "pf";
const TXT_VERSION: &str = "v";

/// 发现到的一台设备。
#[derive(Debug, Clone)]
pub struct Discovered {
    pub device_id: String,
    pub name: String,
    pub platform: Platform,
    pub addresses: Vec<SocketAddr>,
}

pub enum DiscoveryEvent {
    Found(Discovered),
    Lost(String),
}

pub struct Discovery {
    daemon: ServiceDaemon,
    fullname: String,
    device_id: String,
    port: u16,
}

impl Discovery {
    /// 启动广播和浏览；`on_event` 在 mDNS 后台线程上回调。
    pub fn start(
        device_id: &str,
        name: &str,
        platform: Platform,
        port: u16,
        on_event: impl Fn(DiscoveryEvent) + Send + 'static,
    ) -> anyhow::Result<Self> {
        let daemon = ServiceDaemon::new().map_err(mdns_err)?;
        daemon
            .disable_interface(vec![IfKind::IPv6, IfKind::LoopbackV4, IfKind::LoopbackV6])
            .map_err(mdns_err)?;

        let discovery = Self {
            daemon,
            fullname: String::new(),
            device_id: device_id.to_owned(),
            port,
        };
        let fullname = discovery.register(name, platform)?;
        let receiver = discovery.daemon.browse(SERVICE_TYPE).map_err(mdns_err)?;

        let own_id = device_id.to_owned();
        std::thread::Builder::new()
            .name("lan-sync-discovery".to_owned())
            .spawn(move || {
                // daemon 关闭后通道断开，线程随之结束。
                while let Ok(event) = receiver.recv() {
                    match event {
                        ServiceEvent::ServiceResolved(service) => {
                            let Some(found) = parse_service(
                                service.get_property_val_str(TXT_ID),
                                service.get_property_val_str(TXT_NAME),
                                service.get_property_val_str(TXT_PLATFORM),
                                service.get_property_val_str(TXT_VERSION),
                                service.get_addresses_v4().into_iter().map(IpAddr::V4),
                                service.get_port(),
                            ) else {
                                continue;
                            };
                            if found.device_id != own_id {
                                on_event(DiscoveryEvent::Found(found));
                            }
                        }
                        ServiceEvent::ServiceRemoved(_, fullname) => {
                            let Some(id) = instance_name(&fullname) else {
                                continue;
                            };
                            if id != own_id {
                                on_event(DiscoveryEvent::Lost(id));
                            }
                        }
                        _ => {}
                    }
                }
                log::debug!("lan sync discovery thread stopped");
            })
            .context("failed to spawn discovery thread")?;

        Ok(Self {
            fullname,
            ..discovery
        })
    }

    /// 改名后重新广播 TXT。
    pub fn update_name(&mut self, name: &str, platform: Platform) {
        if let Err(err) = self.daemon.unregister(&self.fullname) {
            log::warn!("mdns unregister before rename failed: {err}");
        }
        match self.register(name, platform) {
            Ok(fullname) => self.fullname = fullname,
            Err(err) => log::warn!("mdns re-register after rename failed: {err:#}"),
        }
    }

    /// 撤销广播并关闭 daemon；会短暂阻塞等待注销包发出。
    pub fn stop(self) {
        if let Ok(status) = self.daemon.unregister(&self.fullname) {
            let _ = status.recv_timeout(std::time::Duration::from_secs(1));
        }
        if let Ok(status) = self.daemon.shutdown() {
            let _ = status.recv_timeout(std::time::Duration::from_secs(1));
        }
    }

    fn register(&self, name: &str, platform: Platform) -> anyhow::Result<String> {
        let version = PROTOCOL_VERSION.to_string();
        let properties = [
            (TXT_ID, self.device_id.as_str()),
            (TXT_NAME, name),
            (TXT_PLATFORM, platform_tag(platform)),
            (TXT_VERSION, version.as_str()),
        ];
        let host_name = format!("kwikpaste-{}.local.", self.device_id);
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &self.device_id,
            &host_name,
            "",
            self.port,
            &properties[..],
        )
        .map_err(mdns_err)?
        .enable_addr_auto();
        let fullname = info.get_fullname().to_owned();
        self.daemon.register(info).map_err(mdns_err)?;
        Ok(fullname)
    }
}

fn parse_service(
    id: Option<&str>,
    name: Option<&str>,
    platform: Option<&str>,
    version: Option<&str>,
    ips: impl Iterator<Item = IpAddr>,
    port: u16,
) -> Option<Discovered> {
    let device_id = id?.trim();
    if device_id.is_empty() || version? != PROTOCOL_VERSION.to_string() {
        return None;
    }

    let platform = match platform? {
        "macos" => Platform::Macos,
        "windows" => Platform::Windows,
        _ => return None,
    };
    let addresses: Vec<SocketAddr> = ips
        .filter(|ip| !ip.is_loopback() && !ip.is_unspecified())
        .map(|ip| SocketAddr::new(ip, port))
        .collect();
    if addresses.is_empty() {
        return None;
    }

    Some(Discovered {
        device_id: device_id.to_owned(),
        name: name.unwrap_or(device_id).to_owned(),
        platform,
        addresses,
    })
}

fn instance_name(fullname: &str) -> Option<String> {
    let suffix = format!(".{SERVICE_TYPE}");
    fullname.strip_suffix(&suffix).map(str::to_owned)
}

pub fn platform_tag(platform: Platform) -> &'static str {
    match platform {
        Platform::Macos => "macos",
        Platform::Windows => "windows",
    }
}

fn mdns_err(err: mdns_sd::Error) -> anyhow::Error {
    anyhow!("mdns: {err}")
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;

    #[test]
    fn parses_valid_service_and_drops_loopback() {
        let ips = vec![
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V4(Ipv4Addr::new(192, 168, 1, 8)),
        ];

        let found = parse_service(
            Some("abc"),
            Some("MacBook"),
            Some("macos"),
            Some("1"),
            ips.into_iter(),
            41573,
        )
        .unwrap();

        assert_eq!(found.device_id, "abc");
        assert_eq!(found.platform, Platform::Macos);
        assert_eq!(found.addresses, vec!["192.168.1.8:41573".parse().unwrap()]);
    }

    #[test]
    fn rejects_other_protocol_versions() {
        let ips = vec![IpAddr::V4(Ipv4Addr::new(192, 168, 1, 8))];

        assert!(parse_service(
            Some("abc"),
            None,
            Some("windows"),
            Some("2"),
            ips.into_iter(),
            1
        )
        .is_none());
    }

    #[test]
    fn instance_name_strips_service_type() {
        assert_eq!(
            instance_name("abc._kwikpaste._tcp.local.").as_deref(),
            Some("abc")
        );
        assert_eq!(instance_name("abc._other._tcp.local."), None);
    }
}
