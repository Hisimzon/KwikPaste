//! 读哪些清单、怎么从各渠道里挑出要装的版本。
//!
//! 2.x 只读 v2 地址：每个渠道先读七牛 CDN（国内可达）的 `kwikpaste/v2/<渠道>/latest.json`，失败再读
//! GitHub 的 `channel-v2-<渠道>` 发布里的 `latest-v2.json`。GitHub 上 2.x 的资产永远不叫 `latest.json`：
//! 老客户端的备用地址是 `releases/latest/download/latest.json`，万一哪个 2.x 发布被标成 latest 也不会被读到。
//! **绝不读 1.x 的旧地址**（`kwikpaste/{stable,beta,nightly}/latest.json` 与 GitHub 的
//! `latest`、`channel-beta`、`channel-nightly`），那里只有 1.x。
//!
//! 渠道沿用设置里已有的键：总是检查稳定版，`update.includeBeta` 为真时再检查测试版。v2 没有
//! nightly 渠道，`update.includeNightly` 不再读取。各渠道并发检查，先各自滤掉跳过的版本、系统不满足
//! 要求的版本和没有本机安装包的版本，再取版本号最大的；只要有一个渠道正常响应就不算检查失败。

use anyhow::{Context, anyhow};
use url::Url;

use crate::manifest::{self, PlatformRelease, Release};
use crate::os::OsVersion;

const STABLE_ENDPOINTS: &[&str] = &[
    "https://dl.fastthree.com/kwikpaste/v2/stable/latest.json",
    "https://github.com/ManSanDADADA/KwikPaste/releases/download/channel-v2-stable/latest-v2.json",
];
const BETA_ENDPOINTS: &[&str] = &[
    "https://dl.fastthree.com/kwikpaste/v2/beta/latest.json",
    "https://github.com/ManSanDADADA/KwikPaste/releases/download/channel-v2-beta/latest-v2.json",
];
/// 清单正文上限，防止把一个误配置成大文件的地址整个读进内存。
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Beta,
}

impl Channel {
    pub fn name(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Beta => "beta",
        }
    }
}

/// 按设置要检查的渠道。
pub(crate) fn channels(include_beta: bool) -> Vec<Channel> {
    let mut channels = vec![Channel::Stable];
    if include_beta {
        channels.push(Channel::Beta);
    }
    channels
}

/// 一个渠道按优先级排列的镜像地址；`e2e-overrides` 下可以用环境变量换成单个测试地址。
pub(crate) fn endpoints(channel: Channel) -> anyhow::Result<Vec<Url>> {
    let (env, defaults) = match channel {
        Channel::Stable => (crate::overrides::STABLE_ENDPOINT, STABLE_ENDPOINTS),
        Channel::Beta => (crate::overrides::BETA_ENDPOINT, BETA_ENDPOINTS),
    };
    if let Some(endpoint) = crate::overrides::var(env) {
        return Ok(vec![
            Url::parse(&endpoint).context("invalid update endpoint override")?,
        ]);
    }

    defaults
        .iter()
        .map(|endpoint| Url::parse(endpoint).context("invalid update endpoint"))
        .collect()
}

/// 挑选条件：当前版本、跳过的版本、本机系统与平台键。
#[derive(Debug, Clone)]
pub(crate) struct Criteria {
    pub current: semver::Version,
    pub skipped: Option<String>,
    pub os: Option<OsVersion>,
    pub platform_keys: Vec<String>,
}

/// 选中的更新。
#[derive(Debug, Clone)]
pub struct Candidate {
    pub channel: Channel,
    pub version: semver::Version,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
    pub target: String,
    pub platform: PlatformRelease,
}

