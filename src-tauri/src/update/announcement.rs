//! 应用内公告：随检查更新从官网后端拉取，用原生对话框弹出。
//!
//! 约定见官网仓库 `deploy/announcements.md`（schema 1，对已发布的 1.x 冻结）。后端已经按版本、平台和
//! 系统筛选过，这里仍把每条规则重新判断一遍：CDN 快照是没筛选过的兜底来源，后台万一被攻破也不能把
//! 任意内容推到用户面前，所以只显示纯文本，链接只放行 https 的 fastthree.com / github.com / gitee.com。

use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::{
    DialogExt, MessageDialogButtons, MessageDialogKind, MessageDialogResult,
};
use tauri_plugin_opener::OpenerExt;
use url::Url;

use crate::core::paths;
use crate::i18n::announcement::words;
use crate::settings::{Language, SettingsStore};

const ENDPOINT: &str = "https://paste.fastthree.com/api/v1/announcements";
/// 后端连不上时的兜底：同样格式、没按客户端筛选的 CDN 快照。
const SNAPSHOT: &str = "https://dl.fastthree.com/kwikpaste/announcements.json";
const EVENTS_PATH: &str = "/api/v1/announcement-events";
/// 本地调试：设置后只从这个地址拉取（不读快照），开发构建也照常拉取。
const ENDPOINT_ENV: &str = "KWIKPASTE_ANNOUNCEMENT_ENDPOINT";
const STATE_FILENAME: &str = "announcements.json";
const STATE_VERSION: u16 = 1;
const SCHEMA: u64 = 1;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// 启动时的拉取和随后的自动检查往往前后脚到，这个间隔内不重复请求。
const MIN_FETCH_INTERVAL: Duration = Duration::from_secs(60);
const MAX_RESPONSE_BYTES: usize = 256 * 1024;
const TITLE_MAX: usize = 40;
const BODY_MAX: usize = 300;
const ACTION_MAX: usize = 12;
const ID_MAX: usize = 40;
const ALLOWED_HOSTS: [&str; 3] = ["fastthree.com", "github.com", "gitee.com"];
/// 状态文件最多记住这么多条公告，超出时丢掉最早弹出的。
const MAX_REMEMBERED: usize = 200;
const PLATFORM: &str = if cfg!(target_os = "macos") {
    "macos"
} else {
    "windows"
};

#[derive(Default)]
pub(super) struct AnnouncementState {
    gate: tokio::sync::Mutex<Option<Instant>>,
    /// 每次启动最多弹一条。
    shown_this_launch: AtomicBool,
}

#[derive(Clone, Copy)]
pub(super) enum Trigger {
    /// 应用启动后：只在开着自动检查更新时拉取，关掉自动检查的用户不发起额外的网络请求。
    Launch,
    /// 实际执行了一次检查更新（手动或到期的自动检查）。
    Check,
}

/// 在后台拉取并按需弹出，不阻塞检查更新；失败只记调试日志。
pub(super) fn schedule(app: &AppHandle, trigger: Trigger) {
    let Some(sources) = sources() else {
        return;
    };
    if matches!(trigger, Trigger::Launch)
        && !app.state::<SettingsStore>().snapshot().update.auto_check
    {
        return;
    }

    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = handle.state::<AnnouncementState>();
        let mut last_fetch = state.gate.lock().await;
        if state.shown_this_launch.load(Ordering::Relaxed)
            || last_fetch.is_some_and(|at| at.elapsed() < MIN_FETCH_INTERVAL)
        {
            return;
        }
        *last_fetch = Some(Instant::now());
        if let Err(err) = run(&handle, &state, &sources).await {
            log::debug!("announcement check skipped: {err:#}");
        }
    });
}

struct Sources {
    api: Url,
    snapshot: Option<Url>,
}

/// 开发构建默认不拉，避免本地调试混进线上统计。
fn sources() -> Option<Sources> {
    match std::env::var(ENDPOINT_ENV) {
        Ok(endpoint) if !endpoint.trim().is_empty() => {
            Url::parse(endpoint.trim()).ok().map(|api| Sources {
                api,
                snapshot: None,
            })
        }
        _ if cfg!(dev) => None,
        _ => Some(Sources {
            api: Url::parse(ENDPOINT).ok()?,
            snapshot: Url::parse(SNAPSHOT).ok(),
        }),
    }
}

