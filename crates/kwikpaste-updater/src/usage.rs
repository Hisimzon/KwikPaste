//! 检查更新时附带的最小化使用统计（从 1.x 的 `update/usage.rs` 搬来，字段与语义不变），
//! 用来估算每日活跃安装、新增安装和版本分布；更新本身不依赖它。
//!
//! 请求只带每日随机 ID、上报类型、软件版本、系统、架构和界面语言。每日 ID 按 UTC 日轮换，
//! 不从机器指纹、账号或局域网同步身份派生；发送失败静默跳过，不影响检查更新。
//! 状态文件 `<bootstrap>/update-usage.json` 与 1.x 共用：同一台电脑从 1.x 换到 2.0 不算新增安装。
//! 开发构建默认不上报。

use std::fs;
use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use kwikpaste_core::settings::{Language, Settings};
use kwikpaste_core::{AppEnv, Core};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const ENDPOINT: &str = "https://paste.fastthree.com/api/v1/update-usage";
const STATE_FILENAME: &str = "update-usage.json";
const STATE_VERSION: u16 = 1;
const SYSTEM: &str = if cfg!(target_os = "macos") {
    "macos"
} else {
    "windows"
};

#[derive(Default)]
pub(crate) struct UsageState {
    gate: tokio::sync::Mutex<()>,
}

