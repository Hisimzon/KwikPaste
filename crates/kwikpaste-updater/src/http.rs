//! HTTP 客户端。TLS 用系统实现（Windows SChannel、macOS Security.framework），代理跟随系统设置。

use std::time::Duration;

use anyhow::Context;

/// 被墙的端点不设超时要等系统 TCP 超时（Windows 约 21 秒）才放弃。
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// 检查清单、上报统计、拉公告的单次请求总时长。
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// 下载安装包时两次收到数据之间最多等这么久；下载总时长不设上限。
const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// 检查清单和下载安装包用：跟随重定向（GitHub 的下载地址会跳到对象存储）。
pub(crate) fn updater_client(version: &semver::Version) -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(format!("KwikPaste/{version} updater"))
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .build()
        .context("failed to build the update client")
}

/// 统计与公告用：不跟随重定向，上报的字段不会被转发到别的主机。
pub(crate) fn no_redirect_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("failed to build the http client")
}

/// 读完响应正文，超过 `limit` 字节就放弃。
pub(crate) async fn read_limited(
    mut response: reqwest::Response,
    limit: usize,
) -> anyhow::Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        anyhow::bail!("response is larger than {limit} bytes");
    }

    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.context("failed to read response")? {
        if body.len() + chunk.len() > limit {
            anyhow::bail!("response is larger than {limit} bytes");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// 测试用的本地 HTTP 服务：只监听 127.0.0.1，按顺序应答固定的响应。
#[cfg(test)]
pub(crate) mod testing {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;

    /// 一个固定响应。
    #[derive(Clone)]
    pub(crate) struct Reply {
        pub status: &'static str,
        pub headers: Vec<(String, String)>,
        pub body: Vec<u8>,
    }

    impl Reply {
        pub(crate) fn ok(body: impl Into<Vec<u8>>) -> Self {
            Self {
                status: "200 OK",
                headers: Vec::new(),
                body: body.into(),
            }
        }

        pub(crate) fn status(status: &'static str) -> Self {
            Self {
                status,
                headers: Vec::new(),
                body: Vec::new(),
            }
        }

        pub(crate) fn header(mut self, name: &str, value: &str) -> Self {
            self.headers.push((name.to_owned(), value.to_owned()));
            self
        }
    }

    /// 收到的一个请求：请求行与正文。
    #[derive(Debug, Clone)]
    pub(crate) struct Request {
        pub line: String,
        pub body: Vec<u8>,
    }

    pub(crate) struct Server {
        pub base: String,
        pub requests: Arc<Mutex<Vec<Request>>>,
    }

    impl Server {
        /// 依次应答 `replies`，应答完就停止监听。
        pub(crate) fn start(replies: Vec<Reply>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}", listener.local_addr().unwrap());
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen = requests.clone();
            thread::spawn(move || {
                for reply in replies {
                    let Ok((mut stream, _)) = listener.accept() else {
                        return;
                    };
                    let request = read_request(&mut stream);
                    seen.lock().unwrap().push(request);
                    let mut head = format!(
                        "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n",
                        reply.status,
                        reply.body.len()
                    );
                    for (name, value) in &reply.headers {
                        head.push_str(&format!("{name}: {value}\r\n"));
                    }
                    head.push_str("\r\n");
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&reply.body);
                }
            });
            Self { base, requests }
        }

        pub(crate) fn url(&self, path: &str) -> String {
            format!("{}{path}", self.base)
        }

        pub(crate) fn requests(&self) -> Vec<Request> {
            self.requests.lock().unwrap().clone()
        }
    }

    fn read_request(stream: &mut std::net::TcpStream) -> Request {
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 4096];
        while let Ok(count) = stream.read(&mut buffer) {
            if count == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..count]);
            let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let head = String::from_utf8_lossy(&bytes[..end]).into_owned();
            let length = head
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .and_then(|value| value.trim().parse::<usize>().ok())
                })
                .unwrap_or(0);
            if bytes.len() < end + 4 + length {
                continue;
            }
            return Request {
                line: head.lines().next().unwrap_or_default().to_owned(),
                body: bytes[end + 4..end + 4 + length].to_vec(),
            };
        }
        Request {
            line: String::new(),
            body: Vec::new(),
        }
    }
}