async fn run(app: &AppHandle, state: &AnnouncementState, sources: &Sources) -> Result<()> {
    let lang = app.state::<SettingsStore>().snapshot().appearance.language;
    let facts = ClientFacts {
        app_version: app.package_info().version.clone(),
        os: os_version(),
    };
    let items = match fetch(&request_url(&sources.api, &facts, lang)).await {
        Ok(items) => items,
        Err(err) => {
            let Some(snapshot) = &sources.snapshot else {
                return Err(err);
            };
            log::debug!("announcement api unavailable, trying snapshot: {err:#}");
            fetch(snapshot).await?
        }
    };

    let now = Utc::now();
    let today = Local::now().date_naive();
    let path = paths::bootstrap_dir(app)?.join(STATE_FILENAME);
    let mut remembered = load_state(&path);
    let candidates: Vec<Announcement> = items
        .into_iter()
        .filter(|item| item.matches(&facts, now))
        .collect();
    let Some(item) = pick(&candidates, &remembered, today) else {
        return Ok(());
    };

    // 弹出前就记下来：对话框开着时应用崩溃或被关掉，重启后也不会重复弹。
    state.shown_this_launch.store(true, Ordering::Relaxed);
    remembered.mark_shown(&item.id, today);
    write_state(&path, &remembered)?;
    send_event(&sources.api, &item.id, "shown");

    match show(app, item, lang).await {
        Outcome::Clicked => {
            send_event(&sources.api, &item.id, "clicked");
            if let Some(action) = &item.action {
                if let Err(err) = app.opener().open_url(action.url.as_str(), None::<&str>) {
                    log::warn!("open announcement link failed: {err}");
                }
            }
        }
        Outcome::Dismissed => {
            send_event(&sources.api, &item.id, "dismissed");
            remembered.dismiss(&item.id);
            write_state(&path, &remembered)?;
        }
        Outcome::Closed => {}
    }

    Ok(())
}

/* ---------- 客户端事实 ---------- */

struct ClientFacts {
    app_version: semver::Version,
    os: Option<OsVersion>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OsVersion {
    WindowsBuild(u64),
    Macos((u64, u64, u64)),
}

impl OsVersion {
    fn query_value(self) -> String {
        match self {
            Self::WindowsBuild(build) => build.to_string(),
            Self::Macos((major, minor, patch)) => format!("{major}.{minor}.{patch}"),
        }
    }
}

/// Windows 取 build 号（`10.0.26100` 的第三段），macOS 取系统版本；读不出来就当作未知，
/// 带系统版本要求的公告一律不弹。
fn os_version() -> Option<OsVersion> {
    let version = tauri_plugin_os::version();
    let (major, minor, patch) = match version {
        tauri_plugin_os::Version::Semantic(major, minor, patch) => (major, minor, patch),
        other => parse_macos_version(&other.to_string())?,
    };
    if cfg!(target_os = "macos") {
        Some(OsVersion::Macos((major, minor, patch)))
    } else {
        (patch > 0).then_some(OsVersion::WindowsBuild(patch))
    }
}

fn request_url(api: &Url, facts: &ClientFacts, lang: Language) -> Url {
    let mut url = api.clone();
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("app_version", &facts.app_version.to_string());
        query.append_pair("platform", PLATFORM);
        if let Some(os) = facts.os {
            query.append_pair("os", &os.query_value());
        }
        query.append_pair(
            "locale",
            match lang {
                Language::ZhCN => "zh-CN",
                Language::EnUS => "en-US",
            },
        );
    }
    url
}

/* ---------- 拉取与校验 ---------- */

/// 独立的请求客户端，不跟随重定向，响应大小有上限。
async fn fetch(url: &Url) -> Result<Vec<Announcement>> {
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client.get(url.clone()).send().await?;
    let status = response.status();
    if !status.is_success() {
        bail!("announcement source answered {status}");
    }
    let bytes = response.bytes().await?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        bail!("announcement response is too large");
    }
    parse(&bytes)
}