#[derive(Clone, Copy)]
pub(crate) enum Trigger {
    /// 应用启动：只在首次上报还没送达时发送。
    Launch,
    /// 实际执行了一次检查更新（手动或到期的自动检查）。
    Check,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Kind {
    First,
    Check,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DailyState {
    version: u16,
    day: String,
    daily_id: String,
    /// 新安装的首次上报还没送达；送达前每次上报都标成首次。
    first_pending: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Payload {
    daily_id: String,
    kind: Kind,
    version: String,
    system: &'static str,
    arch: &'static str,
    language: Language,
}

/// 在 core runtime 后台发送，检查更新不等网络和磁盘；多次触发排队执行，首次上报的状态读写不会交错。
pub(crate) fn schedule(core: &Core, state: &Arc<UsageState>, trigger: Trigger) {
    let Some(endpoint) = endpoint(core.info().env) else {
        return;
    };

    let core = core.clone();
    let state = state.clone();
    core.runtime().clone().spawn(async move {
        let _guard = state.gate.lock().await;
        if let Err(err) = report(&core, trigger, &endpoint).await {
            log::debug!("update usage report skipped: {err:#}");
        }
    });
}

/// 开发构建默认不发，避免本地调试混进线上统计；`e2e-overrides` 下可以指定地址（开发构建也发）。
fn endpoint(env: AppEnv) -> Option<String> {
    if let Some(endpoint) = crate::overrides::var(crate::overrides::USAGE_ENDPOINT) {
        return Some(endpoint);
    }
    (env == AppEnv::Prod).then(|| ENDPOINT.to_owned())
}

/// 每日 ID 在发送前落盘，重启或发送失败都不会让同一天换出第二个 ID；首次上报送达后才清掉待发标记。
async fn report(core: &Core, trigger: Trigger, endpoint: &str) -> Result<()> {
    let settings = core.settings();
    let path = core.paths().bootstrap_dir().join(STATE_FILENAME);
    let mut state = load_state(&path, Utc::now(), || is_fresh_install(&settings))?;
    let Some(kind) = kind_for(trigger, state.first_pending) else {
        return Ok(());
    };

    let payload = Payload {
        daily_id: state.daily_id.clone(),
        kind,
        version: core.info().version.to_string(),
        system: SYSTEM,
        arch: std::env::consts::ARCH,
        language: settings.appearance.language,
    };
    send_payload(&payload, endpoint).await?;

    if state.first_pending {
        state.first_pending = false;
        write_state(&path, &state)?;
    }

    Ok(())
}

fn kind_for(trigger: Trigger, first_pending: bool) -> Option<Kind> {
    match (trigger, first_pending) {
        (_, true) => Some(Kind::First),
        (Trigger::Check, false) => Some(Kind::Check),
        (Trigger::Launch, false) => None,
    }
}

/// 状态文件第一次创建时判断是否新安装：引导没走完、也从没检查过更新。
/// 从没有统计功能的旧版本升级上来的安装已有这些痕迹，不算新增。
fn is_fresh_install(settings: &Settings) -> bool {
    !settings.onboarding.completed && settings.update.last_checked_at.is_none()
}

/// 独立的请求客户端，禁止跟随重定向，统计字段不会被转发到别的主机。
async fn send_payload(payload: &Payload, endpoint: &str) -> Result<()> {
    let body = serde_json::to_vec(payload)?;
    let response = crate::http::no_redirect_client()?
        .post(endpoint)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await?;
    let status = response.status();
    if status != reqwest::StatusCode::NO_CONTENT {
        anyhow::bail!("usage endpoint answered {status}");
    }

    Ok(())
}

fn load_state(
    path: &Path,
    now: DateTime<Utc>,
    fresh_install: impl FnOnce() -> bool,
) -> Result<DailyState> {
    let previous = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<DailyState>(&bytes).ok(),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(err.into()),
    };
    let (state, changed) = state_for_day(previous, now, fresh_install);
    if changed {
        write_state(path, &state)?;
    }

    Ok(state)
}

/// 只保留当天的随机 ID：跨 UTC 日直接换新，不留旧 ID 或任何可推算的种子。
/// 文件缺失、损坏或格式版本不对时重建，并按当前设置重新判断是否新安装。
fn state_for_day(
    previous: Option<DailyState>,
    now: DateTime<Utc>,
    fresh_install: impl FnOnce() -> bool,
) -> (DailyState, bool) {
    let day = now.date_naive().to_string();
    let Some(previous) = previous.filter(|state| state.version == STATE_VERSION) else {
        let state = DailyState {
            version: STATE_VERSION,
            day,
            daily_id: Uuid::new_v4().to_string(),
            first_pending: fresh_install(),
        };
        return (state, true);
    };

    if previous.day == day && is_random_id(&previous.daily_id) {
        return (previous, false);
    }

    let state = DailyState {
        day,
        daily_id: Uuid::new_v4().to_string(),
        ..previous
    };
    (state, true)
}

fn is_random_id(value: &str) -> bool {
    Uuid::parse_str(value).is_ok_and(|id| id.get_version_num() == 4 && id.to_string() == value)
}

/// 小型运行状态放在启动锚点目录：不改已发布的设置结构，也不进历史备份。
fn write_state(path: &Path, state: &DailyState) -> Result<()> {
    let parent = path
        .parent()
        .context("usage state has no parent directory")?;
    fs::create_dir_all(parent).context("create usage state directory")?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(&serde_json::to_vec(state)?)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).context("save usage state")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::testing::{Reply, Server};

    fn at(time: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(time)
            .unwrap()
            .with_timezone(&Utc)
    }

    fn payload(kind: Kind) -> Payload {
        Payload {
            daily_id: Uuid::new_v4().to_string(),
            kind,
            version: "1.3.8".to_owned(),
            system: "windows",
            arch: "x86_64",
            language: Language::EnUS,
        }
    }

    #[test]
    fn same_day_restart_keeps_id_and_pending_first_report() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(STATE_FILENAME);
        let time = at("2026-09-30T12:00:00Z");

        let state = load_state(&path, time, || true).unwrap();
        let restored = load_state(&path, time, || panic!("state file exists")).unwrap();

        assert_eq!(restored.daily_id, state.daily_id);
        assert!(restored.first_pending);
    }

    #[test]
    fn next_utc_day_replaces_id_but_keeps_first_report_status() {
        let (state, _) = state_for_day(None, at("2026-09-30T23:59:59Z"), || true);
        let old = state.daily_id.clone();

        let (state, changed) = state_for_day(Some(state), at("2026-10-01T00:00:00Z"), || false);

        assert!(changed);
        assert_ne!(state.daily_id, old);
        assert_eq!(state.day, "2026-10-01");
        assert!(state.first_pending);
    }

