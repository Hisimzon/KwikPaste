//! 用户主动提交的反馈：同一份限额 gzip 正文供上传和离线保存使用。

use std::io::Write as _;
use std::time::Duration;

use flate2::{Compression, write::GzEncoder};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const ENDPOINT: &str = "https://paste.fastthree.com/api/v1/feedback";
const MIB: usize = 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Attachment {
    pub name: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct App {
    pub version: String,
    pub official: bool,
    pub platform: String,
    pub os_version: String,
    pub arch: String,
    pub language: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Payload {
    pub schema: u8,
    pub description: String,
    pub app: App,
    pub diagnostics: Value,
    pub settings: Value,
    pub logs: Vec<Attachment>,
}

#[derive(Debug, Deserialize)]
pub struct Receipt {
    pub id: String,
    pub code: String,
}

/// 反馈失败的原因；界面按变体取本地化文案，这里的英文只进日志。
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FeedbackError {
    #[error("description must be 1-5000 characters")]
    DescriptionLength,
    #[error("attachments are malformed or too large")]
    TooLarge,
    #[error("no feedback endpoint is configured")]
    NoEndpoint,
    #[error("feedback request timed out")]
    Timeout,
    #[error("feedback service is unreachable")]
    Offline,
    #[error("feedback was rejected")]
    Rejected,
    #[error("too many feedback reports today")]
    RateLimited,
    #[error("feedback service is unavailable")]
    Unavailable,
    #[error("unexpected feedback response")]
    BadResponse,
    #[error("diagnostics could not be collected")]
    Collect,
    #[error("feedback file could not be written")]
    Save,
}

pub type Result<T> = std::result::Result<T, FeedbackError>;

/// 描述去首尾空白后 1–5000 字符；弹窗和编码共用。
pub fn validate(description: &str) -> Result<()> {
    if !(1..=5000).contains(&description.trim().chars().count()) {
        return Err(FeedbackError::DescriptionLength);
    }
    Ok(())
}

/// 保留末尾最多指定字节的完整 UTF-8 文本，不切断多字节字符。
pub fn tail(text: &str, limit: usize) -> &str {
    let mut start = text.len().saturating_sub(limit);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

/// 校验协议并压缩；超限先丢旧日志，再逐步裁短当前日志，不修改预览草稿。
pub fn encode(payload: &Payload) -> Result<Vec<u8>> {
    let mut payload = payload.clone();
    payload.description = payload.description.trim().to_owned();
    validate(&payload.description)?;
    if payload.schema != 1
        || !payload.diagnostics.is_object()
        || serde_json::to_vec(&payload.diagnostics)
            .map_err(|_| FeedbackError::TooLarge)?
            .len()
            > 256 * 1024
        || !(payload.settings.is_object() || payload.settings.is_null())
        || payload.logs.len() > 4
        || payload.logs.iter().any(|log| log.name.chars().count() > 64)
    {
        return Err(FeedbackError::TooLarge);
    }
    for log in &mut payload.logs {
        log.text = tail(&log.text, MIB).to_owned();
    }
    loop {
        let json = serde_json::to_vec(&payload).map_err(|_| FeedbackError::TooLarge)?;
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(&json)
            .map_err(|_| FeedbackError::TooLarge)?;
        let body = encoder.finish().map_err(|_| FeedbackError::TooLarge)?;
        if body.len() <= MIB && json.len() <= 4 * MIB {
            return Ok(body);
        }
        if payload.logs.iter().any(|log| log.name == "KwikPaste.log.1") {
            payload.logs.retain(|log| log.name != "KwikPaste.log.1");
            continue;
        }
        let Some(current) = payload
            .logs
            .iter_mut()
            .find(|log| log.name == "KwikPaste.log" && !log.text.is_empty())
        else {
            return Err(FeedbackError::TooLarge);
        };
        current.text = tail(&current.text, current.text.len() / 2).to_owned();
    }
}

/// 仅 debug 接受显式开发端点；发布构建始终使用固定线上地址。
fn endpoint() -> Option<String> {
    if cfg!(debug_assertions) {
        return std::env::var("KWIKPASTE_FEEDBACK_ENDPOINT")
            .ok()
            .filter(|value| !value.trim().is_empty());
    }
    Some(ENDPOINT.to_owned())
}

/// 在 Tokio runtime 内调用；请求不重试、不跟随重定向，避免附件被转发到别的主机。
pub async fn upload(body: Vec<u8>) -> Result<Receipt> {
    let endpoint = endpoint().ok_or(FeedbackError::NoEndpoint)?;
    send(body, &endpoint).await
}

/// 服务器错误正文不直接展示或写日志，避免回显描述和附件。
async fn send(body: Vec<u8>, endpoint: &str) -> Result<Receipt> {
    if body.len() > MIB {
        return Err(FeedbackError::TooLarge);
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| FeedbackError::Offline)?;
    let response = client
        .post(endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::CONTENT_ENCODING, "gzip")
        .body(body)
        .send()
        .await
        .map_err(|err| {
            if err.is_timeout() {
                FeedbackError::Timeout
            } else {
                FeedbackError::Offline
            }
        })?;
    match response.status().as_u16() {
        201 => {
            let bytes = crate::http::read_limited(response, 4096)
                .await
                .map_err(|_| FeedbackError::BadResponse)?;
            let receipt: Receipt =
                serde_json::from_slice(&bytes).map_err(|_| FeedbackError::BadResponse)?;
            if receipt.id.is_empty() || !valid_code(&receipt.code) {
                return Err(FeedbackError::BadResponse);
            }
            Ok(receipt)
        }
        400 => Err(FeedbackError::Rejected),
        413 => Err(FeedbackError::TooLarge),
        429 => Err(FeedbackError::RateLimited),
        500..=599 => Err(FeedbackError::Unavailable),
        _ => Err(FeedbackError::BadResponse),
    }
}

fn valid_code(code: &str) -> bool {
    code.strip_prefix("FB-").is_some_and(|suffix| {
        suffix.len() == 6
            && suffix
                .bytes()
                .all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::testing::{Reply, Server};
    use serde_json::json;
    use std::io::Read as _;

    fn payload() -> Payload {
        Payload {
            schema: 1,
            description: " test ".into(),
            app: App {
                version: "2.0.5".into(),
                official: false,
                platform: "windows".into(),
                os_version: "10.0.26100".into(),
                arch: "x86_64".into(),
                language: "en-US".into(),
            },
            diagnostics: json!({}),
            settings: Value::Null,
            logs: vec![Attachment {
                name: "KwikPaste.log".into(),
                text: "hello".into(),
            }],
        }
    }

    fn decode(body: &[u8]) -> Value {
        let mut text = String::new();
        flate2::read::GzDecoder::new(body)
            .read_to_string(&mut text)
            .unwrap();
        serde_json::from_str(&text).unwrap()
    }

    #[test]
    fn feedback_transport_contract_and_errors() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for (status, expected) in [
            ("201 Created", None),
            ("400 Bad Request", Some(FeedbackError::Rejected)),
            ("413 Payload Too Large", Some(FeedbackError::TooLarge)),
            ("429 Too Many Requests", Some(FeedbackError::RateLimited)),
            ("503 Service Unavailable", Some(FeedbackError::Unavailable)),
            ("302 Found", Some(FeedbackError::BadResponse)),
        ] {
            let server = Server::start(vec![Reply {
                status,
                headers: vec![("Location".into(), "http://127.0.0.1:1/no".into())],
                body: br#"{"id":"test-id","code":"FB-7K3Q9P"}"#.to_vec(),
            }]);
            let result = runtime.block_on(send(
                encode(&payload()).unwrap(),
                &server.url("/api/v1/feedback"),
            ));
            assert_eq!(result.err(), expected);
            let requests = server.requests();
            assert_eq!(requests.len(), 1);
            assert!(
                requests[0]
                    .line
                    .starts_with("POST /api/v1/feedback HTTP/1.1")
            );
            let headers = requests[0].headers.to_ascii_lowercase();
            assert!(headers.contains("content-type: application/json"));
            assert!(headers.contains("content-encoding: gzip"));
            let body = decode(&requests[0].body);
            assert_eq!(body.as_object().unwrap().len(), 6);
            assert_eq!(body["schema"], 1);
            assert_eq!(body["description"], "test");
            assert_eq!(body["app"]["osVersion"], "10.0.26100");
            assert_eq!(body["logs"][0]["text"], "hello");
        }
        assert_eq!(
            runtime.block_on(send(vec![], "http://127.0.0.1:1")).err(),
            Some(FeedbackError::Offline)
        );
    }

    #[test]
    fn feedback_validates_lengths_and_utf8_tail() {
        assert_eq!(tail("a你好", 4), "好");
        let mut p = payload();
        p.logs[0].text = "你".repeat(MIB);
        assert!(
            decode(&encode(&p).unwrap())["logs"][0]["text"]
                .as_str()
                .unwrap()
                .len()
                <= MIB
        );
        p.description = " ".into();
        assert_eq!(encode(&p).err(), Some(FeedbackError::DescriptionLength));
        p.description = "a".repeat(5001);
        assert_eq!(encode(&p).err(), Some(FeedbackError::DescriptionLength));
        p.description = "ok".into();
        p.diagnostics = json!({"large":"x".repeat(256 * 1024)});
        assert_eq!(encode(&p).err(), Some(FeedbackError::TooLarge));
        p.diagnostics = json!({});
        p.settings = json!([]);
        assert_eq!(encode(&p).err(), Some(FeedbackError::TooLarge));
        assert!(!valid_code("FB-12345\n"));
    }

    #[test]
    fn feedback_drops_previous_before_trimming_current() {
        // 可复现的低压缩率 ASCII，不依赖系统随机源。
        let mut state = 123456789u64;
        let noise: String = (0..MIB)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                char::from(32 + (state % 95) as u8)
            })
            .collect();
        let mut p = payload();
        p.logs[0].text = noise.clone();
        p.logs.push(Attachment {
            name: "KwikPaste.log.1".into(),
            text: noise.clone(),
        });
        let body = encode(&p).unwrap();
        assert!(body.len() <= MIB);
        let json = decode(&body);
        assert_eq!(json["logs"].as_array().unwrap().len(), 1);
        assert_eq!(json["logs"][0]["text"], noise);
        p.settings = json!({"noise": noise});
        let body = encode(&p).unwrap();
        assert!(decode(&body)["logs"][0]["text"].as_str().unwrap().len() < MIB);
    }
}
