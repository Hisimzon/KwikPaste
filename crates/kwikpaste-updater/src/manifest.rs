//! 更新清单 `latest.json` 的解析，规则与 Tauri 的 updater 插件一致：
//!
//! - `version`（别名 `name`）去掉开头的 `v` 后按 semver 解析；
//! - `pub_date` 有值就必须是 RFC 3339，否则整个端点视为失败；
//! - `platforms` 是「平台键 → {url, signature}」，任何一项不合法整份失败；没有 `platforms` 时
//!   顶层必须有 `url` 和 `signature`（单平台格式）；
//! - 不认识的键忽略。
//!
//! 另加一个可选的 `kwikpaste` 块：`{"schema": 1, "requires": {"windowsBuild": 17134, "macos": "10.15"}}`。
//! 新版本要求的系统版本比本机高时视为没有更新：便携版和 macOS 换包没有安装程序替我们检查系统版本，
//! 装上一个起不来的版本就回不去了。`schema` 不是 1、`requires` 里有不认识的键或取值不合法，
//! 都算清单解析失败（这个端点不算成功响应）；`kwikpaste` 块里其它不认识的键忽略。

use std::collections::HashMap;

use anyhow::{Context, anyhow, bail};
use serde::Deserialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use url::Url;

use crate::os::OsVersion;

/// `kwikpaste` 块的格式版本。
const KWIKPASTE_SCHEMA: u64 = 1;

/// 一个平台的下载地址与 minisign 签名（base64）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformRelease {
    pub url: Url,
    pub signature: String,
}

/// 清单里的一个版本。
#[derive(Debug, Clone)]
pub struct Release {
    pub version: semver::Version,
    pub notes: Option<String>,
    pub pub_date: Option<OffsetDateTime>,
    platforms: Platforms,
    pub requires: Requirements,
}

#[derive(Debug, Clone)]
enum Platforms {
    /// 单平台格式：顶层的 `url` / `signature`，不区分平台键。
    Single(PlatformRelease),
    Static(HashMap<String, PlatformRelease>),
}

/// 新版本对系统版本的最低要求；没写的项不限。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Requirements {
    pub windows_build: Option<u64>,
    pub macos: Option<(u64, u64, u64)>,
}

impl Requirements {
    /// 本机系统是否满足。读不出系统版本时，有对应要求就按不满足处理。
    pub fn satisfied_by(&self, os: Option<OsVersion>) -> bool {
        if cfg!(target_os = "macos") {
            return match self.macos {
                None => true,
                Some(min) => matches!(os, Some(OsVersion::Macos(version)) if version >= min),
            };
        }

        match self.windows_build {
            None => true,
            Some(min) => matches!(os, Some(OsVersion::WindowsBuild(build)) if build >= min),
        }
    }
}

impl Release {
    /// 按顺序找第一个存在的平台键；单平台格式直接返回顶层地址。
    pub fn platform(&self, keys: &[String]) -> Option<(String, &PlatformRelease)> {
        match &self.platforms {
            Platforms::Single(platform) => {
                Some((keys.first().cloned().unwrap_or_default(), platform))
            }
            Platforms::Static(platforms) => keys
                .iter()
                .find_map(|key| platforms.get(key).map(|platform| (key.clone(), platform))),
        }
    }
}

#[derive(Deserialize)]
struct RawRelease {
    #[serde(alias = "name")]
    version: String,
    notes: Option<String>,
    pub_date: Option<String>,
    platforms: Option<HashMap<String, RawPlatform>>,
    url: Option<Url>,
    signature: Option<String>,
    kwikpaste: Option<RawKwikPaste>,
}

#[derive(Deserialize)]
struct RawPlatform {
    url: Url,
    signature: String,
}

