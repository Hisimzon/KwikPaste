//! 反馈草稿与 UI 接口。仅采集配置、运行状态和日志，不读取剪贴板内容。

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Instant;

use gpui::{App, Task};
use kwikpaste_core::feedback::{Redactor, settings_changed};
use kwikpaste_core::{Core, settings::Settings};
use kwikpaste_updater::feedback::{self as transport, Payload, Result};
use serde_json::{Value, json};

pub use transport::{Attachment, FeedbackError, validate};

static STARTED: OnceLock<Instant> = OnceLock::new();

/// 入口启动时记下单调时钟，避免把首次打开反馈窗口当成启动时间。
pub fn mark_started() {
    let _ = STARTED.set(Instant::now());
}

#[derive(Clone)]
pub struct FeedbackDraft {
    /// 完整的脱敏附件供 UI 预览；上传限额不会改动这里的文本。
    pub attachments: Vec<Attachment>,
    payload: Payload,
}

pub struct Receipt {
    pub code: String,
}

/// 沙箱中系统目录 API 可能不可用，仍使用进程已有的用户目录环境变量脱敏。
fn profile_directory() -> String {
    dirs::home_dir()
        .or_else(|| {
            std::env::var_os(if cfg!(target_os = "windows") {
                "USERPROFILE"
            } else {
                "HOME"
            })
            .map(PathBuf::from)
        })
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// UI 打开弹窗时调用并 await；后台采集完成后展示 attachments，探针失败的字段为 null。
pub fn prepare(cx: &mut App) -> Task<Result<FeedbackDraft>> {
    let Some(core) = crate::core_host::core(cx).cloned() else {
        return Task::ready(Err(FeedbackError::Collect));
    };
    let settings = core.settings();
    let hotkeys = super::hotkey::feedback_status(cx, &settings.shortcuts);
    let triggers = triggers(&settings);
    let hooks = hooks(&settings);
    cx.background_executor().spawn(async move {
        let count = core.diagnostic_item_count().await.ok();
        collect(&core, &settings, hotkeys, triggers, hooks, count)
            .inspect_err(|error| log::warn!("feedback preparation did not complete: {error}"))
    })
}

/// UI 点击发送时传入预览过的草稿；await 成功后仅展示 code，失败可继续用同一草稿保存。
pub fn submit(
    draft: FeedbackDraft,
    description: String,
    contact: String,
    cx: &mut App,
) -> Task<Result<Receipt>> {
    let Some(core) = crate::core_host::core(cx) else {
        return Task::ready(Err(FeedbackError::Collect));
    };
    let runtime = core.runtime().clone();
    cx.background_executor().spawn(async move {
        let result = kwikpaste_core::runtime::hop(&runtime, async move {
            Ok(async move {
                let body = body(&draft, description, contact)?;
                let receipt = transport::upload(body).await?;
                Ok(Receipt { code: receipt.code })
            }
            .await)
        })
        .await
        .unwrap_or(Err(FeedbackError::Unavailable));
        match &result {
            Ok(receipt) => log::info!("feedback sent: {}", receipt.code),
            Err(error) => log::warn!("feedback was not sent: {error}"),
        }
        result
    })
}

/// UI 选择 .json.gz 路径后调用；与 submit 共用编码，上传失败后无需重新采集附件。
pub fn save(
    draft: FeedbackDraft,
    description: String,
    contact: String,
    path: PathBuf,
    cx: &mut App,
) -> Task<Result<()>> {
    cx.background_executor().spawn(async move {
        let result = (|| {
            let body = body(&draft, description, contact)?;
            std::fs::write(path, body).map_err(|_| FeedbackError::Save)
        })();
        if let Err(error) = &result {
            log::warn!("feedback file was not saved: {error}");
        }
        result
    })
}

fn body(draft: &FeedbackDraft, description: String, contact: String) -> Result<Vec<u8>> {
    let mut payload = draft.payload.clone();
    payload.description = description;
    payload.contact = contact;
    transport::encode(&payload)
}

fn triggers(settings: &Settings) -> Value {
    #[cfg(target_os = "windows")]
    let paused = Some(kwikpaste_os::win::trigger_pause::is_paused());
    #[cfg(target_os = "macos")]
    let paused = Some(kwikpaste_os::mac::trigger_pause::is_paused());
    json!({
        "pauseInFullscreen": settings.shortcuts.pause_in_fullscreen,
        "pauseAppCount": kwikpaste_core::app_ids::count_apps(settings.shortcuts.pause_app_ids.iter().map(String::as_str)),
        "paused": paused,
        "winV": settings.shortcuts.win_v,
        "doubleTap": settings.shortcuts.double_tap.key
    })
}

fn hooks(settings: &Settings) -> Value {
    #[cfg(target_os = "windows")]
    {
        use kwikpaste_core::settings::{DoubleTapKey, MouseTrigger};
        use kwikpaste_os::win::{keyboard, mouse, win_v};
        let shown = super::panel::is_shown();
        json!({
            "keyboardTrigger": {"expected": settings.shortcuts.win_v || !matches!(settings.shortcuts.double_tap.key, DoubleTapKey::Disabled | DoubleTapKey::Command), "running": win_v::is_active()},
            "panelKeyboard": {"expected": shown, "running": keyboard::is_running()},
            "mouse": {"expected": shown || settings.shortcuts.mouse_trigger != MouseTrigger::Disabled, "running": mouse::is_running()}
        })
    }
    #[cfg(target_os = "macos")]
    {
        let _ = settings;
        json!({"keyboardTrigger":{"expected":null,"running":null}, "panelKeyboard":{"expected":null,"running":null}, "mouse":{"expected":null,"running":null}})
    }
}

/// 磁盘读取与系统探针在后台执行，任何单项读不到都保留空值。
fn collect(
    core: &Core,
    settings: &Settings,
    hotkeys: Value,
    triggers: Value,
    hooks: Value,
    item_count: Option<i64>,
) -> Result<FeedbackDraft> {
    let os = kwikpaste_os::diagnostics::snapshot();
    let paths = core.paths();
    let app = transport::App {
        version: core.info().version.to_string(),
        official: option_env!("KWIKPASTE_OFFICIAL_BUILD").is_some(),
        platform: std::env::consts::OS.to_owned(),
        // 上报 app 契约要求字符串；诊断里仍用 null 表示查询失败。
        os_version: os.os_version.clone().unwrap_or_else(|| "unknown".into()),
        arch: std::env::consts::ARCH.to_owned(),
        language: serde_json::to_value(core.language())
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".into()),
    };
    let (watcher_running, watcher_paused) = core.watcher_status();
    let settings_value = serde_json::to_value(settings).ok();
    let changed = settings_value
        .as_ref()
        .zip(serde_json::to_value(Settings::default()).ok().as_ref())
        .map(|(current, default)| settings_changed(current, default));
    let extensions: Vec<_> = core
        .installed_extensions()
        .into_iter()
        .map(|(id, ext)| json!({"id":id,"version":ext.version}))
        .collect();
    let mut diagnostics = json!({
        "app": {"version":app.version,"official":app.official,"portable":paths.is_portable(),"environment":paths.env().dir_name(),
            "dataDir":paths.app_data_dir().ok(),"logDir":paths.log_dir(),"uptimeSeconds":STARTED.get().map(|start| start.elapsed().as_secs())},
        "system":{"platform":app.platform,"osVersion":os.os_version,"arch":app.arch,"language":app.language,"displayScales":os.display_scales,"elevated":os.elevated},
        "hotkeys":hotkeys,"triggers":triggers,"hooks":hooks,
        "foreground":{"exe":os.foreground_exe,"privilege":os.foreground_privilege},
        "clipboard":{"watcherRunning":watcher_running,"paused":watcher_paused,"itemCount":item_count,
            "databaseBytes":kwikpaste_core::db::path::db_path(paths).ok().and_then(|path| std::fs::metadata(path).ok()).map(|meta| meta.len())},
        "extensions":extensions,"settingsChanged":changed,
        "quickPaste":settings.shortcuts.quick_paste,"doubleTapSide":settings.shortcuts.double_tap.side
    });
    let home = profile_directory();
    let username = std::env::var(if cfg!(target_os = "windows") {
        "USERNAME"
    } else {
        "USER"
    })
    .unwrap_or_default();
    let redactor = Redactor::new(&home, &username, cfg!(target_os = "windows"))
        .map_err(|_| FeedbackError::Collect)?;
    redactor.value(&mut diagnostics);
    let mut stored_settings = paths
        .config_dir()
        .ok()
        .and_then(|dir| std::fs::read(dir.join("settings.json")).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .filter(Value::is_object)
        .unwrap_or(Value::Null);
    redactor.value(&mut stored_settings);
    let mut attachments = Vec::new();
    let mut logs = Vec::new();
    for name in ["KwikPaste.log", "KwikPaste.log.1"] {
        match std::fs::read(paths.log_dir().join(name)) {
            Ok(bytes) => {
                let attachment = Attachment {
                    name: name.into(),
                    text: redactor.text(&String::from_utf8_lossy(&bytes)),
                };
                attachments.push(attachment.clone());
                logs.push(attachment);
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => log::warn!("feedback log attachment is unreadable: {name}"),
        }
    }
    for (name, value) in [
        ("settings.json", &stored_settings),
        ("diagnostics.json", &diagnostics),
    ] {
        attachments.push(Attachment {
            name: name.into(),
            text: serde_json::to_string_pretty(value).map_err(|_| FeedbackError::Collect)?,
        });
    }
    Ok(FeedbackDraft {
        attachments,
        payload: Payload {
            schema: 1,
            description: String::new(),
            contact: String::new(),
            app,
            diagnostics,
            settings: stored_settings,
            logs,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_trigger_and_hook_shapes() {
        let settings = Settings::default();
        let triggers = triggers(&settings);
        assert_eq!(triggers["doubleTap"], "disabled");
        assert_eq!(triggers["pauseAppCount"], 0);
        assert_eq!(triggers.as_object().unwrap().len(), 5);
        let hooks = hooks(&settings);
        for key in ["keyboardTrigger", "panelKeyboard", "mouse"] {
            assert_eq!(hooks[key].as_object().unwrap().len(), 2);
        }
    }

    #[test]
    fn draft_uses_redacted_files_and_survives_missing_probes() {
        use kwikpaste_core::{AppEnv, AppInfo, CoreOptions, CorePaths, CoreRuntime, NoopSink};
        let temp = tempfile::tempdir().unwrap();
        let runtime = CoreRuntime::new().unwrap();
        let paths = CorePaths::new(
            AppEnv::Dev,
            temp.path().join("data"),
            temp.path().join("logs"),
            None,
        );
        let core = runtime
            .handle()
            .block_on(Core::start(
                AppInfo {
                    name: kwikpaste_core::APP_NAME,
                    identifier: "feedback-test",
                    version: semver::Version::new(2, 0, 5),
                    env: AppEnv::Dev,
                },
                paths,
                CoreOptions::default(),
                std::sync::Arc::new(NoopSink),
                runtime.handle(),
            ))
            .unwrap();
        assert_eq!(
            futures::executor::block_on(core.diagnostic_item_count()).unwrap(),
            0
        );
        assert_eq!(core.watcher_status(), (false, false));
        core.set_capture_paused(true);
        assert_eq!(core.watcher_status(), (false, true));
        std::fs::create_dir_all(core.paths().log_dir()).unwrap();
        let home = profile_directory();
        assert!(!home.is_empty());
        std::fs::write(
            core.paths().log_dir().join("KwikPaste.log"),
            format!("path: {home}/test"),
        )
        .unwrap();
        let settings = core.settings();
        let draft = collect(
            &core,
            &settings,
            json!({}),
            triggers(&settings),
            hooks(&settings),
            Some(0),
        )
        .unwrap();
        assert_eq!(draft.attachments.len(), 3);
        assert!(!draft.attachments[0].text.contains(&home));
        assert!(draft.payload.settings.is_object());
        assert_eq!(draft.payload.diagnostics["clipboard"]["itemCount"], 0);
        assert!(draft.payload.diagnostics["settingsChanged"].is_object());
        assert_eq!(
            body(&draft, "test".into(), String::new()).unwrap(),
            body(&draft, "test".into(), String::new()).unwrap()
        );
        std::fs::write(
            core.paths().config_dir().unwrap().join("settings.json"),
            b"broken json",
        )
        .unwrap();
        let draft = collect(
            &core,
            &settings,
            Value::Null,
            triggers(&settings),
            hooks(&settings),
            None,
        )
        .unwrap();
        assert!(draft.payload.settings.is_null());
        assert!(draft.payload.diagnostics["clipboard"]["itemCount"].is_null());
        assert!(draft.payload.diagnostics["hotkeys"].is_null());
        futures::executor::block_on(core.shutdown()).unwrap();
    }
}