#[derive(Deserialize)]
struct Envelope {
    schema: u64,
    #[serde(default)]
    items: Vec<serde_json::Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawItem {
    id: String,
    level: String,
    title: Localized,
    body: Localized,
    action: Option<RawAction>,
    #[serde(rename = "match")]
    rules: RawMatch,
    starts_at: Option<String>,
    ends_at: Option<String>,
    repeat: String,
    #[serde(default)]
    updated_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct Localized {
    #[serde(rename = "zh-CN")]
    zh: String,
    #[serde(rename = "en-US")]
    en: String,
}

impl Localized {
    fn get(&self, lang: Language) -> &str {
        match lang {
            Language::ZhCN => &self.zh,
            Language::EnUS => &self.en,
        }
    }
}

#[derive(Deserialize)]
struct RawAction {
    label: Localized,
    url: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawMatch {
    platforms: Vec<String>,
    min_app_version: Option<String>,
    max_app_version: Option<String>,
    min_windows_build: Option<u64>,
    min_macos: Option<String>,
}

#[derive(Clone, Debug)]
struct Action {
    label: Localized,
    url: Url,
}

#[derive(Clone, Debug)]
struct Announcement {
    id: String,
    important: bool,
    daily: bool,
    title: Localized,
    body: Localized,
    action: Option<Action>,
    platforms: Vec<String>,
    min_app_version: Option<semver::Version>,
    max_app_version: Option<semver::Version>,
    min_windows_build: Option<u64>,
    min_macos: Option<(u64, u64, u64)>,
    starts_at: Option<DateTime<Utc>>,
    ends_at: Option<DateTime<Utc>>,
    updated_at: Option<DateTime<Utc>>,
}

/// 格式版本不认识就整份忽略；单条结构或内容不合规只跳过那一条，不认识的字段直接忽略。
fn parse(bytes: &[u8]) -> Result<Vec<Announcement>> {
    let envelope: Envelope = serde_json::from_slice(bytes).context("parse announcements")?;
    if envelope.schema != SCHEMA {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    for value in envelope.items {
        let validated = serde_json::from_value::<RawItem>(value)
            .context("unexpected announcement shape")
            .and_then(validate);
        match validated {
            Ok(item) => items.push(item),
            Err(err) => log::debug!("skip announcement: {err:#}"),
        }
    }
    Ok(items)
}

fn validate(raw: RawItem) -> Result<Announcement> {
    if !valid_id(&raw.id) {
        bail!("invalid id");
    }
    let important = match raw.level.as_str() {
        "info" => false,
        "important" => true,
        _ => bail!("unknown level"),
    };
    let daily = match raw.repeat.as_str() {
        "once" => false,
        "daily" => true,
        _ => bail!("unknown repeat"),
    };
    check_localized(&raw.title, TITLE_MAX, false)?;
    check_localized(&raw.body, BODY_MAX, true)?;
    let action = match raw.action {
        Some(action) => {
            check_localized(&action.label, ACTION_MAX, false)?;
            let url = allowed_url(&action.url).context("link is not allowed")?;
            Some(Action {
                label: action.label,
                url,
            })
        }
        None => None,
    };
    if raw.rules.platforms.is_empty() {
        bail!("no platforms");
    }
    Ok(Announcement {
        id: raw.id,
        important,
        daily,
        title: raw.title,
        body: raw.body,
        action,
        platforms: raw.rules.platforms,
        min_app_version: raw
            .rules
            .min_app_version
            .as_deref()
            .map(parse_app_version)
            .transpose()?,
        max_app_version: raw
            .rules
            .max_app_version
            .as_deref()
            .map(parse_app_version)
            .transpose()?,
        min_windows_build: raw.rules.min_windows_build,
        min_macos: raw
            .rules
            .min_macos
            .as_deref()
            .map(|value| parse_macos_version(value).context("invalid macOS version"))
            .transpose()?,
        starts_at: raw.starts_at.as_deref().map(parse_time).transpose()?,
        ends_at: raw.ends_at.as_deref().map(parse_time).transpose()?,
        updated_at: raw
            .updated_at
            .as_deref()
            .and_then(|value| parse_time(value).ok()),
    })
}

fn valid_id(id: &str) -> bool {
    let mut chars = id.chars();
    id.len() <= ID_MAX
        && chars
            .next()
            .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn check_localized(text: &Localized, max: usize, multiline: bool) -> Result<()> {
    check_text(&text.zh, max, multiline)?;
    check_text(&text.en, max, multiline)
}

/// 纯文本：去掉首尾空白后不能为空、不超长，除正文里的换行外不能有控制字符。
fn check_text(text: &str, max: usize, multiline: bool) -> Result<()> {
    let text = text.trim();
    if text.is_empty() || text.chars().count() > max {
        bail!("text is empty or too long");
    }
    if text
        .chars()
        .any(|c| c.is_control() && !(multiline && c == '\n'))
    {
        bail!("text contains control characters");
    }
    Ok(())
}

/// https、默认端口、不带用户名密码，域名是白名单里的或它们的子域名。
fn allowed_url(value: &str) -> Option<Url> {
    let url = Url::parse(value).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let allowed = url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && ALLOWED_HOSTS
            .iter()
            .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")));
    allowed.then_some(url)
}

/// 版本比较按 semver 规则：预发布版低于同号正式版，构建元数据不参与比较。
fn parse_app_version(value: &str) -> Result<semver::Version> {
    let mut version = semver::Version::parse(value).context("invalid app version")?;
    version.build = semver::BuildMetadata::EMPTY;
    Ok(version)
}

/// `10.15`、`14.6.1` 这类点分版本，缺的段按 0 补齐。
fn parse_macos_version(value: &str) -> Option<(u64, u64, u64)> {
    let mut parts = value.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Some(0), |part| part.parse().ok())?;
    let patch = parts.next().map_or(Some(0), |part| part.parse().ok())?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn parse_time(value: &str) -> Result<DateTime<Utc>> {
    Ok(DateTime::parse_from_rfc3339(value)
        .context("invalid timestamp")?
        .with_timezone(&Utc))
}

impl Announcement {
    /// 与后端相同的规则；客户端拿不到的条件（读不出系统版本）按不满足处理，宁可不弹。
    fn matches(&self, facts: &ClientFacts, now: DateTime<Utc>) -> bool {
        if !self.platforms.iter().any(|platform| platform == PLATFORM) {
            return false;
        }
        if self.starts_at.is_some_and(|starts| starts > now)
            || self.ends_at.is_some_and(|ends| ends <= now)
        {
            return false;
        }
        let mut current = facts.app_version.clone();
        current.build = semver::BuildMetadata::EMPTY;
        if self
            .min_app_version
            .as_ref()
            .is_some_and(|min| current < *min)
            || self
                .max_app_version
                .as_ref()
                .is_some_and(|max| current > *max)
        {
            return false;
        }
        if PLATFORM == "windows" {
            if let Some(min) = self.min_windows_build {
                return matches!(facts.os, Some(OsVersion::WindowsBuild(build)) if build >= min);
            }
        } else if let Some(min) = self.min_macos {
            return matches!(facts.os, Some(OsVersion::Macos(version)) if version >= min);
        }
        true
    }
}

/* ---------- 挑选与记忆 ---------- */

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Remembered {
    version: u16,
    #[serde(default)]
    dismissed: Vec<String>,
    /// 每条公告最后一次弹出的本地日期。
    #[serde(default)]
    shown: BTreeMap<String, NaiveDate>,
    #[serde(default)]
    last_shown_day: Option<NaiveDate>,
}

impl Remembered {
    fn mark_shown(&mut self, id: &str, today: NaiveDate) {
        self.shown.insert(id.to_owned(), today);
        self.last_shown_day = Some(today);
        while self.shown.len() > MAX_REMEMBERED {
            let oldest = self
                .shown
                .iter()
                .min_by_key(|(_, day)| **day)
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => self.shown.remove(&id),
                None => break,
            };
        }
    }

    fn dismiss(&mut self, id: &str) {
        if !self.dismissed.iter().any(|known| known == id) {
            self.dismissed.push(id.to_owned());
        }
        let overflow = self.dismissed.len().saturating_sub(MAX_REMEMBERED);
        self.dismissed.drain(..overflow);
    }
}

/// 一天最多弹一次；重要的优先，其余按更新时间从新到旧。
/// 只提醒一次的弹过就不再弹，每天提醒的直到点「不再提醒」。
fn pick<'a>(
    items: &'a [Announcement],
    remembered: &Remembered,
    today: NaiveDate,
) -> Option<&'a Announcement> {
    if remembered.last_shown_day == Some(today) {
        return None;
    }
    items
        .iter()
        .filter(|item| !remembered.dismissed.contains(&item.id))
        .filter(|item| match remembered.shown.get(&item.id) {
            Some(day) => item.daily && *day != today,
            None => true,
        })
        .max_by(|a, b| {
            (a.important, a.updated_at)
                .cmp(&(b.important, b.updated_at))
                .then_with(|| b.id.cmp(&a.id))
        })
}

/// 文件缺失、损坏或格式版本不对都当作什么都没记住。
fn load_state(path: &Path) -> Remembered {
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Remembered>(&bytes).ok())
        .filter(|state| state.version == STATE_VERSION)
        .unwrap_or(Remembered {
            version: STATE_VERSION,
            ..Remembered::default()
        })
}

/// 放在启动锚点目录：不进 `settings.json`，也就不会随备份带到别的电脑上。
fn write_state(path: &Path, state: &Remembered) -> Result<()> {
    let parent = path
        .parent()
        .context("announcement state has no parent directory")?;
    fs::create_dir_all(parent).context("create announcement state directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec(state)?)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).context("save announcement state")?;
    Ok(())
}

/* ---------- 对话框 ---------- */

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    /// 点了公告自己的按钮：打开链接。
    Clicked,
    /// 点了「不再提醒」。
    Dismissed,
    /// 关掉（含「稍后」「关闭」「知道了」、Esc 和标题栏关闭）。
    Closed,
}

/// 按钮排布保证按 Esc 或点标题栏关闭永远落在「稍后 / 关闭」，不会被当成「不再提醒」。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    /// 只提醒一次、带链接：[按钮文字] [关闭]
    OnceLinked,
    /// 只提醒一次、没链接：[知道了]
    OncePlain,
    /// 每天提醒、带链接：[按钮文字] [不再提醒] [稍后]
    DailyLinked,
    /// 每天提醒、没链接：[不再提醒] [稍后]
    DailyPlain,
}

