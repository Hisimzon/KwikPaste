//! 局域网同步的线上消息。
//!
//! 一条应用消息 = `u32` 头长度 + JSON 头 + 二进制附件（图片原始 PNG），整体再按
//! [`super::transport`] 分块加密。记录只描述内容本身，不带本机 id，后续自托管 / 官方服务器沿用同一套定义。

use anyhow::{anyhow, Context};
use serde::{Deserialize, Serialize};

use crate::db::models::{ClipboardKind, ClipboardSubKind, Platform};

/// mDNS / DNS-SD 服务类型。
pub const SERVICE_TYPE: &str = "_kwikpaste._tcp.local.";
/// 协议版本：握手时互报，不兼容直接拒绝。
pub const PROTOCOL_VERSION: u16 = 1;
/// Noise 握手模式：XX 双方交换静态公钥，身份由配对时固定的公钥确认。
pub const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
pub const NOISE_PROLOGUE: &[u8] = b"KwikPaste LAN sync v1";
/// 单条应用消息（含附件）的上限，防止对端用超大长度耗尽内存。
pub const MAX_MESSAGE_BYTES: usize = 128 * 1024 * 1024;

/// 连接双方在握手后互报的设备信息。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub name: String,
    pub platform: Platform,
    pub app_version: String,
    pub protocol: u16,
    /// 本机监听端口；对端据此记下「下次怎么连回来」。
    pub listen_port: u16,
}

/// 发起方的连接目的。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Intent {
    /// 已配对设备之间的同步会话。
    Sync,
    /// 用对方显示的配对码配对；`spake` 是 SPAKE2 首条消息（hex）。
    Pair { spake: String },
}

/// 拒绝原因，发起方据此给用户提示。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RejectReason {
    NotPaired,
    PairingUnavailable,
    WrongCode,
    Incompatible,
}

/// 同步的一条剪贴板记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireItem {
    pub kind: ClipboardKind,
    pub sub_kind: Option<ClipboardSubKind>,
    /// 文本的源表示（纯文本 / HTML / RTF）；图片为空，内容在附件里。
    pub content: String,
    pub search_text: Option<String>,
    pub summary: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    /// 发送方这条记录的 `updated_at`（发送方时钟），接收方只用来记补齐进度。
    pub stamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Message {
    Hello {
        device: DeviceInfo,
        intent: Intent,
    },
    Welcome {
        device: DeviceInfo,
    },
    PairChallenge {
        device: DeviceInfo,
        spake: String,
    },
    PairConfirm {
        mac: String,
    },
    PairDone {
        mac: String,
    },
    Reject {
        reason: RejectReason,
    },
    Ping,
    /// 一条记录；`live` 为真表示对方刚刚复制，接收方可写入系统剪贴板。
    Push {
        item: WireItem,
        live: bool,
    },
    /// 请求对方补发 `since`（对方时钟）之后本机采集的记录；`None` 表示刚配对，只要进度不要内容。
    CatchUp {
        since: Option<String>,
    },
    CatchUpDone {
        until: String,
    },
    /// 发送方已把接收方从已配对设备里移除。
    Unpair,
}

/// 序列化一条消息：`u32` 头长度 + JSON 头 + 附件。
pub fn encode(message: &Message, attachment: &[u8]) -> anyhow::Result<Vec<u8>> {
    let header = serde_json::to_vec(message).context("failed to encode sync message")?;
    let header_len = u32::try_from(header.len()).context("sync message header too large")?;

    let mut out = Vec::with_capacity(4 + header.len() + attachment.len());
    out.extend_from_slice(&header_len.to_be_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(attachment);
    Ok(out)
}

/// 解析 [`encode`] 的结果，返回消息与附件。
pub fn decode(mut bytes: Vec<u8>) -> anyhow::Result<(Message, Vec<u8>)> {
    if bytes.len() < 4 {
        return Err(anyhow!("sync message too short"));
    }

    let header_len = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    let header_end = 4usize
        .checked_add(header_len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| anyhow!("sync message header length out of range"))?;

    let message = serde_json::from_slice(&bytes[4..header_end]).context("invalid sync message")?;
    let attachment = bytes.split_off(header_end);
    Ok((message, attachment))
}

pub fn to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;

    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, b| {
            let _ = write!(out, "{b:02x}");
            out
        })
}

pub fn from_hex(text: &str) -> anyhow::Result<Vec<u8>> {
    if !text.is_ascii() || !text.len().is_multiple_of(2) {
        return Err(anyhow!("invalid hex string"));
    }

    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).map_err(|_| anyhow!("invalid hex string")))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_round_trips_with_attachment() {
        let message = Message::CatchUp {
            since: Some("2026-09-29T00:00:00+00:00".to_owned()),
        };
        let bytes = encode(&message, b"png-bytes").unwrap();
        let (decoded, attachment) = decode(bytes).unwrap();

        assert!(matches!(decoded, Message::CatchUp { since: Some(_) }));
        assert_eq!(attachment, b"png-bytes");
    }

    #[test]
    fn decode_rejects_header_longer_than_message() {
        let mut bytes = 100u32.to_be_bytes().to_vec();
        bytes.extend_from_slice(b"{}");

        assert!(decode(bytes).is_err());
    }

    #[test]
    fn hex_round_trips() {
        let bytes = [0u8, 1, 0xab, 0xff];

        assert_eq!(to_hex(&bytes), "0001abff");
        assert_eq!(from_hex("0001abff").unwrap(), bytes);
        assert!(from_hex("abc").is_err());
        assert!(from_hex("zz").is_err());
    }
}