/// 一个渠道的检查结果：`Ok(None)` 表示正常响应但没有可装的更新。
pub(crate) async fn check_channel(
    client: &reqwest::Client,
    channel: Channel,
    mirrors: &[Url],
    criteria: &Criteria,
) -> anyhow::Result<Option<Candidate>> {
    let mut last_error = None;
    for url in mirrors {
        match fetch(client, url).await {
            Ok(Fetched::NoContent) => return Ok(None),
            Ok(Fetched::Release(release)) => return Ok(select(channel, *release, criteria)),
            Err(err) => {
                log::warn!("update manifest {url} unusable: {err:#}");
                last_error = Some(err);
            }
        }
    }

    Err(last_error
        .unwrap_or_else(|| anyhow!("no update endpoint"))
        .context(format!("{} channel unavailable", channel.name())))
}

/// 并发检查各渠道，返回版本号最大的候选。所有渠道都失败才报错。
pub(crate) async fn check_all(
    client: &reqwest::Client,
    channels: Vec<(Channel, Vec<Url>)>,
    criteria: &Criteria,
) -> anyhow::Result<Option<Candidate>> {
    let checks: Vec<_> = channels
        .into_iter()
        .map(|(channel, mirrors)| {
            let client = client.clone();
            let criteria = criteria.clone();
            tokio::spawn(async move { check_channel(&client, channel, &mirrors, &criteria).await })
        })
        .collect();

    let mut newest: Option<Candidate> = None;
    let mut any_responded = false;
    let mut last_error = None;
    for check in checks {
        match check
            .await
            .map_err(anyhow::Error::new)
            .and_then(|found| found)
        {
            Ok(found) => {
                any_responded = true;
                if let Some(candidate) = found {
                    let newer = newest
                        .as_ref()
                        .is_none_or(|current| candidate.version > current.version);
                    if newer {
                        newest = Some(candidate);
                    }
                }
            }
            Err(err) => {
                log::warn!("update check failed: {err:#}");
                last_error = Some(err);
            }
        }
    }

    match (any_responded, last_error) {
        (false, Some(err)) => Err(err.context("failed to check for updates")),
        _ => Ok(newest),
    }
}

enum Fetched {
    NoContent,
    Release(Box<Release>),
}

async fn fetch(client: &reqwest::Client, url: &Url) -> anyhow::Result<Fetched> {
    let response = client
        .get(url.clone())
        .timeout(crate::http::REQUEST_TIMEOUT)
        .send()
        .await
        .context("request failed")?;
    let status = response.status();
    if status == reqwest::StatusCode::NO_CONTENT {
        return Ok(Fetched::NoContent);
    }
    if !status.is_success() {
        anyhow::bail!("endpoint answered {status}");
    }

    let body = crate::http::read_limited(response, MAX_MANIFEST_BYTES).await?;
    Ok(Fetched::Release(Box::new(manifest::parse(&body)?)))
}