impl Layout {
    fn of(item: &Announcement) -> Self {
        match (item.daily, item.action.is_some()) {
            (false, true) => Self::OnceLinked,
            (false, false) => Self::OncePlain,
            (true, true) => Self::DailyLinked,
            (true, false) => Self::DailyPlain,
        }
    }

    /// 平台对话框可能返回按钮文字，也可能返回 Ok/Yes/No/Cancel，两种都按排布映射。
    fn outcome(self, result: &MessageDialogResult, action: &str, dismiss: &str) -> Outcome {
        match (self, result) {
            (Self::OnceLinked | Self::DailyLinked, MessageDialogResult::Custom(label))
                if label == action =>
            {
                Outcome::Clicked
            }
            (Self::DailyLinked | Self::DailyPlain, MessageDialogResult::Custom(label))
                if label == dismiss =>
            {
                Outcome::Dismissed
            }
            (Self::OnceLinked, MessageDialogResult::Ok)
            | (Self::DailyLinked, MessageDialogResult::Yes) => Outcome::Clicked,
            (Self::DailyLinked, MessageDialogResult::No)
            | (Self::DailyPlain, MessageDialogResult::Ok) => Outcome::Dismissed,
            _ => Outcome::Closed,
        }
    }
}

async fn show(app: &AppHandle, item: &Announcement, lang: Language) -> Outcome {
    let words = words(lang);
    let layout = Layout::of(item);
    let action = item
        .action
        .as_ref()
        .map(|action| action.label.get(lang).trim().to_owned())
        .unwrap_or_default();
    let buttons = match layout {
        Layout::OnceLinked => {
            MessageDialogButtons::OkCancelCustom(action.clone(), words.close.to_owned())
        }
        Layout::OncePlain => MessageDialogButtons::OkCustom(words.got_it.to_owned()),
        Layout::DailyLinked => MessageDialogButtons::YesNoCancelCustom(
            action.clone(),
            words.dismiss.to_owned(),
            words.later.to_owned(),
        ),
        Layout::DailyPlain => {
            MessageDialogButtons::OkCancelCustom(words.dismiss.to_owned(), words.later.to_owned())
        }
    };

    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .message(item.body.get(lang).trim())
        .title(item.title.get(lang).trim())
        .kind(MessageDialogKind::Info)
        .buttons(buttons)
        .show_with_result(move |result| {
            let _ = sender.send(result);
        });
    let result = receiver.await.unwrap_or(MessageDialogResult::Cancel);
    layout.outcome(&result, &action, words.dismiss)
}

