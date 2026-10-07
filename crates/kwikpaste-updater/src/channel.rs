//! 读哪份清单、清单里的版本能不能装。
//!
//! 2.x 只有一个更新渠道：发布了哪个版本（包括 `-beta.N` / `-rc.N`），所有 2.x 客户端就更新到它。
//! 先读七牛 CDN（国内可达）的 `kwikpaste/v2/stable/latest.json`，失败再读 GitHub 的 `channel-v2-stable`
//! 发布里的 `latest-v2.json`。地址里的 `stable` 是早先分渠道时留下的名字，2.0.0-beta.1 总会读它，所以不改。
//! GitHub 上 2.x 的资产永远不叫 `latest.json`：老客户端的备用地址是 `releases/latest/download/latest.json`，
//! 万一哪个 2.x 发布被标成 latest 也不会被读到。**绝不读 1.x 的旧地址**（`kwikpaste/{stable,beta,nightly}/latest.json`
//! 与 GitHub 的 `latest`、`channel-beta`、`channel-nightly`），那里只有 1.x。
//!
//! 设置里的 `update.includeBeta` / `update.includeNightly` 是 1.x 的渠道开关，2.x 不再读取。
//! 清单 404 表示还没发布过 2.x 版本，算没有更新，不算检查失败。

use anyhow::{Context, anyhow};
use url::Url;

use crate::manifest::{self, PlatformRelease, Release};
use crate::os::OsVersion;

const ENDPOINTS: &[&str] = &[
    "https://dl.fastthree.com/kwikpaste/v2/stable/latest.json",
    "https://github.com/ManSanDADADA/KwikPaste/releases/download/channel-v2-stable/latest-v2.json",
];
/// 清单正文上限，防止把一个误配置成大文件的地址整个读进内存。
const MAX_MANIFEST_BYTES: usize = 1024 * 1024;

/// 按优先级排列的镜像地址；`e2e-overrides` 下可以用环境变量换成单个测试地址。
pub(crate) fn endpoints() -> anyhow::Result<Vec<Url>> {
    if let Some(endpoint) = crate::overrides::var(crate::overrides::ENDPOINT) {
        return Ok(vec![
            Url::parse(&endpoint).context("invalid update endpoint override")?,
        ]);
    }

    ENDPOINTS
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
    pub version: semver::Version,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
    pub target: String,
    pub platform: PlatformRelease,
}

/// 依次读各镜像，第一份能用的清单说了算：`Ok(None)` 表示正常响应但没有可装的更新。
///
/// 没有镜像给出清单、但有镜像答 404 时，按还没发布过版本处理：清单就是不存在，算正常响应。
pub(crate) async fn check(
    client: &reqwest::Client,
    mirrors: &[Url],
    criteria: &Criteria,
) -> anyhow::Result<Option<Candidate>> {
    let mut missing = false;
    let mut last_error = None;
    for url in mirrors {
        match fetch(client, url).await {
            Ok(Fetched::NoContent) => return Ok(None),
            Ok(Fetched::Release(release)) => return Ok(select(*release, criteria)),
            Ok(Fetched::Missing) => {
                log::info!("update manifest {url} does not exist");
                missing = true;
            }
            Err(err) => {
                log::warn!("update manifest {url} unusable: {err:#}");
                last_error = Some(err);
            }
        }
    }
    if missing {
        log::info!("no 2.x release has been published yet");
        return Ok(None);
    }

    Err(last_error
        .unwrap_or_else(|| anyhow!("no update endpoint"))
        .context("failed to check for updates"))
}

enum Fetched {
    NoContent,
    /// 404：这个镜像上没有清单。
    Missing,
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
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(Fetched::Missing);
    }
    if !status.is_success() {
        anyhow::bail!("endpoint answered {status}");
    }

    let body = crate::http::read_limited(response, MAX_MANIFEST_BYTES).await?;
    Ok(Fetched::Release(Box::new(manifest::parse(&body)?)))
}