#[derive(Deserialize)]
struct RawKwikPaste {
    schema: u64,
    requires: Option<RawRequirements>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawRequirements {
    windows_build: Option<u64>,
    macos: Option<String>,
}

/// 解析一份清单。
pub fn parse(bytes: &[u8]) -> anyhow::Result<Release> {
    let raw: RawRelease = serde_json::from_slice(bytes).context("invalid update manifest")?;
    let version = semver::Version::parse(raw.version.trim_start_matches('v'))
        .with_context(|| format!("invalid version {:?}", raw.version))?;
    let pub_date = raw
        .pub_date
        .map(|date| {
            OffsetDateTime::parse(&date, &Rfc3339)
                .map_err(|err| anyhow!("invalid value for `pub_date`: {err}"))
        })
        .transpose()?;
    let platforms = match raw.platforms {
        Some(platforms) => Platforms::Static(
            platforms
                .into_iter()
                .map(|(key, platform)| {
                    (
                        key,
                        PlatformRelease {
                            url: platform.url,
                            signature: platform.signature,
                        },
                    )
                })
                .collect(),
        ),
        None => Platforms::Single(PlatformRelease {
            url: raw
                .url
                .ok_or_else(|| anyhow!("the `url` field was not set on the updater response"))?,
            signature: raw.signature.ok_or_else(|| {
                anyhow!("the `signature` field was not set on the updater response")
            })?,
        }),
    };
    let requires = match raw.kwikpaste {
        Some(block) => requirements(block)?,
        None => Requirements::default(),
    };

    Ok(Release {
        version,
        notes: raw.notes,
        pub_date,
        platforms,
        requires,
    })
}

fn requirements(block: RawKwikPaste) -> anyhow::Result<Requirements> {
    if block.schema != KWIKPASTE_SCHEMA {
        bail!("unsupported kwikpaste manifest schema {}", block.schema);
    }
    let Some(raw) = block.requires else {
        return Ok(Requirements::default());
    };

    Ok(Requirements {
        windows_build: raw.windows_build,
        macos: raw
            .macos
            .as_deref()
            .map(|value| {
                crate::os::parse_macos_version(value)
                    .ok_or_else(|| anyhow!("invalid macOS requirement {value:?}"))
            })
            .transpose()?,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn manifest(value: serde_json::Value) -> anyhow::Result<Release> {
        parse(&serde_json::to_vec(&value).unwrap())
    }

    fn platforms() -> serde_json::Value {
        json!({
            "windows-x86_64-nsis": {"url": "https://dl.example.com/setup.exe", "signature": "c2ln"},
            "windows-x86_64": {"url": "https://dl.example.com/fallback.exe", "signature": "Zg=="},
        })
    }

    /// 发布流水线（scripts/release/v2-manifest.mjs）生成的清单：每种安装形态、每个架构查找的平台键都能
    /// 落到对的资产上，系统要求读得懂。样本由 `--sample` 生成，CI 核对它与脚本当前的输出一致。
    #[test]
    fn the_release_pipeline_manifest_serves_every_install_kind() {
        let release = parse(include_bytes!("../fixtures/latest-v2.sample.json")).unwrap();

        assert_eq!(release.version, semver::Version::new(2, 0, 0));
        assert_eq!(
            release.requires,
            Requirements {
                windows_build: Some(17134),
                macos: Some((10, 15, 0)),
            }
        );
        let asset = |keys: &[&str]| {
            let keys: Vec<String> = keys.iter().map(|key| (*key).to_owned()).collect();
            let (_, platform) = release.platform(&keys).unwrap();
            platform
                .url
                .path_segments()
                .unwrap()
                .next_back()
                .unwrap()
                .to_owned()
        };
        for (arch, setup, portable, app) in [
            ("x86_64", "x64", "x64", "x64"),
            ("aarch64", "arm64", "arm64", "aarch64"),
        ] {
            assert_eq!(
                asset(&[&format!("windows-{arch}-nsis"), &format!("windows-{arch}")]),
                format!("KwikPaste_2.0.0_{setup}-setup.exe")
            );
            assert_eq!(
                asset(&[&format!("windows-{arch}-portable")]),
                format!("KwikPaste_2.0.0_{portable}_portable.zip")
            );
            assert_eq!(
                asset(&[&format!("darwin-{arch}-app"), &format!("darwin-{arch}")]),
                format!("KwikPaste_2.0.0_{app}.app.tar.gz")
            );
        }
    }

    #[test]
    fn parses_the_tauri_static_format() {
        let release = manifest(json!({
            "version": "v2.0.1",
            "notes": "fixes",
            "pub_date": "2027-04-05T08:00:00Z",
            "platforms": platforms(),
            "unknownKey": [1, 2, 3]
        }))
        .unwrap();

        assert_eq!(release.version, semver::Version::new(2, 0, 1));
        assert_eq!(release.notes.as_deref(), Some("fixes"));
        assert!(release.pub_date.is_some());
        assert_eq!(release.requires, Requirements::default());
        let keys = [
            "windows-x86_64-nsis".to_owned(),
            "windows-x86_64".to_owned(),
        ];
        let (key, platform) = release.platform(&keys).unwrap();
        assert_eq!(key, "windows-x86_64-nsis");
        assert_eq!(platform.url.as_str(), "https://dl.example.com/setup.exe");
        assert!(release.platform(&["darwin-aarch64".to_owned()]).is_none());
    }

    #[test]
    fn name_is_an_alias_of_version_and_dates_are_optional() {
        let release = manifest(json!({"name": "2.1.0-beta.2", "platforms": platforms()})).unwrap();

        assert_eq!(
            release.version,
            semver::Version::parse("2.1.0-beta.2").unwrap()
        );
        assert!(release.pub_date.is_none());
    }

    #[test]
    fn single_platform_format_needs_url_and_signature() {
        let release = manifest(json!({
            "version": "2.0.1",
            "url": "https://dl.example.com/any.exe",
            "signature": "c2ln"
        }))
        .unwrap();
        let (_, platform) = release
            .platform(&["windows-x86_64-nsis".to_owned()])
            .unwrap();
        assert_eq!(platform.signature, "c2ln");

        assert!(manifest(json!({"version": "2.0.1", "url": "https://dl.example.com/a"})).is_err());
        assert!(manifest(json!({"version": "2.0.1", "signature": "c2ln"})).is_err());
    }

    #[test]
    fn invalid_fields_fail_the_whole_endpoint() {
        for bad in [
            json!({"version": "2.0.1", "pub_date": "2027-04-05 08:00:00", "platforms": platforms()}),
            json!({"version": "2.0", "platforms": platforms()}),
            json!({"platforms": platforms()}),
            json!({"version": "2.0.1", "platforms": {"windows-x86_64": {"url": "not a url", "signature": "x"}}}),
            json!({"version": "2.0.1", "platforms": {"windows-x86_64": {"url": "https://a.b/c"}}}),
            json!([1, 2]),
        ] {
            assert!(manifest(bad.clone()).is_err(), "{bad}");
        }
        assert!(parse(b"not json").is_err());
    }

    #[test]
    fn kwikpaste_block_carries_system_requirements() {
        let release = manifest(json!({
            "version": "2.1.0",
            "platforms": platforms(),
            "kwikpaste": {"schema": 1, "requires": {"windowsBuild": 19041, "macos": "11.0"}, "later": true}
        }))
        .unwrap();
        assert_eq!(
            release.requires,
            Requirements {
                windows_build: Some(19041),
                macos: Some((11, 0, 0)),
            }
        );

        let empty = manifest(
            json!({"version": "2.1.0", "platforms": platforms(), "kwikpaste": {"schema": 1}}),
        )
        .unwrap();
        assert_eq!(empty.requires, Requirements::default());

        for bad in [
            json!({"schema": 2}),
            json!({"schema": 1, "requires": {"linux": "6.0"}}),
            json!({"schema": 1, "requires": {"macos": "eleven"}}),
            json!({"schema": 1, "requires": {"windowsBuild": "19041"}}),
            json!({"requires": {}}),
        ] {
            let value = json!({"version": "2.1.0", "platforms": platforms(), "kwikpaste": bad});
            assert!(manifest(value).is_err(), "{bad}");
        }
    }

    #[test]
    fn requirements_compare_against_the_running_system() {
        let windows = Requirements {
            windows_build: Some(19041),
            macos: None,
        };
        let macos = Requirements {
            windows_build: None,
            macos: Some((11, 0, 0)),
        };
        let none = Requirements::default();

        assert!(none.satisfied_by(None));
        if cfg!(target_os = "macos") {
            assert!(macos.satisfied_by(Some(OsVersion::Macos((11, 0, 0)))));
            assert!(!macos.satisfied_by(Some(OsVersion::Macos((10, 15, 7)))));
            assert!(!macos.satisfied_by(None));
            assert!(windows.satisfied_by(None));
        } else {
            assert!(windows.satisfied_by(Some(OsVersion::WindowsBuild(26100))));
            assert!(!windows.satisfied_by(Some(OsVersion::WindowsBuild(17134))));
            assert!(!windows.satisfied_by(None));
            assert!(macos.satisfied_by(None));
        }
    }
}