/// 只带公告 id 和事件类型；发送失败不重试，也不影响用户操作。
fn send_event(api: &Url, id: &str, event: &'static str) {
    let Ok(url) = api.join(EVENTS_PATH) else {
        return;
    };
    let body = serde_json::json!({ "id": id, "event": event });
    tauri::async_runtime::spawn(async move {
        let sent = async {
            let client = reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            client
                .post(url)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(serde_json::to_vec(&body)?)
                .send()
                .await?;
            anyhow::Ok(())
        };
        if let Err(err) = sent.await {
            log::debug!("announcement event skipped: {err:#}");
        }
    });
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write as _},
        net::TcpListener,
        thread,
    };

    use super::*;

    fn at(time: &str) -> DateTime<Utc> {
        parse_time(time).unwrap()
    }

    fn day(value: &str) -> NaiveDate {
        NaiveDate::parse_from_str(value, "%Y-%m-%d").unwrap()
    }

    fn item_json(id: &str) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "level": "info",
            "title": { "zh-CN": "标题", "en-US": "Title" },
            "body": { "zh-CN": "第一行\n第二行", "en-US": "Body" },
            "action": { "label": { "zh-CN": "去下载", "en-US": "Download" }, "url": "https://paste.fastthree.com/" },
            "match": { "platforms": ["windows", "macos"], "minAppVersion": null, "maxAppVersion": null, "minWindowsBuild": null, "minMacos": null },
            "startsAt": null,
            "endsAt": null,
            "repeat": "once",
            "updatedAt": "2026-10-02T07:00:00Z",
            "futureField": { "ignored": true }
        })
    }

    fn items(values: Vec<serde_json::Value>) -> Vec<Announcement> {
        parse(&serde_json::to_vec(&serde_json::json!({ "schema": 1, "items": values })).unwrap())
            .unwrap()
    }

    fn one(mutate: impl FnOnce(&mut serde_json::Value)) -> Vec<Announcement> {
        let mut value = item_json("a");
        mutate(&mut value);
        items(vec![value])
    }

    fn facts(version: &str, os: Option<OsVersion>) -> ClientFacts {
        ClientFacts {
            app_version: semver::Version::parse(version).unwrap(),
            os,
        }
    }

    #[test]
    fn unknown_schema_is_ignored_and_bad_items_are_skipped() {
        let bytes =
            serde_json::to_vec(&serde_json::json!({ "schema": 2, "items": [item_json("a")] }))
                .unwrap();
        assert!(parse(&bytes).unwrap().is_empty());
        let mut broken = item_json("broken");
        broken["title"] = serde_json::json!("not localized");
        let parsed = items(vec![item_json("good"), broken, serde_json::json!(42)]);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "good");
        assert!(parse(b"not json").is_err());
    }

    #[test]
    fn content_rules_match_the_server() {
        assert_eq!(one(|_| {}).len(), 1);
        for mutate in [
            |v: &mut serde_json::Value| v["id"] = "Bad Id".into(),
            |v: &mut serde_json::Value| v["level"] = "urgent".into(),
            |v: &mut serde_json::Value| v["repeat"] = "weekly".into(),
            |v: &mut serde_json::Value| v["title"]["zh-CN"] = "长".repeat(41).into(),
            |v: &mut serde_json::Value| v["title"]["en-US"] = "tab\there".into(),
            |v: &mut serde_json::Value| v["title"]["zh-CN"] = "  ".into(),
            |v: &mut serde_json::Value| v["body"]["en-US"] = "x".repeat(301).into(),
            |v: &mut serde_json::Value| v["body"]["zh-CN"] = "line\r\nbreak".into(),
            |v: &mut serde_json::Value| {
                v["action"]["label"]["zh-CN"] = "十三个字的按钮文字太长了吧".into()
            },
            |v: &mut serde_json::Value| v["match"]["platforms"] = serde_json::json!([]),
            |v: &mut serde_json::Value| v["match"]["minAppVersion"] = "v2".into(),
            |v: &mut serde_json::Value| v["match"]["minMacos"] = "ten".into(),
            |v: &mut serde_json::Value| v["startsAt"] = "tomorrow".into(),
        ] {
            assert!(one(mutate).is_empty());
        }
    }

    #[test]
    fn links_are_limited_to_https_on_our_hosts() {
        for ok in [
            "https://paste.fastthree.com/",
            "https://fastthree.com/x",
            "https://github.com/ManSanDADADA/KwikPaste/releases",
            "https://gitee.com/mansan33",
            "https://paste.fastthree.com:443/",
        ] {
            assert!(allowed_url(ok).is_some(), "{ok}");
        }
        for bad in [
            "http://paste.fastthree.com/",
            "https://evil.example/",
            "https://fastthree.com.evil.example/",
            "https://evilfastthree.com/",
            "https://user:pw@paste.fastthree.com/",
            "https://paste.fastthree.com:8443/",
            "javascript:alert(1)",
            "file:///C:/Windows",
        ] {
            assert!(allowed_url(bad).is_none(), "{bad}");
        }
        assert!(one(|v| v["action"]["url"] = "https://evil.example/".into()).is_empty());
        let plain = one(|v| v["action"] = serde_json::Value::Null);
        assert!(plain[0].action.is_none());
    }

    #[test]
    fn audience_rules() {
        let now = at("2026-10-02T08:00:00Z");
        let win = Some(OsVersion::WindowsBuild(26100));
        let native = one(|v| {
            v["match"]["maxAppVersion"] = "1.99.99".into();
            v["match"]["minWindowsBuild"] = 17134.into();
            v["match"]["minMacos"] = "10.15".into();
        });
        assert!(native[0].matches(&facts("1.4.0", win), now));
        assert!(native[0].matches(&facts("1.4.0-beta.1", win), now));
        assert!(!native[0].matches(&facts("2.0.0", win), now));
        assert!(!native[0].matches(&facts("1.4.0", Some(OsVersion::WindowsBuild(14393))), now));
        assert!(!native[0].matches(&facts("1.4.0", None), now));

        let fresh = one(|v| v["match"]["minAppVersion"] = "1.4.0".into());
        assert!(!fresh[0].matches(&facts("1.4.0-beta.1", win), now));
        assert!(fresh[0].matches(&facts("1.4.0+build.7", win), now));

        let mac_only = one(|v| v["match"]["platforms"] = serde_json::json!(["macos"]));
        assert_eq!(
            mac_only[0].matches(&facts("1.4.0", win), now),
            PLATFORM == "macos"
        );

        let later = one(|v| v["startsAt"] = "2026-10-03T00:00:00Z".into());
        assert!(!later[0].matches(&facts("1.4.0", win), now));
        let over = one(|v| v["endsAt"] = "2026-10-02T08:00:00Z".into());
        assert!(!over[0].matches(&facts("1.4.0", win), now));
    }

    #[test]
    fn picking_follows_repeat_dismissal_and_daily_limit() {
        let mut once = item_json("once");
        once["level"] = "important".into();
        let mut daily = item_json("daily");
        daily["repeat"] = "daily".into();
        let list = items(vec![daily, once]);
        let today = day("2026-10-02");
        let mut remembered = Remembered::default();

        assert_eq!(pick(&list, &remembered, today).unwrap().id, "once");
        remembered.mark_shown("once", today);
        assert!(pick(&list, &remembered, today).is_none());

        let tomorrow = day("2026-10-03");
        assert_eq!(pick(&list, &remembered, tomorrow).unwrap().id, "daily");
        remembered.mark_shown("daily", tomorrow);
        let after = day("2026-10-04");
        assert_eq!(pick(&list, &remembered, after).unwrap().id, "daily");
        remembered.dismiss("daily");
        assert!(pick(&list, &remembered, after).is_none());
    }

    #[test]
    fn newer_announcement_wins_within_the_same_level() {
        let mut old = item_json("old");
        old["updatedAt"] = "2026-10-01T00:00:00Z".into();
        let list = items(vec![old, item_json("new")]);
        assert_eq!(
            pick(&list, &Remembered::default(), day("2026-10-02"))
                .unwrap()
                .id,
            "new"
        );
    }

    #[test]
    fn remembered_state_survives_and_stays_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(STATE_FILENAME);
        assert_eq!(load_state(&path).version, STATE_VERSION);
        let mut state = load_state(&path);
        for index in 0..(MAX_REMEMBERED + 5) {
            let id = format!("a{index}");
            state.mark_shown(&id, day("2026-01-01") + chrono::Days::new(index as u64));
            state.dismiss(&id);
        }
        write_state(&path, &state).unwrap();
        let loaded = load_state(&path);
        assert_eq!(loaded.shown.len(), MAX_REMEMBERED);
        assert_eq!(loaded.dismissed.len(), MAX_REMEMBERED);
        assert!(!loaded.shown.contains_key("a0"));
        assert!(!loaded.dismissed.iter().any(|id| id == "a0"));
        fs::write(&path, b"{ broken").unwrap();
        assert!(load_state(&path).shown.is_empty());
    }

    #[test]
    fn escape_never_counts_as_dismissal() {
        let cancel = MessageDialogResult::Cancel;
        for layout in [
            Layout::OnceLinked,
            Layout::OncePlain,
            Layout::DailyLinked,
            Layout::DailyPlain,
        ] {
            assert_eq!(
                layout.outcome(&cancel, "去下载", "不再提醒"),
                Outcome::Closed
            );
        }
        let custom = |label: &str| MessageDialogResult::Custom(label.to_owned());
        assert_eq!(
            Layout::DailyLinked.outcome(&custom("去下载"), "去下载", "不再提醒"),
            Outcome::Clicked
        );
        assert_eq!(
            Layout::DailyLinked.outcome(&custom("不再提醒"), "去下载", "不再提醒"),
            Outcome::Dismissed
        );
        assert_eq!(
            Layout::DailyLinked.outcome(&custom("稍后"), "去下载", "不再提醒"),
            Outcome::Closed
        );
        assert_eq!(
            Layout::DailyPlain.outcome(&custom("不再提醒"), "", "不再提醒"),
            Outcome::Dismissed
        );
        assert_eq!(
            Layout::OnceLinked.outcome(&MessageDialogResult::Ok, "去下载", "不再提醒"),
            Outcome::Clicked
        );
        assert_eq!(
            Layout::OncePlain.outcome(&custom("知道了"), "", "不再提醒"),
            Outcome::Closed
        );
    }

    #[test]
    fn request_carries_only_the_documented_facts() {
        let api = Url::parse(ENDPOINT).unwrap();
        let url = request_url(
            &api,
            &facts("1.4.0", Some(OsVersion::WindowsBuild(26100))),
            Language::ZhCN,
        );
        let pairs: Vec<(String, String)> = url.query_pairs().into_owned().collect();
        let keys: Vec<&str> = pairs.iter().map(|(key, _)| key.as_str()).collect();
        assert_eq!(keys, ["app_version", "platform", "os", "locale"]);
        assert!(pairs.contains(&("os".into(), "26100".into())));
        assert_eq!(OsVersion::Macos((10, 15, 7)).query_value(), "10.15.7");
        assert_eq!(parse_macos_version("15.0"), Some((15, 0, 0)));
        assert_eq!(parse_macos_version("10.15.7.1"), None);
    }

    /// 只接受一个连接的本地 HTTP 服务，返回固定的状态码和正文。
    fn serve_once(status: &str, body: Vec<u8>) -> Url {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_owned();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 2048];
            let _ = stream.read(&mut buffer);
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        });
        Url::parse(&format!("http://{address}/api/v1/announcements")).unwrap()
    }

    #[test]
    fn fetch_accepts_ok_json_and_rejects_errors_and_oversized_bodies() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let ok = serde_json::to_vec(&serde_json::json!({ "schema": 1, "items": [item_json("a")] }))
            .unwrap();
        let fetched = runtime.block_on(fetch(&serve_once("200 OK", ok))).unwrap();
        assert_eq!(fetched.len(), 1);
        assert!(runtime
            .block_on(fetch(&serve_once(
                "500 Internal Server Error",
                b"{}".to_vec()
            )))
            .is_err());
        assert!(runtime
            .block_on(fetch(&serve_once("302 Found", Vec::new())))
            .is_err());
        let huge = vec![b' '; MAX_RESPONSE_BYTES + 1];
        assert!(runtime
            .block_on(fetch(&serve_once("200 OK", huge)))
            .is_err());
    }
}