/// 这个渠道的版本能不能装：比当前新、没被跳过、系统满足要求、有本机的安装包。
fn select(channel: Channel, release: Release, criteria: &Criteria) -> Option<Candidate> {
    if release.version <= criteria.current {
        return None;
    }
    let version = release.version.to_string();
    if criteria
        .skipped
        .as_deref()
        .is_some_and(|skipped| skipped.trim_start_matches('v') == version)
    {
        log::info!(
            "update {version} on {} was skipped by the user",
            channel.name()
        );
        return None;
    }
    if !release.requires.satisfied_by(criteria.os) {
        log::info!(
            "update {version} on {} needs a newer system: {:?}",
            channel.name(),
            release.requires
        );
        return None;
    }
    let Some((target, platform)) = release.platform(&criteria.platform_keys) else {
        log::warn!(
            "update {version} on {} has no package for {:?}",
            channel.name(),
            criteria.platform_keys
        );
        return None;
    };

    Some(Candidate {
        channel,
        target,
        platform: platform.clone(),
        notes: release.notes.clone(),
        pub_date: release.pub_date.and_then(|date| {
            date.format(&time::format_description::well_known::Rfc3339)
                .ok()
        }),
        version: release.version,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::http::testing::{Reply, Server};

    fn criteria() -> Criteria {
        Criteria {
            current: semver::Version::new(2, 0, 0),
            skipped: None,
            os: Some(OsVersion::WindowsBuild(26100)),
            platform_keys: vec![
                "windows-x86_64-nsis".to_owned(),
                "windows-x86_64".to_owned(),
            ],
        }
    }

    fn manifest(version: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "version": version,
            "pub_date": "2027-04-05T08:00:00Z",
            "platforms": {
                "windows-x86_64": {"url": format!("https://dl.example.com/{version}.exe"), "signature": "c2ln"},
                "darwin-aarch64": {"url": format!("https://dl.example.com/{version}.tar.gz"), "signature": "c2ln"},
            }
        }))
        .unwrap()
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn client() -> reqwest::Client {
        crate::http::updater_client(&semver::Version::new(2, 0, 0)).unwrap()
    }

    fn url(server: &Server, path: &str) -> Url {
        Url::parse(&server.url(path)).unwrap()
    }

    /// 生产地址只有 v2，全部 https，绝不出现 1.x 的旧地址。
    #[test]
    fn production_endpoints_are_v2_only() {
        if cfg!(feature = "e2e-overrides") {
            return;
        }
        let all: Vec<Url> = [Channel::Stable, Channel::Beta]
            .into_iter()
            .flat_map(|channel| endpoints(channel).unwrap())
            .collect();

        assert_eq!(all.len(), 4);
        for url in &all {
            assert_eq!(url.scheme(), "https", "{url}");
            let path = url.path();
            assert!(
                path.contains("/v2/") || path.contains("/channel-v2-"),
                "{url}"
            );
            if url.host_str() == Some("github.com") {
                assert!(path.ends_with("/latest-v2.json"), "{url}");
            }
        }
        for legacy in [
            "https://dl.fastthree.com/kwikpaste/stable/latest.json",
            "https://dl.fastthree.com/kwikpaste/beta/latest.json",
            "https://dl.fastthree.com/kwikpaste/nightly/latest.json",
            "https://github.com/ManSanDADADA/KwikPaste/releases/latest/download/latest.json",
            "https://github.com/ManSanDADADA/KwikPaste/releases/download/channel-beta/latest.json",
            "https://github.com/ManSanDADADA/KwikPaste/releases/download/channel-nightly/latest.json",
        ] {
            assert!(!all.iter().any(|url| url.as_str() == legacy), "{legacy}");
        }
    }

    #[test]
    fn channels_follow_the_existing_beta_setting() {
        assert_eq!(channels(false), [Channel::Stable]);
        assert_eq!(channels(true), [Channel::Stable, Channel::Beta]);
    }

    #[test]
    fn falls_back_to_the_next_mirror_when_the_first_is_unusable() {
        let rt = runtime();
        for first in [
            Reply::status("404 Not Found"),
            Reply::ok(b"{\"version\": \"2.1.0\", \"pub_date\": \"yesterday\", \"url\": \"https://a.b/c\", \"signature\": \"s\"}".to_vec()),
        ] {
            let server = Server::start(vec![first, Reply::ok(manifest("2.0.1"))]);
            let mirrors = [url(&server, "/cdn/latest.json"), url(&server, "/gh/latest.json")];

            let found = rt
                .block_on(check_channel(&client(), Channel::Stable, &mirrors, &criteria()))
                .unwrap()
                .unwrap();

            assert_eq!(found.version, semver::Version::new(2, 0, 1));
            assert_eq!(found.target, "windows-x86_64");
            assert_eq!(found.platform.url.as_str(), "https://dl.example.com/2.0.1.exe");
            assert_eq!(found.pub_date.as_deref(), Some("2027-04-05T08:00:00Z"));
            assert_eq!(server.requests().len(), 2);
        }
    }

    #[test]
    fn no_content_counts_as_a_response_without_update() {
        let rt = runtime();
        let server = Server::start(vec![Reply::status("204 No Content")]);
        let mirrors = [
            url(&server, "/cdn/latest.json"),
            url(&server, "/gh/latest.json"),
        ];

        let found = rt
            .block_on(check_channel(
                &client(),
                Channel::Stable,
                &mirrors,
                &criteria(),
            ))
            .unwrap();

        assert!(found.is_none());
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn candidates_are_filtered_before_picking_the_newest() {
        let rt = runtime();
        let stable = Server::start(vec![Reply::ok(manifest("2.0.1"))]);
        let beta = Server::start(vec![Reply::ok(manifest("2.1.0-beta.1"))]);
        let channels = vec![
            (Channel::Stable, vec![url(&stable, "/latest.json")]),
            (Channel::Beta, vec![url(&beta, "/latest.json")]),
        ];
        let found = rt
            .block_on(check_all(&client(), channels, &criteria()))
            .unwrap()
            .unwrap();
        assert_eq!(found.channel, Channel::Beta);
        assert_eq!(found.version.to_string(), "2.1.0-beta.1");

        // 跳过了测试版：各渠道先滤掉跳过的版本，稳定版照常提供。
        let stable = Server::start(vec![Reply::ok(manifest("2.0.1"))]);
        let beta = Server::start(vec![Reply::ok(manifest("2.1.0-beta.1"))]);
        let channels = vec![
            (Channel::Stable, vec![url(&stable, "/latest.json")]),
            (Channel::Beta, vec![url(&beta, "/latest.json")]),
        ];
        let skipped = Criteria {
            skipped: Some("v2.1.0-beta.1".to_owned()),
            ..criteria()
        };
        let found = rt
            .block_on(check_all(&client(), channels, &skipped))
            .unwrap()
            .unwrap();
        assert_eq!(found.channel, Channel::Stable);
    }

    #[test]
    fn one_responding_channel_is_enough() {
        let rt = runtime();
        let stable = Server::start(vec![Reply::ok(manifest("2.0.0"))]);
        let broken = Server::start(vec![Reply::status("500 Internal Server Error")]);
        let channels = vec![
            (Channel::Stable, vec![url(&stable, "/latest.json")]),
            (Channel::Beta, vec![url(&broken, "/latest.json")]),
        ];
        assert!(
            rt.block_on(check_all(&client(), channels, &criteria()))
                .unwrap()
                .is_none()
        );

        let broken = Server::start(vec![Reply::status("500 Internal Server Error")]);
        let channels = vec![(Channel::Stable, vec![url(&broken, "/latest.json")])];
        assert!(
            rt.block_on(check_all(&client(), channels, &criteria()))
                .is_err()
        );
    }

    #[test]
    fn system_requirements_and_missing_packages_mean_no_update() {
        let rt = runtime();
        let requires = serde_json::to_vec(&json!({
            "version": "2.2.0",
            "platforms": {"windows-x86_64": {"url": "https://dl.example.com/a.exe", "signature": "c2ln"}},
            "kwikpaste": {"schema": 1, "requires": {"windowsBuild": 99999, "macos": "99.0"}}
        }))
        .unwrap();
        let server = Server::start(vec![Reply::ok(requires)]);
        let found = rt
            .block_on(check_channel(
                &client(),
                Channel::Stable,
                &[url(&server, "/latest.json")],
                &criteria(),
            ))
            .unwrap();
        assert!(found.is_none());

        let server = Server::start(vec![Reply::ok(manifest("2.0.1"))]);
        let portable = Criteria {
            platform_keys: vec!["windows-x86_64-portable".to_owned()],
            ..criteria()
        };
        let found = rt
            .block_on(check_channel(
                &client(),
                Channel::Stable,
                &[url(&server, "/latest.json")],
                &portable,
            ))
            .unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn oversized_manifests_are_rejected() {
        let rt = runtime();
        let server = Server::start(vec![Reply::ok(vec![b' '; MAX_MANIFEST_BYTES + 1])]);
        assert!(
            rt.block_on(check_channel(
                &client(),
                Channel::Stable,
                &[url(&server, "/latest.json")],
                &criteria(),
            ))
            .is_err()
        );
    }
}