/// 清单里的版本能不能装：比当前新、没被跳过、系统满足要求、有本机的安装包。
fn select(release: Release, criteria: &Criteria) -> Option<Candidate> {
    if release.version <= criteria.current {
        return None;
    }
    let version = release.version.to_string();
    if criteria
        .skipped
        .as_deref()
        .is_some_and(|skipped| skipped.trim_start_matches('v') == version)
    {
        log::info!("update {version} was skipped by the user");
        return None;
    }
    if !release.requires.satisfied_by(criteria.os) {
        log::info!(
            "update {version} needs a newer system: {:?}",
            release.requires
        );
        return None;
    }
    let Some((target, platform)) = release.platform(&criteria.platform_keys) else {
        log::warn!(
            "update {version} has no package for {:?}",
            criteria.platform_keys
        );
        return None;
    };

    Some(Candidate {
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

    fn check_one(server: &Server, criteria: &Criteria) -> anyhow::Result<Option<Candidate>> {
        runtime().block_on(check(&client(), &[url(server, "/latest.json")], criteria))
    }

    /// 生产地址只有 v2，全部 https，绝不出现 1.x 的旧地址。
    #[test]
    fn production_endpoints_are_v2_only() {
        if cfg!(feature = "e2e-overrides") {
            return;
        }
        let all = endpoints().unwrap();

        assert_eq!(all.len(), 2);
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
    fn falls_back_to_the_next_mirror_when_the_first_is_unusable() {
        let rt = runtime();
        for first in [
            Reply::status("404 Not Found"),
            Reply::ok(b"{\"version\": \"2.1.0\", \"pub_date\": \"yesterday\", \"url\": \"https://a.b/c\", \"signature\": \"s\"}".to_vec()),
        ] {
            let server = Server::start(vec![first, Reply::ok(manifest("2.0.1"))]);
            let mirrors = [url(&server, "/cdn/latest.json"), url(&server, "/gh/latest.json")];

            let found = rt
                .block_on(check(&client(), &mirrors, &criteria()))
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
            .block_on(check(&client(), &mirrors, &criteria()))
            .unwrap();

        assert!(found.is_none());
        assert_eq!(server.requests().len(), 1);
    }

    /// 还没发布过版本：清单 404 算没有更新，一个镜像不可用、另一个答 404 也一样。
    #[test]
    fn missing_manifests_mean_no_release_yet() {
        let rt = runtime();
        for second in [
            Reply::status("404 Not Found"),
            Reply::status("500 Internal Server Error"),
        ] {
            let server = Server::start(vec![Reply::status("404 Not Found"), second]);
            let mirrors = [
                url(&server, "/cdn/latest.json"),
                url(&server, "/gh/latest.json"),
            ];

            let found = rt
                .block_on(check(&client(), &mirrors, &criteria()))
                .unwrap();

            assert!(found.is_none());
            assert_eq!(server.requests().len(), 2);
        }
    }

    /// 镜像都不可用才算检查失败。
    #[test]
    fn unusable_mirrors_fail_the_check() {
        let server = Server::start(vec![Reply::status("500 Internal Server Error")]);
        assert!(check_one(&server, &criteria()).is_err());
    }

    /// 测试版和正式版走同一个渠道：发布了测试版就提供测试版，跳过的版本不再提供。
    #[test]
    fn prereleases_are_offered_unless_skipped() {
        let server = Server::start(vec![Reply::ok(manifest("2.1.0-beta.1"))]);
        let found = check_one(&server, &criteria()).unwrap().unwrap();
        assert_eq!(found.version.to_string(), "2.1.0-beta.1");

        let server = Server::start(vec![Reply::ok(manifest("2.1.0-beta.1"))]);
        let skipped = Criteria {
            skipped: Some("v2.1.0-beta.1".to_owned()),
            ..criteria()
        };
        assert!(check_one(&server, &skipped).unwrap().is_none());
    }

    #[test]
    fn system_requirements_and_missing_packages_mean_no_update() {
        let requires = serde_json::to_vec(&json!({
            "version": "2.2.0",
            "platforms": {"windows-x86_64": {"url": "https://dl.example.com/a.exe", "signature": "c2ln"}},
            "kwikpaste": {"schema": 1, "requires": {"windowsBuild": 99999, "macos": "99.0"}}
        }))
        .unwrap();
        let server = Server::start(vec![Reply::ok(requires)]);
        assert!(check_one(&server, &criteria()).unwrap().is_none());

        let server = Server::start(vec![Reply::ok(manifest("2.0.1"))]);
        let portable = Criteria {
            platform_keys: vec!["windows-x86_64-portable".to_owned()],
            ..criteria()
        };
        assert!(check_one(&server, &portable).unwrap().is_none());
    }

    #[test]
    fn oversized_manifests_are_rejected() {
        let server = Server::start(vec![Reply::ok(vec![b' '; MAX_MANIFEST_BYTES + 1])]);
        assert!(check_one(&server, &criteria()).is_err());
    }
}