    #[test]
    fn corrupt_state_and_invalid_id_are_replaced() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(STATE_FILENAME);
        fs::write(&path, b"not json").unwrap();
        let time = at("2026-09-30T12:00:00Z");

        let mut state = load_state(&path, time, || false).unwrap();
        assert!(is_random_id(&state.daily_id));
        assert!(!state.first_pending);

        state.daily_id = "not-a-random-id".to_owned();
        let (state, changed) = state_for_day(Some(state), time, || true);
        assert!(changed);
        assert!(is_random_id(&state.daily_id));
        assert!(!state.first_pending);
    }

    /// 1.x 写下的状态文件 2.0 原样读回：同一台电脑换到 2.0 不重新算新增安装。
    #[test]
    fn state_written_by_1x_is_reused() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join(STATE_FILENAME);
        let id = Uuid::new_v4().to_string();
        fs::write(
            &path,
            format!(r#"{{"version":1,"day":"2026-10-02","dailyId":"{id}","firstPending":false}}"#),
        )
        .unwrap();

        let state = load_state(&path, at("2026-10-02T09:00:00Z"), || true).unwrap();

        assert_eq!(state.daily_id, id);
        assert!(!state.first_pending);
        assert_eq!(kind_for(Trigger::Launch, state.first_pending), None);
    }

    #[test]
    fn only_untouched_settings_count_as_fresh_install() {
        let mut settings = Settings::default();
        assert!(is_fresh_install(&settings));

        settings.update.last_checked_at = Some("2026-09-29T00:00:00Z".to_owned());
        assert!(!is_fresh_install(&settings));

        settings.update.last_checked_at = None;
        settings.onboarding.completed = true;
        assert!(!is_fresh_install(&settings));
    }

    #[test]
    fn pending_first_report_wins_and_launch_alone_stays_quiet() {
        assert_eq!(kind_for(Trigger::Launch, true), Some(Kind::First));
        assert_eq!(kind_for(Trigger::Check, true), Some(Kind::First));
        assert_eq!(kind_for(Trigger::Check, false), Some(Kind::Check));
        assert_eq!(kind_for(Trigger::Launch, false), None);
    }

    #[test]
    fn payload_contains_exactly_the_six_fields() {
        let json = serde_json::to_value(payload(Kind::First)).unwrap();
        let object = json.as_object().unwrap();

        assert_eq!(object.len(), 6);
        for key in ["dailyId", "kind", "version", "system", "arch", "language"] {
            assert!(object.contains_key(key), "{key}");
        }
        assert_eq!(json["kind"], "first");
        assert_eq!(json["language"], "en-US");
        assert_eq!(serde_json::to_value(Kind::Check).unwrap(), "check");
    }

    /// 开发构建默认不上报；正式构建发往线上地址（这里只比较地址，不发请求）。
    #[test]
    fn development_builds_stay_quiet_by_default() {
        if cfg!(feature = "e2e-overrides") {
            return;
        }
        assert_eq!(endpoint(AppEnv::Dev), None);
        assert_eq!(endpoint(AppEnv::Prod).as_deref(), Some(ENDPOINT));
    }

    #[test]
    fn real_http_transport_sends_only_contract_and_rejects_failure_or_redirect() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for (status, ok) in [
            ("204 No Content", true),
            ("503 Service Unavailable", false),
            ("302 Found", false),
        ] {
            let server = Server::start(vec![
                Reply::status(status).header("Location", "http://127.0.0.1:1/must-not-follow"),
            ]);
            let endpoint = server.url("/api/v1/update-usage");

            let result = runtime.block_on(send_payload(&payload(Kind::Check), &endpoint));

            assert_eq!(result.is_ok(), ok, "{status}");
            let requests = server.requests();
            assert_eq!(requests.len(), 1);
            assert!(
                requests[0]
                    .line
                    .starts_with("POST /api/v1/update-usage HTTP/1.1")
            );
            let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
            assert_eq!(body.as_object().unwrap().len(), 6);
            assert_eq!(body["kind"], "check");
            assert_eq!(body["version"], "1.3.8");
        }
    }
}
