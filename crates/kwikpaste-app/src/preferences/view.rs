use gpui::{
    App, AppContext as _, Context, FocusHandle, Image, ImageSource, InteractiveElement as _,
    IntoElement, KeyDownEvent, Keystroke, ParentElement as _, Render, Role, ScrollHandle,
    StatefulInteractiveElement as _, Styled as _, Subscription, TitlebarOptions, WeakEntity,
    Window, WindowBounds, WindowOptions, div, img, prelude::FluentBuilder as _, px, rems, size,
};
use kwikpaste_core::{
    CoreEvent,
    backup::{self, BackupContainerMode, BackupExportMode, BackupImportStrategy},
    db::overview::{ClearScope, ContentCategory},
    ops::{PreferenceDirectory, StorageOverview},
    readable_export::{ExportFormat, ExportOptions, ExportPreview},
    settings::Settings,
    settings::{CaptureKind, RetentionRule, RetentionUnit},
    sync::{LanDeviceView, LanNearbyView, LanSyncState, PairTarget},
};
use kwikpaste_ui::{
    Button, Checkbox, DialogSpec, IconName, Input, KpStyled as _, NumberInput, NumberInputState,
    ScrollArea, Select, SelectOption, SelectState, Switch, TextInput, form_dialog,
    theme::{self, TextSize, space},
    toast::{self, Toast},
};
use serde_json::json;
use std::{collections::HashSet, path::PathBuf, sync::Arc};

use super::{
    icons::PrefIcon,
    schema::{self, Control, PermissionKind, Setting, TabId},
    text, values,
};
use crate::{
    clipboard::{self, source::ClipboardSource, view::group_dialogs},
    core_host, i18n,
    platform::{core_events, hotkey},
};

const WINDOW_MIN_SIZE: gpui::Size<gpui::Pixels> = size(px(960.), px(600.));

pub(crate) fn logo() -> Arc<Image> {
    static LOGO: std::sync::LazyLock<Arc<Image>> = std::sync::LazyLock::new(|| {
        let bytes: &[u8] = if cfg!(target_os = "macos") {
            include_bytes!("../../assets/logo-mac.png")
        } else {
            include_bytes!("../../assets/logo.png")
        };
        Arc::new(Image::from_bytes(gpui::ImageFormat::Png, bytes.to_vec()))
    });
    LOGO.clone()
}

fn window_size(cx: &App) -> gpui::Size<gpui::Pixels> {
    let scale = cx
        .try_global::<crate::platform::SystemSignals>()
        .map_or(1.0, |signals| signals.text_scale as f32);
    size(px(980. * scale), px(700. * scale))
}

fn initial_tab() -> TabId {
    if crate::selftest::active() {
        match std::env::var("KP_PREFERENCES_TAB").ok().as_deref() {
            Some("shortcuts") => TabId::Shortcuts,
            Some("appearance") => TabId::Appearance,
            Some("capture") => TabId::Capture,
            Some("window") => TabId::Window,
            Some("paste") => TabId::Paste,
            Some("items") => TabId::Items,
            Some("sync") => TabId::Sync,
            Some("overview") => TabId::Overview,
            Some("data") => TabId::Data,
            Some("about") => TabId::About,
            _ => TabId::General,
        }
    } else {
        TabId::General
    }
}

pub(super) fn open(cx: &mut App) -> anyhow::Result<()> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(window_size(cx), cx)),
        window_min_size: Some(WINDOW_MIN_SIZE),
        titlebar: Some(TitlebarOptions {
            title: Some(i18n::t("preferences:windowTitle")),
            ..Default::default()
        }),
        focus: false,
        ..Default::default()
    };
    crate::platform::open_window(options, cx, |window, cx| {
        let view = cx.new(|cx| Preferences::new(window, cx));
        view.update(cx, |this, cx| this.refresh_storage_overview(cx));
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        bring_window_to_front(window);
        view
    })
    .map(|_| ())
    .map_err(|error| anyhow::anyhow!("failed to open preferences window: {error:#}"))
}

pub(super) fn open_import(path: PathBuf, cx: &mut App) -> anyhow::Result<()> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(window_size(cx), cx)),
        window_min_size: Some(WINDOW_MIN_SIZE),
        titlebar: Some(TitlebarOptions {
            title: Some(i18n::t("preferences:windowTitle")),
            ..Default::default()
        }),
        focus: false,
        ..Default::default()
    };
    crate::platform::open_window(options, cx, |window, cx| {
        let view = cx.new(|cx| Preferences::new(window, cx));
        view.update(cx, |this, cx| this.refresh_storage_overview(cx));
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        bring_window_to_front(window);
        Preferences::show_import_confirmation(path.clone(), window, cx);
        view
    })
    .map(|_| ())
    .map_err(|error| anyhow::anyhow!("failed to open preferences window: {error:#}"))
}

#[cfg(target_os = "windows")]
pub(super) fn bring_window_to_front(window: &Window) {
    use kwikpaste_os::win::foreground::bring_to_front;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let Ok(handle) = HasWindowHandle::window_handle(window) else {
        return;
    };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return;
    };
    let _ = bring_to_front(handle.hwnd.get());
}

#[cfg(target_os = "macos")]
pub(super) fn bring_window_to_front(_: &Window) {
    // TODO: bridge to NSWindow makeKeyAndOrderFront without activating the panel.
    log::debug!("macOS preferences foreground handoff is not implemented yet");
}

struct Preferences {
    tab: TabId,
    settings: Settings,
    search: TextInput,
    _subscriptions: Vec<Subscription>,
    appearance: SelectState,
    language: SelectState,
    selects: std::collections::HashMap<&'static str, SelectState>,
    number_inputs: std::collections::HashMap<&'static str, NumberInputState>,
    scroll: ScrollHandle,
    focus: FocusHandle,
    recording: Option<&'static str>,
    storage_overview: Option<StorageOverview>,
    lan_state: Option<LanSyncState>,
    lan_code_hidden: bool,
    lan_name: TextInput,
    lan_max_image: NumberInputState,
}

impl Preferences {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = core_host::core(cx).map_or_else(Settings::default, |core| core.settings());
        let search = TextInput::new(i18n::t("preferences:search.placeholder"), window, cx);
        let search_subscription = search.on_change(cx, |_, _, cx| cx.notify());
        let theme = settings.appearance.theme;
        let language_value = settings.appearance.language;
        let appearance = SelectState::new(
            vec![
                SelectOption::new(
                    "auto",
                    i18n::t("preferences:schema.settings.appearance.theme.options.auto"),
                ),
                SelectOption::new(
                    "light",
                    i18n::t("preferences:schema.settings.appearance.theme.options.light"),
                ),
                SelectOption::new(
                    "dark",
                    i18n::t("preferences:schema.settings.appearance.theme.options.dark"),
                ),
            ],
            Some(match theme {
                kwikpaste_core::settings::Theme::Auto => "auto",
                kwikpaste_core::settings::Theme::Light => "light",
                kwikpaste_core::settings::Theme::Dark => "dark",
            }),
            window,
            cx,
        );
        let language = SelectState::new(
            vec![
                SelectOption::new(
                    "zh-CN",
                    i18n::t("preferences:schema.settings.appearance.language.options.zh-CN"),
                ),
                SelectOption::new("en-US", "English"),
            ],
            Some(match language_value {
                kwikpaste_core::settings::Language::ZhCN => "zh-CN",
                kwikpaste_core::settings::Language::EnUS => "en-US",
            }),
            window,
            cx,
        );
        let appearance_sub = appearance.on_change(cx, |this, value, cx| {
            if let Some(value) = value {
                this.update("appearance.theme", json!(value.as_ref()), cx);
            }
        });
        let language_sub = language.on_change(cx, |this, value, cx| {
            if let Some(value) = value {
                this.update("appearance.language", json!(value.as_ref()), cx);
            }
        });
        let lan_name = TextInput::new(
            i18n::t("preferences:schema.settings.sync.lan.deviceName.placeholder"),
            window,
            cx,
        );
        lan_name.set_value(settings.sync.lan.device_name.clone(), window, cx);
        let lan_name_sub = lan_name.on_change(cx, |this, value, cx| {
            this.update("sync.lan.deviceName", json!(value.to_string()), cx);
        });
        let lan_max_image = NumberInputState::new(
            u64::from(settings.sync.lan.max_image_mb),
            kwikpaste_core::settings::LAN_SYNC_MAX_IMAGE_MB_MIN.into(),
            kwikpaste_core::settings::LAN_SYNC_MAX_IMAGE_MB_MAX.into(),
            window,
            cx,
        );
        let lan_max_image_sub = lan_max_image.on_commit(window, cx, |this, value, _, cx| {
            this.update("sync.lan.maxImageMb", json!(value), cx);
        });
        let portable = core_host::core(cx).is_some_and(|core| core.paths().is_portable());
        let mut selects = std::collections::HashMap::new();
        let mut number_inputs = std::collections::HashMap::new();
        let mut setting_subscriptions = Vec::new();
        let settings_json = values::to_json(&settings);
        for tab in schema::tabs(portable) {
            for section in tab.sections {
                for setting in section.settings {
                    match setting.control {
                        Control::Select(kind) => {
                            let numeric = matches!(kind, schema::Options::Numbers(_));
                            let options = text::options(&setting)
                                .into_iter()
                                .map(|(value, label)| SelectOption::new(value, label))
                                .collect();
                            let selected =
                                values::get_choice(&settings_json, setting.path.unwrap_or(""));
                            let state = SelectState::new(options, selected.as_deref(), window, cx);
                            let path = setting.path;
                            let subscription = state.on_change(cx, move |this, value, cx| {
                                if let (Some(path), Some(value)) = (path, value) {
                                    this.apply_patch(
                                        values::choice_patch(path, &value, numeric),
                                        cx,
                                    );
                                }
                            });
                            setting_subscriptions.push(subscription);
                            selects.insert(setting.id, state);
                        }
                        Control::Tiles(kind) => {
                            let options = kind
                                .values()
                                .iter()
                                .map(|value| {
                                    SelectOption::new(
                                        *value,
                                        i18n::t(&format!(
                                            "preferences:schema.settings.{}.options.{}",
                                            setting.id, value
                                        )),
                                    )
                                })
                                .collect::<Vec<_>>();
                            let selected =
                                values::get_choice(&settings_json, setting.path.unwrap_or(""));
                            let state = SelectState::new(options, selected.as_deref(), window, cx);
                            let path = setting.path;
                            let subscription = state.on_change(cx, move |this, value, cx| {
                                if let (Some(path), Some(value)) = (path, value) {
                                    this.update(path, json!(value.as_ref()), cx);
                                }
                            });
                            setting_subscriptions.push(subscription);
                            selects.insert(setting.id, state);
                        }
                        Control::Number { min, max, .. } => {
                            let value = values::get(&settings_json, setting.path.unwrap_or(""))
                                .and_then(serde_json::Value::as_u64)
                                .unwrap_or(min);
                            let state = NumberInputState::new(value, min, max, window, cx);
                            let path = setting.path;
                            let subscription =
                                state.on_commit(window, cx, move |this, value, _, cx| {
                                    if let Some(path) = path {
                                        this.update(path, json!(value), cx);
                                    }
                                });
                            setting_subscriptions.push(subscription);
                            number_inputs.insert(setting.id, state);
                        }
                        _ => {}
                    }
                }
            }
        }
        let retention_value = NumberInputState::new(
            u64::from(settings.clipboard.history.retention.value),
            0,
            u64::from(u32::MAX),
            window,
            cx,
        );
        let retention_value_sub = retention_value.on_commit(window, cx, |this, value, _, cx| {
            this.update("clipboard.history.retention.value", json!(value), cx);
        });
        number_inputs.insert("history.retention.value", retention_value);
        let retention_unit = match settings.clipboard.history.retention.unit {
            kwikpaste_core::settings::RetentionUnit::Minutes => "minutes",
            kwikpaste_core::settings::RetentionUnit::Hours => "hours",
            kwikpaste_core::settings::RetentionUnit::Days => "days",
            kwikpaste_core::settings::RetentionUnit::Weeks => "weeks",
            kwikpaste_core::settings::RetentionUnit::Months => "months",
            kwikpaste_core::settings::RetentionUnit::Forever => "forever",
        };
        let retention_unit_state = SelectState::new(
            ["minutes", "hours", "days", "weeks", "months", "forever"]
                .into_iter()
                .map(|value| {
                    SelectOption::new(
                        value,
                        i18n::t(&format!("preferences:schema.retentionUnits.{value}")),
                    )
                })
                .collect(),
            Some(retention_unit),
            window,
            cx,
        );
        let retention_unit_sub = retention_unit_state.on_change(cx, |this, value, cx| {
            if let Some(value) = value {
                this.update(
                    "clipboard.history.retention.unit",
                    json!(value.as_ref()),
                    cx,
                );
            }
        });
        selects.insert("history.retention.unit", retention_unit_state);
        // 材质切换后窗口重套背板，分层底色也要跟着重画；选项显示的仍是用户的设置值（同 1.x）。
        let material_subscription =
            cx.observe_global::<crate::platform::material::WindowMaterial>(|_, cx| cx.notify());
        let lan_state = core_host::core(cx).map(|core| core.lan_sync_state());
        let subscriptions = core_events(cx)
            .map(|events| {
                cx.subscribe(&events, |this, _, event: &CoreEvent, cx| {
                    if matches!(
                        event,
                        CoreEvent::LanSyncChanged | CoreEvent::LanDevicePaired { .. }
                    ) && let Some(core) = core_host::core(cx)
                    {
                        this.lan_state = Some(core.lan_sync_state());
                        cx.notify();
                    }
                })
            })
            .into_iter()
            .chain([
                search_subscription,
                appearance_sub,
                language_sub,
                lan_name_sub,
                lan_max_image_sub,
                retention_value_sub,
                retention_unit_sub,
                material_subscription,
            ])
            .chain(setting_subscriptions)
            .collect();
        Self {
            tab: initial_tab(),
            settings,
            search,
            _subscriptions: subscriptions,
            appearance,
            language,
            selects,
            number_inputs,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            recording: None,
            storage_overview: None,
            lan_state,
            lan_code_hidden: false,
            lan_name,
            lan_max_image,
        }
    }

    fn update(&mut self, path: &'static str, value: serde_json::Value, cx: &mut Context<Self>) {
        let mut patch = values::patch(path, value.clone());
        if path == "clipboard.display.density"
            && value == "custom"
            && let Some(seed) = values::custom_density_seed(&self.settings)
        {
            patch = values::merge(patch, seed);
        }
        self.apply_patch(patch, cx);
    }

    /// 所有控件共用同一条落盘路径，不把结构化设置误写成字符串。
    fn apply_patch(&mut self, patch: serde_json::Value, cx: &mut Context<Self>) {
        if let Some(core) = core_host::core(cx).cloned() {
            let patch_for_task = patch.clone();
            cx.spawn(
                async move |this, cx| match core.update_settings(patch_for_task).await {
                    Ok(settings) => {
                        let _ = this.update(cx, |this, cx| {
                            this.settings = settings;
                            cx.notify();
                        });
                    }
                    Err(error) => log::error!("preferences update failed: {error}"),
                },
            )
            .detach();
        } else {
            let merged = values::merge(values::to_json(&self.settings), patch);
            if let Ok(settings) = serde_json::from_value(merged) {
                self.settings = settings;
            }
            cx.notify();
        }
    }

    fn begin_recording(&mut self, id: &'static str, window: &mut Window, cx: &mut Context<Self>) {
        self.recording = Some(id);
        hotkey::suspend(cx);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn finish_recording(&mut self, cx: &mut Context<Self>) {
        self.recording = None;
        hotkey::resume(cx);
        cx.notify();
    }

    fn capture_shortcut(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.recording else {
            return;
        };
        if event.keystroke.key.eq_ignore_ascii_case("escape") {
            self.finish_recording(cx);
            return;
        }
        if !event.keystroke.modifiers.control
            && !event.keystroke.modifiers.alt
            && !event.keystroke.modifiers.shift
            && !event.keystroke.modifiers.platform
            && matches!(
                event.keystroke.key.to_ascii_lowercase().as_str(),
                "backspace" | "delete"
            )
        {
            self.save_recorded_shortcut(id, String::new(), window, cx);
            return;
        }
        let Some(shortcut) = shortcut_from_keystroke(&event.keystroke) else {
            return;
        };
        if shortcut_conflicts_with_settings(id, &shortcut, &self.settings) {
            log::warn!("shortcut {shortcut:?} conflicts with an existing preference shortcut");
            toast::show(
                Toast::error(i18n::t("preferences:controls.shortcutConflict")),
                window,
                cx,
            );
            self.finish_recording(cx);
            return;
        }
        self.save_recorded_shortcut(id, shortcut, window, cx);
    }

    fn save_recorded_shortcut(
        &mut self,
        path_id: &'static str,
        value: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = shortcut_path(path_id) else {
            self.finish_recording(cx);
            return;
        };
        let patch = values::patch(path, json!(value));
        if let Some(core) = core_host::core(cx).cloned() {
            cx.spawn(async move |this, cx| {
                let result = core.update_settings(patch).await;
                let _ = this.update(cx, |this, cx| {
                    this.recording = None;
                    hotkey::resume(cx);
                    match result {
                        Ok(settings) => this.settings = settings,
                        Err(error) => log::error!("shortcut update failed: {error}"),
                    }
                    cx.notify();
                });
            })
            .detach();
        } else {
            let merged = values::merge(values::to_json(&self.settings), patch);
            if let Ok(settings) = serde_json::from_value(merged) {
                self.settings = settings;
            }
            self.finish_recording(cx);
        }
    }

    fn refresh_storage_overview(&mut self, cx: &mut Context<Self>) {
        let Some(core) = core_host::core(cx).cloned() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let result = core.storage_overview().await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(overview) => this.storage_overview = Some(overview),
                    Err(error) => log::warn!("storage overview failed: {error:#}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn clean_resource_cache(&mut self, cx: &mut Context<Self>) {
        let Some(core) = core_host::core(cx).cloned() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            match core.clean_resource_cache().await {
                Ok(result) => log::info!(
                    "preferences cleaned {} cache files ({} bytes)",
                    result.removed_files,
                    result.removed_bytes
                ),
                Err(error) => log::warn!("preferences cache cleanup failed: {error:#}"),
            }
            if let Ok(overview) = core.storage_overview().await {
                let _ = this.update(cx, |this, cx| {
                    this.storage_overview = Some(overview);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn run_history_cleanup(&mut self, cx: &mut Context<Self>) {
        let Some(core) = core_host::core(cx).cloned() else {
            return;
        };
        cx.spawn(async move |_this, _cx| match core.run_cleanup_now().await {
            Ok(result) => log::info!(
                "preferences history cleanup removed {} items",
                result.removed
            ),
            Err(error) => log::warn!("preferences history cleanup failed: {error:#}"),
        })
        .detach();
    }

    /// 打开确认框后按内容类别或来源应用清理普通记录；收藏与置顶由 core 保留。
    fn clear_storage_scope(
        &self,
        scope: ClearScope,
        title: gpui::SharedString,
        content: gpui::SharedString,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(core) = core_host::core(cx).cloned() else {
            return;
        };
        let entity = cx.entity().downgrade();
        let answer = form_dialog(
            DialogSpec::new(title)
                .ok_text(i18n::t("preferences:overview.clear.confirm"))
                .cancel_text(i18n::t("common:actions.cancel")),
            move |_, _| {
                div()
                    .kp_text(TextSize::Sm)
                    .child(content.clone())
                    .into_any_element()
            },
            window,
            cx,
        );
        window
            .spawn(cx, async move |cx| {
                if !answer.await.unwrap_or(false) {
                    return;
                }
                match core.clear_items_in_scope(scope).await {
                    Ok(removed) => {
                        let _ = entity.update_in(cx, |this, window, cx| {
                            toast::show(
                                Toast::success(i18n::t_args(
                                    "preferences:overview.clear.done",
                                    &[("count", &removed.to_string())],
                                )),
                                window,
                                cx,
                            );
                            this.refresh_storage_overview(cx);
                        });
                    }
                    Err(error) => log::warn!("scoped storage cleanup failed: {error:#}"),
                }
            })
            .detach();
    }

    fn open_directory(&self, target: PreferenceDirectory, cx: &mut Context<Self>) {
        let Some(core) = core_host::core(cx) else {
            return;
        };
        match core.preference_directory(target) {
            Ok(path) => cx.reveal_path(&path),
            Err(error) => log::warn!("could not open preference directory: {error:#}"),
        }
    }

    fn render_action(&self, id: &'static str, cx: &mut Context<Self>) -> gpui::AnyElement {
        let label = if id == "about.checkUpdates" {
            i18n::t("preferences:schema.settings.about.checkUpdates.controlLabel")
        } else if id == "localData.cleanCache" {
            i18n::t("preferences:schema.settings.localData.cleanCache.controlLabel")
        } else if id == "localData.clearHistory" {
            i18n::t("preferences:schema.settings.localData.clearHistory.controlLabel")
        } else if id == "organizing.customGroups" {
            i18n::t("preferences:schema.settings.organizing.customGroups.controlLabel")
        } else if id == "source.excludedApps" {
            i18n::t("preferences:schema.settings.source.excludedApps.controlLabel")
        } else if id == "history.cleanupStatus" {
            i18n::t("preferences:schema.settings.history.cleanupStatus.controlLabel")
        } else if id == "capture.order" {
            i18n::t("preferences:schema.settings.capture.order.title")
        } else if id == "actions.visible" {
            i18n::t("preferences:schema.settings.actions.visible.title")
        } else if id == "history.rules" {
            i18n::t("preferences:schema.settings.history.rules.title")
        } else {
            i18n::t("common:actions.open")
        };
        let entity = cx.entity().downgrade();
        Button::new(format!("action-{id}"), label)
            .on_click(move |_, window, cx| {
                let Some(entity) = entity.upgrade() else {
                    return;
                };
                entity.update(cx, |this, cx| match id {
                    "backup.exportHistory" => this.export_backup(window, cx),
                    "backup.importHistory" => this.import_backup(window, cx),
                    "backup.exportReadable" => this.export_readable(window, cx),
                    "localData.cleanCache" => this.clean_resource_cache(cx),
                    "history.cleanupStatus" | "localData.clearHistory" => {
                        this.run_history_cleanup(cx)
                    }
                    "localData.dataDirectory" => this.open_directory(PreferenceDirectory::Data, cx),
                    "localData.logDirectory" => this.open_directory(PreferenceDirectory::Logs, cx),
                    "organizing.customGroups" => this.open_group_manager(window, cx),
                    "source.excludedApps" => this.open_source_apps(window, cx),
                    "control.reopenOnboarding" => {
                        if let Err(error) = super::open_onboarding(cx) {
                            log::warn!("could not reopen onboarding: {error:#}");
                        }
                    }
                    "about.checkUpdates" => {
                        log::info!("manual update check requested from preferences")
                    }
                    _ => log::info!("preferences action requested: {id}"),
                });
            })
            .into_any_element()
    }

    fn move_capture_kind(&mut self, kind: CaptureKind, offset: i8, cx: &mut Context<Self>) {
        let order = self.settings.clipboard.capture.ordered_kinds();
        let Some(index) = order.iter().position(|candidate| *candidate == kind) else {
            return;
        };
        let target = index as i8 + offset;
        if target < 0 || target as usize >= order.len() {
            return;
        }
        self.move_capture_kind_to(kind, target as usize, cx);
    }

    /// 拖放按当前位置移入目标行；未启用的格式仍保留在顺序中，避免开关采集类型时丢失位置。
    fn move_capture_kind_to(&mut self, kind: CaptureKind, target: usize, cx: &mut Context<Self>) {
        let mut order = self.settings.clipboard.capture.ordered_kinds();
        if !reorder_capture_kinds(&mut order, kind, target) {
            return;
        }
        match serde_json::to_value(order) {
            Ok(value) => self.update("clipboard.capture.order", value, cx),
            Err(error) => log::warn!("could not encode capture order: {error}"),
        }
    }

    fn add_retention_rule(&self, window: &mut Window, cx: &mut Context<Self>) {
        let rules = &self.settings.clipboard.history.rules;
        let id = (1..=rules.len() + 1)
            .map(|index| format!("custom-{index}"))
            .find(|candidate| !rules.iter().any(|rule| rule.id == *candidate))
            .unwrap_or_else(|| format!("custom-{}", rules.len() + 1));
        let mut rule = RetentionRule {
            id,
            enabled: true,
            keep: kwikpaste_core::settings::Retention {
                value: 7,
                unit: RetentionUnit::Days,
            },
            ..RetentionRule::default()
        };
        rule.categories = Vec::new();
        self.open_retention_rule_editor(None, rule, window, cx);
    }

    fn update_retention_rules(&mut self, rules: Vec<RetentionRule>, cx: &mut Context<Self>) {
        match serde_json::to_value(rules) {
            Ok(value) => self.update("clipboard.history.rules", value, cx),
            Err(error) => log::warn!("could not encode retention rules: {error}"),
        }
    }

    fn edit_retention_rule(&self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rule) = self.settings.clipboard.history.rules.get(index).cloned() else {
            return;
        };
        self.open_retention_rule_editor(Some(index), rule, window, cx);
    }

    fn open_retention_rule_editor(
        &self,
        index: Option<usize>,
        base: RetentionRule,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let editor = cx.new(|cx| RetentionRuleEditor::new(&base, window, cx));
        let title_key = if index.is_some() {
            "preferences:retentionRules.form.titleEdit"
        } else {
            "preferences:retentionRules.form.titleCreate"
        };
        let content = editor.clone();
        let answer = form_dialog(
            DialogSpec::new(i18n::t(title_key))
                .ok_text(i18n::t("common:actions.save"))
                .cancel_text(i18n::t("common:actions.cancel")),
            move |_, _| content.clone().into_any_element(),
            window,
            cx,
        );
        let entity = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                if !answer.await.unwrap_or(false) {
                    return;
                }
                let Some(rule) = cx.update(|_, cx| editor.read(cx).to_rule(&base, cx)).ok() else {
                    return;
                };
                let _ = entity.update(cx, |this, cx| {
                    let mut rules = this.settings.clipboard.history.rules.clone();
                    if let Some(index) = index {
                        if let Some(current) = rules.get_mut(index) {
                            *current = rule;
                        }
                    } else {
                        rules.insert(0, rule);
                    }
                    this.update_retention_rules(rules, cx);
                });
            })
            .detach();
    }

    fn render_capture_order(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let entity = cx.entity().downgrade();
        let tokens = theme::tokens(cx);
        let order = self.settings.clipboard.capture.ordered_kinds();
        let rows = order.iter().enumerate().map(|(index, kind)| {
            let kind = *kind;
            let label = capture_kind_label(kind);
            let icon = capture_kind_icon(kind);
            let up = entity.clone();
            let down = entity.clone();
            div()
                .id(format!("capture-order-{index}"))
                .flex()
                .items_center()
                .gap(space(2.))
                .px(space(2.))
                .py(space(1.5))
                .border_b_1()
                .border_color(tokens.border_secondary)
                .on_drag(CaptureOrderDrag { kind }, |dragged, _, _, cx| {
                    cx.new(|_| *dragged)
                })
                .drag_over::<CaptureOrderDrag>(move |style, _, _, _| {
                    style.bg(tokens.fill_secondary)
                })
                .on_drop(cx.listener(move |this, dragged: &CaptureOrderDrag, _, cx| {
                    this.move_capture_kind_to(dragged.kind, index, cx);
                }))
                .child(PrefIcon::Grip.view(rems(1.), tokens.secondary))
                .child(icon.view(rems(1.), tokens.secondary))
                .child(div().flex_1().kp_text(TextSize::Sm).child(label))
                .child(
                    Button::new(
                        format!("capture-up-{index}"),
                        i18n::t("preferences:retentionRules.moveUp"),
                    )
                    .small()
                    .ghost()
                    .disabled(index == 0)
                    .on_click(move |_, _, cx| {
                        if let Some(entity) = up.upgrade() {
                            entity.update(cx, |this, cx| this.move_capture_kind(kind, -1, cx));
                        }
                    }),
                )
                .child(
                    Button::new(
                        format!("capture-down-{index}"),
                        i18n::t("preferences:retentionRules.moveDown"),
                    )
                    .small()
                    .ghost()
                    .disabled(index + 1 == order.len())
                    .on_click(move |_, _, cx| {
                        if let Some(entity) = down.upgrade() {
                            entity.update(cx, |this, cx| this.move_capture_kind(kind, 1, cx));
                        }
                    }),
                )
                .into_any_element()
        });
        div()
            .flex()
            .flex_col()
            .w_full()
            .rounded(theme::radius::SM)
            .border_1()
            .border_color(tokens.border_secondary)
            .children(rows)
            .into_any_element()
    }

    fn render_retention_rules(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let entity = cx.entity().downgrade();
        let tokens = theme::tokens(cx);
        let rules = &self.settings.clipboard.history.rules;
        let rows = rules.iter().enumerate().map(|(index, rule)| {
            let rule_id = rule.id.clone();
            let up = entity.clone();
            let down = entity.clone();
            let remove = entity.clone();
            let enabled = rule.enabled;
            let summary = retention_rule_summary(rule);
            let keep = retention_label(rule.keep.value, rule.keep.unit);
            div()
                .flex()
                .items_center()
                .gap(space(2.))
                .px(space(2.))
                .py(space(1.5))
                .border_b_1()
                .border_color(tokens.border_secondary)
                .child(
                    Switch::new(format!("retention-enabled-{index}"))
                        .small()
                        .checked(enabled)
                        .accessibility_label(summary.clone())
                        .on_change({
                            let entity = entity.clone();
                            let id = rule_id.clone();
                            move |checked, _, cx| {
                                if let Some(entity) = entity.upgrade() {
                                    entity.update(cx, |this, cx| {
                                        let mut next =
                                            this.settings.clipboard.history.rules.clone();
                                        if let Some(rule) =
                                            next.iter_mut().find(|rule| rule.id == id)
                                        {
                                            rule.enabled = checked;
                                        }
                                        this.update_retention_rules(next, cx);
                                    });
                                }
                            }
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(space(0.5))
                        .flex_1()
                        .child(div().kp_text(TextSize::Sm).child(summary))
                        .child(
                            div()
                                .kp_text(TextSize::Xs)
                                .text_color(tokens.secondary)
                                .child(if enabled {
                                    keep
                                } else {
                                    i18n::t("preferences:retentionRules.disabled")
                                }),
                        ),
                )
                .child(
                    Button::new(
                        format!("retention-up-{index}"),
                        i18n::t("preferences:retentionRules.moveUp"),
                    )
                    .small()
                    .ghost()
                    .disabled(index == 0)
                    .on_click(move |_, _, cx| {
                        if let Some(entity) = up.upgrade() {
                            entity.update(cx, |this, cx| {
                                let mut next = this.settings.clipboard.history.rules.clone();
                                if index > 0 {
                                    next.swap(index, index - 1);
                                    this.update_retention_rules(next, cx);
                                }
                            });
                        }
                    }),
                )
                .child(
                    Button::new(
                        format!("retention-down-{index}"),
                        i18n::t("preferences:retentionRules.moveDown"),
                    )
                    .small()
                    .ghost()
                    .disabled(index + 1 >= rules.len())
                    .on_click(move |_, _, cx| {
                        if let Some(entity) = down.upgrade() {
                            entity.update(cx, |this, cx| {
                                let mut next = this.settings.clipboard.history.rules.clone();
                                if index + 1 < next.len() {
                                    next.swap(index, index + 1);
                                    this.update_retention_rules(next, cx);
                                }
                            });
                        }
                    }),
                )
                .child(
                    Button::new(
                        format!("retention-edit-{index}"),
                        i18n::t("preferences:retentionRules.edit"),
                    )
                    .small()
                    .ghost()
                    .on_click({
                        let entity = entity.clone();
                        move |_, window, cx| {
                            if let Some(entity) = entity.upgrade() {
                                entity.update(cx, |this, cx| {
                                    this.edit_retention_rule(index, window, cx);
                                });
                            }
                        }
                    }),
                )
                .child(
                    Button::new(
                        format!("retention-delete-{index}"),
                        i18n::t("preferences:retentionRules.delete"),
                    )
                    .small()
                    .danger_outline()
                    .on_click(move |_, _, cx| {
                        if let Some(entity) = remove.upgrade() {
                            entity.update(cx, |this, cx| {
                                let next = this
                                    .settings
                                    .clipboard
                                    .history
                                    .rules
                                    .iter()
                                    .filter(|rule| rule.id != rule_id)
                                    .cloned()
                                    .collect();
                                this.update_retention_rules(next, cx);
                            });
                        }
                    }),
                )
                .into_any_element()
        });
        let add = entity.clone();
        div()
            .flex()
            .flex_col()
            .w_full()
            .gap(space(2.))
            .child(if rules.is_empty() {
                div()
                    .kp_text(TextSize::Xs)
                    .text_color(tokens.secondary)
                    .child(i18n::t("preferences:retentionRules.empty"))
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .flex_col()
                    .rounded(theme::radius::SM)
                    .border_1()
                    .border_color(tokens.border_secondary)
                    .children(rows)
                    .into_any_element()
            })
            .child(
                Button::new("retention-add", i18n::t("preferences:retentionRules.add"))
                    .small()
                    .with_icon(IconName::Plus)
                    .on_click(move |_, window, cx| {
                        if let Some(entity) = add.upgrade() {
                            entity.update(cx, |this, cx| this.add_retention_rule(window, cx));
                        }
                    }),
            )
            .into_any_element()
    }

    fn render_storage_overview(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(overview) = self.storage_overview.as_ref() else {
            let entity = cx.entity().downgrade();
            return div()
                .flex()
                .items_center()
                .gap(space(2.))
                .child(i18n::t("preferences:storage.loading"))
                .child(
                    Button::new("storage-refresh", i18n::t("preferences:overview.refresh"))
                        .ghost()
                        .on_click(move |_, _, cx| {
                            if let Some(entity) = entity.upgrade() {
                                entity.update(cx, |this, cx| {
                                    this.refresh_storage_overview(cx);
                                });
                            }
                        }),
                )
                .into_any_element();
        };
        let entity = cx.entity().downgrade();
        let usage = &overview.usage;
        let reclaimable = overview.reclaimable.bytes;
        let reclaimable_count = overview.reclaimable.files.to_string();
        let reclaimable_size = format_bytes(reclaimable);
        let reclaimable_label = i18n::t_args(
            "preferences:overview.space.reclaimable",
            &[("count", &reclaimable_count), ("size", &reclaimable_size)],
        );
        let limit = self.settings.clipboard.history.storage_limit_mb as u64 * 1024 * 1024;
        let used_ratio = if limit == 0 {
            0.0
        } else {
            (usage.total_bytes as f32 / limit as f32).clamp(0.0, 1.0)
        };
        let used_width = 40. * used_ratio.max(0.02);
        let used_bytes = usage.total_bytes.max(1) as f32;
        let storage_segment = |bytes: u64, color| {
            div()
                .h_full()
                .w(rems(used_width * (bytes as f32 / used_bytes)))
                .bg(color)
        };
        let total_records = overview.history.totals.total;
        let today_records = overview.history.daily.last().map_or(0, |day| day.count);
        let reuse_count = overview.history.totals.reuses;
        let oldest = overview
            .history
            .oldest_date
            .map_or_else(|| "--".to_owned(), |date| date.to_string());
        let total_hint = i18n::t_args(
            "preferences:overview.tiles.total.today",
            &[("value", &today_records.to_string())],
        );
        let recent_hint = i18n::t_args("preferences:overview.trend.subtitle", &[("count", "30")]);
        let span_hint = if oldest == "--" {
            i18n::t("preferences:overview.tiles.span.empty")
        } else {
            i18n::t_args(
                "preferences:overview.tiles.span.since",
                &[("date", &oldest)],
            )
        };
        let category_entity = entity.clone();
        let category_rows = overview
            .history
            .categories
            .iter()
            .filter(|stat| stat.count > 0)
            .map(|stat| {
                let name = category_label(stat.category);
                let count = stat.count.to_string();
                let removable = stat.removable;
                let entity = category_entity.clone();
                let scope = ClearScope::Category {
                    category: stat.category,
                };
                let title = i18n::t_args(
                    "preferences:overview.clear.categoryTitle",
                    &[("name", name.as_ref())],
                );
                let content = i18n::t_args(
                    if removable == stat.count {
                        "preferences:overview.clear.content"
                    } else {
                        "preferences:overview.clear.contentWithKept"
                    },
                    &[
                        ("value", &removable.to_string()),
                        ("kept", &(stat.count - removable).to_string()),
                    ],
                );
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(space(2.))
                    .child(format!("{name}: {count}"))
                    .child(
                        Button::new(
                            format!("storage-category-{:?}", stat.category),
                            i18n::t("preferences:overview.clear.tooltip"),
                        )
                        .ghost()
                        .disabled(removable == 0)
                        .on_click(move |_, window, cx| {
                            if let Some(entity) = entity.upgrade() {
                                entity.update(cx, |this, cx| {
                                    this.clear_storage_scope(
                                        scope.clone(),
                                        title.clone(),
                                        content.clone(),
                                        window,
                                        cx,
                                    );
                                });
                            }
                        }),
                    )
                    .into_any_element()
            });
        let source_entity = entity.clone();
        let source_rows = overview.history.source_apps.iter().map(|stat| {
            let name = stat
                .name
                .clone()
                .unwrap_or_else(|| i18n::t("preferences:overview.sources.unknown").to_string());
            let removable = stat.removable;
            let entity = source_entity.clone();
            let scope = ClearScope::SourceApp {
                app_id: stat.app_id.clone(),
            };
            let title = i18n::t_args(
                "preferences:overview.clear.sourceAppTitle",
                &[("name", &name)],
            );
            let content = i18n::t_args(
                if removable == stat.count {
                    "preferences:overview.clear.content"
                } else {
                    "preferences:overview.clear.contentWithKept"
                },
                &[
                    ("value", &removable.to_string()),
                    ("kept", &(stat.count - removable).to_string()),
                ],
            );
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(space(2.))
                .child(format!("{}: {}", name, stat.count))
                .child(
                    Button::new(
                        format!(
                            "storage-source-{}",
                            stat.app_id.as_deref().unwrap_or("unknown")
                        ),
                        i18n::t("preferences:overview.clear.tooltip"),
                    )
                    .ghost()
                    .disabled(removable == 0)
                    .on_click(move |_, window, cx| {
                        if let Some(entity) = entity.upgrade() {
                            entity.update(cx, |this, cx| {
                                this.clear_storage_scope(
                                    scope.clone(),
                                    title.clone(),
                                    content.clone(),
                                    window,
                                    cx,
                                );
                            });
                        }
                    }),
                )
                .into_any_element()
        });
        div()
            .flex()
            .flex_col()
            .gap(space(2.))
            .p(space(3.))
            .rounded(theme::radius::MD)
            .bg(theme::tokens(cx).bg_elevated)
            .child(
                div()
                    .kp_text(TextSize::Sm)
                    .child(i18n::t("preferences:overview.space.title")),
            )
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(space(1.))
                    .child(
                        div()
                            .kp_text(TextSize::Lg)
                            .child(format_bytes(usage.total_bytes)),
                    )
                    .child(
                        div()
                            .kp_text(TextSize::Xs)
                            .text_color(theme::tokens(cx).secondary)
                            .child(format!("/ {}", format_bytes(limit),)),
                    ),
            )
            .child(
                div()
                    .h(px(8.))
                    .w_full()
                    .rounded(theme::radius::SM)
                    .bg(theme::tokens(cx).fill_secondary)
                    .child(
                        div()
                            .flex()
                            .h_full()
                            .w(rems(used_width))
                            .rounded(theme::radius::SM)
                            .overflow_hidden()
                            .children([
                                storage_segment(
                                    overview.breakdown.database_bytes,
                                    theme::tokens(cx).primary,
                                ),
                                storage_segment(
                                    overview.breakdown.image_bytes,
                                    theme::tokens(cx).info,
                                ),
                                storage_segment(
                                    overview.breakdown.icon_bytes,
                                    theme::tokens(cx).warning,
                                ),
                                storage_segment(
                                    overview.breakdown.other_bytes,
                                    theme::tokens(cx).secondary,
                                ),
                            ]),
                    ),
            )
            .children([
                format_storage_row(
                    i18n::t("preferences:overview.space.segments.database"),
                    overview.breakdown.database_bytes,
                ),
                format_storage_row(
                    i18n::t("preferences:overview.space.segments.image"),
                    overview.breakdown.image_bytes,
                ),
                format_storage_row(
                    i18n::t("preferences:overview.space.segments.icon"),
                    overview.breakdown.icon_bytes,
                ),
                format_storage_row(
                    i18n::t("preferences:overview.space.segments.other"),
                    overview.breakdown.other_bytes,
                ),
            ])
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space(2.))
                    .child(reclaimable_label)
                    .child(
                        Button::new(
                            "storage-clean",
                            i18n::t("preferences:overview.space.cleanCache"),
                        )
                        .ghost()
                        .on_click(move |_, _, cx| {
                            if let Some(entity) = entity.upgrade() {
                                entity.update(cx, |this, cx| {
                                    this.clean_resource_cache(cx);
                                });
                            }
                        }),
                    )
                    .child(
                        Button::new(
                            "storage-open",
                            i18n::t("preferences:overview.openDirectory"),
                        )
                        .ghost()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_directory(PreferenceDirectory::Data, cx);
                        })),
                    ),
            )
            .child(div().grid().grid_cols(4).gap(space(2.)).children([
                overview_metric_card(
                    i18n::t("preferences:overview.tiles.total.label"),
                    total_records.to_string(),
                    total_hint,
                    cx,
                ),
                overview_metric_card(
                    i18n::t("preferences:overview.tiles.recent.label"),
                    today_records.to_string(),
                    recent_hint,
                    cx,
                ),
                overview_metric_card(
                    i18n::t("preferences:overview.tiles.reuses.label"),
                    reuse_count.to_string(),
                    i18n::t("preferences:overview.tiles.reuses.hint"),
                    cx,
                ),
                overview_metric_card(
                    i18n::t("preferences:overview.tiles.span.label"),
                    oldest,
                    span_hint,
                    cx,
                ),
            ]))
            .child(overview_trend_card(&overview.history.daily, cx))
            .child(div().grid().grid_cols(2).gap(space(2.)).children([
                overview_detail_card(
                    i18n::t("preferences:overview.categories.title"),
                    category_rows,
                    cx,
                ),
                overview_detail_card(
                    i18n::t("preferences:overview.sources.title"),
                    source_rows,
                    cx,
                ),
            ]))
            .child(overview_groups_card(
                &overview.history.groups,
                &overview.history.totals,
                cx,
            ))
            .into_any_element()
    }

    fn render_app_exclusion(
        &self,
        value: Option<serde_json::Value>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let count = value
            .and_then(|value| value.as_array().map(|items| items.len()))
            .unwrap_or_default();
        let entity = cx.entity().downgrade();
        Button::new(
            "excluded-apps",
            i18n::t_args(
                "preferences:schema.settings.source.excludedApps.count",
                &[("count", &count.to_string())],
            ),
        )
        .ghost()
        .on_click(move |_, window, cx| {
            if let Some(entity) = entity.upgrade() {
                entity.update(cx, |this, cx| this.open_source_apps(window, cx));
            }
        })
        .into_any_element()
    }

    fn render_group_select(
        &self,
        path: Option<&'static str>,
        value: Option<serde_json::Value>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let selected = value
            .as_ref()
            .and_then(serde_json::Value::as_str)
            .unwrap_or("all")
            .to_owned();
        let options = ["all", "preserve", "missingGroup"];
        let next = options
            .iter()
            .position(|option| *option == selected)
            .and_then(|index| options.get((index + 1) % options.len()))
            .copied()
            .unwrap_or("all")
            .to_owned();
        let label = match selected.as_str() {
            "preserve" => {
                i18n::t("preferences:schema.settings.window.selectGroupOnOpen.options.preserve")
            }
            "missingGroup" => {
                i18n::t("preferences:schema.settings.window.selectGroupOnOpen.options.missingGroup")
            }
            _ => i18n::t("preferences:schema.settings.window.selectGroupOnOpen.options.all"),
        };
        let entity = cx.entity().downgrade();
        Button::new("group-select", label)
            .on_click(move |_, _, cx| {
                if let Some(path) = path
                    && let Some(entity) = entity.upgrade()
                {
                    entity.update(cx, |this, cx| this.update(path, json!(next), cx));
                }
            })
            .into_any_element()
    }

    /// 采集类型使用与 1.x 相同的五项多选控件，并一次性提交对象补丁。
    fn render_capture_kinds(
        &self,
        value: Option<serde_json::Value>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let capture = value.unwrap_or_default();
        let entity = cx.entity().downgrade();
        div()
            .flex()
            .flex_wrap()
            .gap(space(2.))
            .children(values::CAPTURE_KINDS.into_iter().map(|kind| {
                let checked = capture
                    .get(kind)
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let label = i18n::t(&format!("preferences:schema.captureKinds.{kind}"));
                let callback_entity = entity.clone();
                Checkbox::new(format!("capture-kind-{kind}"))
                    .label(label.clone())
                    .accessibility_label(label)
                    .checked(checked)
                    .on_change(move |checked, _, cx| {
                        if let Some(entity) = callback_entity.upgrade() {
                            entity.update(cx, |this, cx| {
                                let current = values::to_json(&this.settings);
                                let selected: Vec<&str> = values::CAPTURE_KINDS
                                    .into_iter()
                                    .filter(|candidate| {
                                        if *candidate == kind {
                                            checked
                                        } else {
                                            values::get(
                                                &current,
                                                &format!("clipboard.capture.{candidate}"),
                                            )
                                            .and_then(serde_json::Value::as_bool)
                                            .unwrap_or(false)
                                        }
                                    })
                                    .collect();
                                this.apply_patch(values::capture_kinds_patch(&selected), cx);
                            });
                        }
                    })
            }))
            .into_any_element()
    }

    fn render_retention(&self, cx: &App) -> gpui::AnyElement {
        let Some(value_state) = self.number_inputs.get("history.retention.value") else {
            return div().into_any_element();
        };
        let Some(unit_state) = self.selects.get("history.retention.unit") else {
            return div().into_any_element();
        };
        let keep_forever = unit_state
            .selected_value(cx)
            .is_some_and(|value| value.as_ref() == "forever");
        div()
            .flex()
            .items_center()
            .gap(space(1.))
            .when(!keep_forever, |row| {
                row.child(NumberInput::new(value_state).width(rems(7.)))
            })
            .child(
                Select::new(unit_state)
                    .small()
                    .width(rems(8.))
                    .accessibility_label(i18n::t(
                        "preferences:schema.settings.history.retention.title",
                    )),
            )
            .into_any_element()
    }

    fn refresh_lan_code(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(core) = core_host::core(cx).cloned() else {
            return;
        };
        let entity = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                match core.refresh_lan_pairing_code().await {
                    Ok(state) => {
                        let _ = entity.update(cx, |this, cx| {
                            this.lan_state = Some(state);
                            cx.notify();
                        });
                    }
                    Err(error) => log::warn!("refresh LAN pairing code failed: {error:#}"),
                }
            })
            .detach();
    }

    /// 设备行的配对对话框；附近设备用 id，手动添加时改用地址。
    fn open_lan_pair_dialog(
        &self,
        nearby: Option<LanNearbyView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(core) = core_host::core(cx).cloned() else {
            return;
        };
        let address = TextInput::new(
            i18n::t("preferences:lanSync.pairModal.addressPlaceholder"),
            window,
            cx,
        );
        let code = TextInput::new(i18n::t("preferences:lanSync.pairModal.code"), window, cx);
        let address_content = address.clone();
        let code_content = code.clone();
        let nearby_for_content = nearby.clone();
        let title = nearby.as_ref().map_or_else(
            || i18n::t("preferences:lanSync.pairModal.manualTitle"),
            |device| {
                i18n::t_args(
                    "preferences:lanSync.pairModal.title",
                    &[("name", &device.name)],
                )
            },
        );
        let answer = form_dialog(
            DialogSpec::new(title)
                .ok_text(i18n::t("preferences:lanSync.pairModal.ok"))
                .cancel_text(i18n::t("common:actions.cancel")),
            move |_, _| {
                let mut body = div().flex().flex_col().gap(space(2.));
                if nearby_for_content.is_none() {
                    body = body.child(Input::new(&address_content));
                }
                body.child(Input::new(&code_content)).into_any_element()
            },
            window,
            cx,
        );
        let entity = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                if !answer.await.unwrap_or(false) {
                    return;
                }
                let values = cx
                    .update(|_, cx| (address.value(cx).to_string(), code.value(cx).to_string()))
                    .unwrap_or_default();
                let target = nearby.map_or_else(
                    || PairTarget::Address(values.0.trim().to_owned()),
                    |device| PairTarget::Device(device.id),
                );
                match core.pair_lan_device(target, values.1).await {
                    Ok(name) => {
                        let _ = entity.update_in(cx, |_, window, cx| {
                            toast::show(
                                Toast::success(i18n::t_args(
                                    "preferences:lanSync.pairedByOther",
                                    &[("name", &name)],
                                )),
                                window,
                                cx,
                            );
                        });
                    }
                    Err(error) => {
                        let _ = entity.update_in(cx, |_, window, cx| {
                            toast::show(Toast::error(error.to_string()), window, cx);
                        });
                    }
                }
            })
            .detach();
    }

    fn open_lan_connect_dialog(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(core) = core_host::core(cx).cloned() else {
            return;
        };
        let address = TextInput::new(
            i18n::t("preferences:lanSync.connect.addressPlaceholder"),
            window,
            cx,
        );
        let content = address.clone();
        let answer = form_dialog(
            DialogSpec::new(i18n::t("preferences:lanSync.connect.title"))
                .ok_text(i18n::t("preferences:lanSync.connect.ok"))
                .cancel_text(i18n::t("common:actions.cancel")),
            move |_, _| div().child(Input::new(&content)).into_any_element(),
            window,
            cx,
        );
        let entity = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                if !answer.await.unwrap_or(false) {
                    return;
                }
                let address = cx
                    .update(|_, cx| address.value(cx).to_string())
                    .unwrap_or_default();
                match core.connect_lan_device(address.trim().to_owned()).await {
                    Ok(name) => {
                        let _ = entity.update_in(cx, |_, window, cx| {
                            toast::show(
                                Toast::success(i18n::t_args(
                                    "preferences:lanSync.connect.success",
                                    &[("name", &name)],
                                )),
                                window,
                                cx,
                            );
                        });
                    }
                    Err(error) => {
                        let _ = entity.update_in(cx, |_, window, cx| {
                            toast::show(Toast::error(error.to_string()), window, cx);
                        });
                    }
                }
            })
            .detach();
    }

    fn remove_lan_device(
        &self,
        device: LanDeviceView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(core) = core_host::core(cx).cloned() else {
            return;
        };
        let entity = cx.entity().downgrade();
        let title = i18n::t_args(
            "preferences:lanSync.devices.removeConfirmTitle",
            &[("name", &device.name)],
        );
        let answer = form_dialog(
            DialogSpec::new(title)
                .ok_text(i18n::t("preferences:lanSync.devices.remove"))
                .cancel_text(i18n::t("common:actions.cancel")),
            move |_, _| {
                div()
                    .kp_text(TextSize::Sm)
                    .child(i18n::t("preferences:lanSync.devices.removeConfirmContent"))
                    .into_any_element()
            },
            window,
            cx,
        );
        window
            .spawn(cx, async move |cx| {
                if !answer.await.unwrap_or(false) {
                    return;
                }
                match core.remove_lan_device(device.id).await {
                    Ok(()) => {
                        let _ = entity.update_in(cx, |_, window, cx| {
                            toast::show(
                                Toast::success(i18n::t("preferences:lanSync.devices.removed")),
                                window,
                                cx,
                            );
                        });
                    }
                    Err(error) => {
                        let _ = entity.update_in(cx, |_, window, cx| {
                            toast::show(Toast::error(error.to_string()), window, cx);
                        });
                    }
                }
            })
            .detach();
    }

    fn render_lan_sync(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let entity = cx.entity().downgrade();
        let lan = &self.settings.sync.lan;
        let enabled = Switch::new("lan-sync-enabled")
            .accessibility_label(i18n::t(
                "preferences:schema.settings.sync.lan.enabled.title",
            ))
            .checked(lan.enabled)
            .on_change({
                let entity = entity.clone();
                move |checked, _, cx| {
                    if let Some(entity) = entity.upgrade() {
                        entity.update(cx, |this, cx| {
                            this.update("sync.lan.enabled", json!(checked), cx);
                        });
                    }
                }
            });
        let mut panel = div()
            .flex()
            .flex_col()
            .gap(space(2.))
            .p(space(3.))
            .rounded(theme::radius::MD)
            .bg(theme::tokens(cx).bg_elevated)
            .child(setting_row(
                i18n::t("preferences:schema.settings.sync.lan.enabled.title"),
                i18n::t("preferences:schema.settings.sync.lan.enabled.description"),
                enabled.into_any_element(),
            ))
            .child(setting_row(
                i18n::t("preferences:schema.settings.sync.lan.deviceName.title"),
                i18n::t("preferences:schema.settings.sync.lan.deviceName.description"),
                Input::new(&self.lan_name)
                    .width(rems(16.))
                    .into_any_element(),
            ))
            .child(setting_row(
                i18n::t("preferences:schema.settings.sync.lan.text.title"),
                gpui::SharedString::default(),
                self.render_lan_switch(
                    "sync.lan.text",
                    lan.text,
                    i18n::t("preferences:schema.settings.sync.lan.text.title"),
                    cx,
                ),
            ))
            .child(setting_row(
                i18n::t("preferences:schema.settings.sync.lan.image.title"),
                gpui::SharedString::default(),
                self.render_lan_switch(
                    "sync.lan.image",
                    lan.image,
                    i18n::t("preferences:schema.settings.sync.lan.image.title"),
                    cx,
                ),
            ))
            .child(setting_row(
                i18n::t("preferences:schema.settings.sync.lan.writeClipboard.title"),
                i18n::t("preferences:schema.settings.sync.lan.writeClipboard.description"),
                self.render_lan_switch(
                    "sync.lan.writeClipboard",
                    lan.write_clipboard,
                    i18n::t("preferences:schema.settings.sync.lan.writeClipboard.title"),
                    cx,
                ),
            ))
            .child(setting_row(
                i18n::t("preferences:schema.settings.sync.lan.maxImageMb.title"),
                i18n::t("preferences:schema.settings.sync.lan.maxImageMb.description"),
                NumberInput::new(&self.lan_max_image)
                    .suffix("MB")
                    .width(rems(10.))
                    .accessibility_label(i18n::t(
                        "preferences:schema.settings.sync.lan.maxImageMb.title",
                    ))
                    .into_any_element(),
            ));

        if !lan.enabled {
            return panel
                .child(
                    div()
                        .kp_text(TextSize::Sm)
                        .child(i18n::t("preferences:lanSync.off")),
                )
                .into_any_element();
        }

        let Some(state) = self.lan_state.as_ref() else {
            return panel
                .child(
                    div()
                        .kp_text(TextSize::Sm)
                        .child(i18n::t("preferences:lanSync.starting")),
                )
                .into_any_element();
        };
        if !state.running {
            return panel
                .child(
                    div()
                        .kp_text(TextSize::Sm)
                        .text_color(theme::tokens(cx).error)
                        .child(state.error.clone().unwrap_or_else(|| {
                            i18n::t("preferences:lanSync.starting").to_string()
                        })),
                )
                .into_any_element();
        }

        let port = state.port.unwrap_or(kwikpaste_core::sync::DEFAULT_PORT);
        let addresses = if state.addresses.is_empty() {
            i18n::t("preferences:lanSync.thisDevice.noAddress").to_string()
        } else {
            state
                .addresses
                .iter()
                .map(|address| format_socket_address(address, port))
                .collect::<Vec<_>>()
                .join(" · ")
        };
        let code = state
            .pairing_code
            .as_deref()
            .map_or_else(|| "".to_owned(), |value| value.to_owned());
        let code_label = if code.is_empty() {
            i18n::t("preferences:lanSync.pairingCode.exhausted")
        } else if state.pairing_attempts_left == 1 {
            i18n::t_args("preferences:lanSync.pairingCode.hint", &[("count", "1")])
        } else {
            i18n::t_args(
                "preferences:lanSync.pairingCode.hint",
                &[("count", &state.pairing_attempts_left.to_string())],
            )
        };
        panel = panel
            .child(
                div()
                    .kp_text(TextSize::Sm)
                    .child(format!("{} · {}", state.device_name, addresses)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space(2.))
                    .child(i18n::t("preferences:lanSync.pairingCode.title"))
                    .child(
                        Button::new(
                            "lan-code-toggle",
                            if self.lan_code_hidden {
                                "••••••"
                            } else {
                                &code
                            },
                        )
                        .ghost()
                        .disabled(code.is_empty())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.lan_code_hidden = !this.lan_code_hidden;
                            cx.notify();
                        })),
                    )
                    .child(
                        Button::new(
                            "lan-code-refresh",
                            i18n::t("preferences:lanSync.pairingCode.refresh"),
                        )
                        .ghost()
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.refresh_lan_code(window, cx);
                        })),
                    )
                    .child(code_label),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space(2.))
                    .child(i18n::t("preferences:lanSync.devices.title"))
                    .child(
                        Button::new("lan-connect", i18n::t("preferences:lanSync.connect.manual"))
                            .ghost()
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_lan_connect_dialog(window, cx);
                            })),
                    ),
            );
        if state.devices.is_empty() {
            panel = panel.child(
                div()
                    .kp_text(TextSize::Sm)
                    .child(i18n::t("preferences:lanSync.devices.empty")),
            );
        } else {
            for device in state.devices.iter().cloned() {
                let name = device.name.clone();
                let address = device.address.as_deref().map_or_else(
                    || i18n::t("preferences:lanSync.devices.offline").to_string(),
                    |address| address.to_owned(),
                );
                let entity = entity.clone();
                panel = panel.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space(2.))
                        .child(format!("{} · {}", name, address))
                        .child(if device.online {
                            i18n::t("preferences:lanSync.devices.online")
                        } else {
                            i18n::t("preferences:lanSync.devices.offline")
                        })
                        .child(
                            Button::new(
                                format!("lan-remove-{}", device.id),
                                i18n::t("preferences:lanSync.devices.remove"),
                            )
                            .ghost()
                            .on_click(move |_, window, cx| {
                                if let Some(entity) = entity.upgrade() {
                                    entity.update(cx, |this, cx| {
                                        this.remove_lan_device(device.clone(), window, cx);
                                    });
                                }
                            }),
                        ),
                );
            }
        }
        panel = panel.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(space(2.))
                .child(i18n::t("preferences:lanSync.nearby.title"))
                .child(
                    Button::new(
                        "lan-manual-pair",
                        i18n::t("preferences:lanSync.nearby.manual"),
                    )
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_lan_pair_dialog(None, window, cx);
                    })),
                ),
        );
        if state.nearby.is_empty() {
            panel = panel.child(
                div()
                    .kp_text(TextSize::Sm)
                    .child(i18n::t("preferences:lanSync.nearby.empty")),
            );
        } else {
            for device in state.nearby.iter().cloned() {
                let entity = entity.clone();
                let label = if device.compatible {
                    i18n::t("preferences:lanSync.nearby.pair")
                } else {
                    i18n::t("preferences:lanSync.nearby.incompatible")
                };
                panel = panel.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space(2.))
                        .child(format!(
                            "{} · {} · {}",
                            device.name,
                            device.address,
                            platform_label(device.platform)
                        ))
                        .child(
                            Button::new(format!("lan-pair-{}", device.id), label)
                                .primary()
                                .disabled(!device.compatible)
                                .on_click(move |_, window, cx| {
                                    if let Some(entity) = entity.upgrade() {
                                        entity.update(cx, |this, cx| {
                                            this.open_lan_pair_dialog(
                                                Some(device.clone()),
                                                window,
                                                cx,
                                            );
                                        });
                                    }
                                }),
                        ),
                );
            }
        }
        #[cfg(target_os = "windows")]
        {
            panel = panel.child(
                div()
                    .kp_text(TextSize::Xs)
                    .child(i18n::t("preferences:lanSync.firewallHint")),
            );
        }
        panel.into_any_element()
    }

    fn render_lan_switch(
        &self,
        path: &'static str,
        checked: bool,
        label: gpui::SharedString,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let entity = cx.entity().downgrade();
        Switch::new(path)
            .accessibility_label(label)
            .checked(checked)
            .on_change(move |checked, _, cx| {
                if let Some(entity) = entity.upgrade() {
                    entity.update(cx, |this, cx| this.update(path, json!(checked), cx));
                }
            })
            .into_any_element()
    }

    fn export_backup(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(core) = core_host::core(cx).cloned() else {
            return;
        };
        let mode = SelectState::new(
            vec![
                SelectOption::new(
                    "encrypted",
                    i18n::t("preferences:backup.export.modeEncrypted"),
                ),
                SelectOption::new("plain", i18n::t("preferences:backup.export.modePlain")),
            ],
            Some("encrypted"),
            window,
            cx,
        );
        let password = TextInput::new(i18n::t("preferences:backup.export.password"), window, cx);
        let mode_content = mode.clone();
        let password_content = password.clone();
        let answer = form_dialog(
            DialogSpec::new(i18n::t("preferences:backup.export.title"))
                .ok_text(i18n::t("common:actions.save"))
                .cancel_text(i18n::t("common:actions.cancel")),
            move |_, _cx| {
                div()
                    .flex()
                    .flex_col()
                    .gap(space(2.))
                    .child(
                        Select::new(&mode_content)
                            .width(rems(16.))
                            .accessibility_label(i18n::t("preferences:backup.export.mode")),
                    )
                    .child(Input::new(&password_content))
                    .into_any_element()
            },
            window,
            cx,
        );
        window
            .spawn(cx, async move |cx| {
                if !answer.await.unwrap_or(false) {
                    return;
                }
                let (mode, password) = cx
                    .update(|_, cx| {
                        (
                            mode.selected_value(cx)
                                .unwrap_or_else(|| "encrypted".into()),
                            password.value(cx).to_string(),
                        )
                    })
                    .unwrap_or_default();
                let Ok(prompt) = cx.update(|_, cx| {
                    let directory = core
                        .preference_directory(PreferenceDirectory::Data)
                        .unwrap_or_else(|_| std::env::temp_dir());
                    clipboard::view::pin::prompt_for_new_path(
                        &directory,
                        "KwikPaste-history.kwikpastebak",
                        cx,
                    )
                }) else {
                    log::warn!("could not open backup save dialog");
                    return;
                };
                let Some(path) = prompt.await else {
                    return;
                };
                let export_mode = if mode == "plain" {
                    BackupExportMode::Plain
                } else {
                    BackupExportMode::Encrypted
                };
                match core
                    .export_history_backup(
                        path,
                        backup::ExportHistoryBackupOptions {
                            mode: export_mode,
                            password: (!password.is_empty()).then_some(password),
                        },
                    )
                    .await
                {
                    Ok(result) => log::info!("history backup exported: {}", result.path),
                    Err(error) => log::warn!("history backup export failed: {error:#}"),
                }
            })
            .detach();
    }
    fn import_backup(&self, _window: &mut Window, cx: &mut Context<Self>) {
        let prompt = clipboard::view::pin::prompt_for_paths(
            gpui::PathPromptOptions {
                files: true,
                directories: false,
                multiple: false,
                prompt: None,
            },
            cx,
        );
        let entity = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let Some(path) = prompt.await.and_then(|paths| paths.into_iter().next()) else {
                return;
            };
            let _ = entity.update_in(cx, |_this, window, cx| {
                Preferences::show_import_confirmation(path, window, cx);
            });
        })
        .detach();
    }

    fn show_import_confirmation(path: PathBuf, window: &mut Window, cx: &mut App) {
        let Ok(mode) = backup::inspect_backup_file(&path) else {
            log::warn!("invalid backup file: {}", path.display());
            return;
        };
        let Some(core) = core_host::core(cx).cloned() else {
            return;
        };
        let password = TextInput::new(i18n::t("preferences:backup.import.password"), window, cx);
        let strategy = SelectState::new(
            vec![
                SelectOption::new("merge", i18n::t("preferences:backup.import.strategyMerge")),
                SelectOption::new(
                    "overwrite",
                    i18n::t("preferences:backup.import.strategyOverwrite"),
                ),
            ],
            Some("merge"),
            window,
            cx,
        );
        let password_for_content = password.clone();
        let strategy_for_content = strategy.clone();
        let answer = form_dialog(
            DialogSpec::new(i18n::t("preferences:backup.import.title"))
                .ok_text(i18n::t("preferences:backup.import.ok"))
                .cancel_text(i18n::t("common:actions.cancel")),
            move |_, _cx| {
                div()
                    .flex()
                    .flex_col()
                    .gap(space(2.))
                    .child(div().kp_text(TextSize::Sm).child(match mode {
                        BackupContainerMode::Encrypted => {
                            i18n::t("preferences:backup.import.typeEncrypted")
                        }
                        BackupContainerMode::Plain => {
                            i18n::t("preferences:backup.import.typePlain")
                        }
                    }))
                    .child(Input::new(&password_for_content))
                    .child(
                        Select::new(&strategy_for_content)
                            .width(rems(16.))
                            .accessibility_label(i18n::t("preferences:backup.import.strategy")),
                    )
                    .into_any_element()
            },
            window,
            cx,
        );
        window
            .spawn(cx, async move |cx| {
                if !answer.await.unwrap_or(false) {
                    return;
                }
                let (password, strategy) = cx
                    .update(|_, cx| {
                        (
                            password.value(cx).to_string(),
                            strategy
                                .selected_value(cx)
                                .unwrap_or_else(|| "merge".into()),
                        )
                    })
                    .unwrap_or_default();
                let strategy = if strategy == "overwrite" {
                    BackupImportStrategy::Overwrite
                } else {
                    BackupImportStrategy::Merge
                };
                match core
                    .import_history_backup(
                        backup::ImportHistoryBackupInput {
                            path: path.clone(),
                            password: (!password.is_empty()).then_some(password),
                        },
                        strategy,
                    )
                    .await
                {
                    Ok(result) => log::info!(
                        "history backup imported: {} items, {} skipped, restart_required={}",
                        result.imported_items,
                        result.skipped_items,
                        result.requires_restart
                    ),
                    Err(error) => log::warn!("history backup import failed: {error:#}"),
                }
            })
            .detach();
    }

    fn export_readable(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(core) = core_host::core(cx).cloned() else {
            return;
        };
        let dialog = cx.new(|cx| ReadableExportDialog::new(core.clone(), window, cx));
        let groups_dialog = dialog.downgrade();
        let groups_core = core.clone();
        window
            .spawn(cx, async move |cx| match groups_core.list_groups().await {
                Ok(groups) => {
                    let _ = groups_dialog.update(cx, |dialog, cx| {
                        dialog.groups = groups;
                        dialog.groups_ready = true;
                        cx.notify();
                    });
                }
                Err(error) => log::warn!("load readable export groups failed: {error:#}"),
            })
            .detach();
        let content = dialog.clone();
        let validation_dialog = dialog.downgrade();
        let answer = form_dialog(
            DialogSpec::new(i18n::t("preferences:readableExport.title"))
                .ok_text(i18n::t("preferences:readableExport.export"))
                .cancel_text(i18n::t("common:actions.cancel"))
                .validate(move |window, cx| {
                    let Some(dialog) = validation_dialog.upgrade() else {
                        return false;
                    };
                    let valid = dialog.read(cx).preview.as_ref().is_some_and(|preview| {
                        preview.item_count > 0
                            && (!dialog.read(cx).include_sensitive
                                || dialog.read(cx).sensitive_confirmed)
                    });
                    if !valid {
                        toast::show(
                            Toast::error(i18n::t("preferences:readableExport.previewRequired")),
                            window,
                            cx,
                        );
                    }
                    valid
                }),
            move |_, _| content.clone().into_any_element(),
            window,
            cx,
        );
        window
            .spawn(cx, async move |cx| {
                if !answer.await.unwrap_or(false) {
                    return;
                }
                let (options, fingerprint) = cx
                    .update(|_, cx| {
                        let dialog = dialog.read(cx);
                        (
                            dialog.export_options(cx),
                            dialog
                                .preview
                                .as_ref()
                                .map(|preview| preview.fingerprint.clone()),
                        )
                    })
                    .unwrap_or((
                        ExportOptions {
                            format: ExportFormat::Xlsx,
                            favorites_only: false,
                            group_ids: None,
                            include_ungrouped: true,
                            split_by_group: false,
                            include_sensitive: false,
                        },
                        None,
                    ));
                let Some(fingerprint) = fingerprint else {
                    log::warn!("readable export confirmed without a preview");
                    return;
                };
                let path = if options.split_by_group {
                    let Ok(prompt) = cx.update(|_, cx| {
                        clipboard::view::pin::prompt_for_paths(
                            gpui::PathPromptOptions {
                                files: false,
                                directories: true,
                                multiple: false,
                                prompt: None,
                            },
                            cx,
                        )
                    }) else {
                        log::warn!("could not open readable export directory dialog");
                        return;
                    };
                    prompt.await.and_then(|paths| paths.into_iter().next())
                } else {
                    let Ok(prompt) = cx.update(|_, cx| {
                        let directory = core
                            .preference_directory(PreferenceDirectory::Data)
                            .unwrap_or_else(|_| std::env::temp_dir());
                        let name = if options.format == ExportFormat::Markdown {
                            "KwikPaste-readable.md"
                        } else {
                            "KwikPaste-readable.xlsx"
                        };
                        clipboard::view::pin::prompt_for_new_path(&directory, name, cx)
                    }) else {
                        log::warn!("could not open readable export save dialog");
                        return;
                    };
                    prompt.await
                };
                let Some(path) = path else {
                    return;
                };
                match core.export_readable_data(options, fingerprint, path).await {
                    Ok(result) => log::info!("readable export written: {}", result.path),
                    Err(error) => log::warn!("readable export failed: {error:#}"),
                }
            })
            .detach();
    }

    fn open_group_manager(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(core) = core_host::core(cx).cloned() else {
            return;
        };
        let source: Arc<dyn ClipboardSource> = Arc::new(
            clipboard::source::core_source::CoreSource::new(core.clone()),
        );
        window
            .spawn(cx, async move |cx| {
                let Ok(groups) = source.groups().await else {
                    log::warn!("could not load groups for preferences");
                    return;
                };
                let _ = cx.update(|window, cx| {
                    group_dialogs::manage_groups(source, groups, |_, _| {}, window, cx);
                });
            })
            .detach();
    }

    fn open_source_apps(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(core) = core_host::core(cx).cloned() else {
            return;
        };
        let excluded = self.settings.clipboard.filters.excluded_app_ids.clone();
        let entity = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let apps = match core.list_all_apps().await {
                    Ok(apps) => apps,
                    Err(error) => {
                        log::warn!("could not load source apps: {error:#}");
                        return;
                    }
                };
                let _ = cx.update(|window, cx| {
                    open_source_apps_dialog(apps, excluded, core, entity, window, cx);
                });
            })
            .detach();
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let portable = core_host::core(cx).is_some_and(|core| core.paths().is_portable());
        let tabs = schema::tabs(portable);
        let tokens = theme::tokens(cx);
        let mut nav = div().flex().flex_col().gap(space(0.5));
        let mut group = None;
        for tab in tabs {
            if group != Some(tab.group) && group.is_some() {
                nav = nav.child(div().h(px(1.)).my(space(1.)).bg(tokens.split));
            }
            group = Some(tab.group);
            let selected = tab.id == self.tab;
            let id = tab.id;
            let title = text::tab_title_of(&tab);
            nav = nav.child(
                div()
                    .id(id.key())
                    .role(Role::Tab)
                    .aria_label(title.clone())
                    .flex()
                    .items_center()
                    .gap(space(2.))
                    .w_full()
                    .h(rems(2.25))
                    .px(space(2.5))
                    .rounded(theme::radius::MD)
                    .kp_text(TextSize::Sm)
                    // 选中项是中性灰底（同剪贴板面板的当前项），悬停浅一档。
                    .when(selected, |item| {
                        item.bg(tokens.fill_secondary).text_color(tokens.text)
                    })
                    .when(!selected, |item| {
                        item.text_color(tokens.secondary)
                            .hover(|style| style.bg(tokens.fill_tertiary))
                    })
                    .cursor_pointer()
                    .child(tab.icon.view(
                        rems(1.),
                        if selected {
                            tokens.text
                        } else {
                            tokens.secondary
                        },
                    ))
                    .child(title)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = id;
                        if id == TabId::Overview {
                            this.refresh_storage_overview(cx);
                        }
                        cx.notify();
                    })),
            );
        }
        let usage = self
            .storage_overview
            .as_ref()
            .map(|overview| overview.usage.total_bytes)
            .unwrap_or(0);
        let limit = self.settings.clipboard.history.storage_limit_mb as u64 * 1024 * 1024;
        let ratio = if limit == 0 {
            0.0
        } else {
            (usage as f32 / limit as f32).clamp(0.0, 1.0)
        };
        let storage_card = div()
            .flex()
            .flex_col()
            .gap(space(1.5))
            .rounded(theme::radius::LG)
            .bg(tokens.fill_tertiary)
            .p(space(3.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space(1.))
                    .kp_text(TextSize::Sm)
                    .child(PrefIcon::HardDrive.view(rems(1.), tokens.success))
                    .child(i18n::t("preferences:storage.title")),
            )
            .child(
                div()
                    .kp_text(TextSize::Xs)
                    .text_color(tokens.tertiary)
                    .child(format!(
                        "{} / {}",
                        text::format_bytes(usage),
                        text::format_bytes(limit)
                    )),
            )
            .child(
                div()
                    .h(px(4.))
                    .w_full()
                    .rounded(theme::radius::SM)
                    .bg(tokens.fill_secondary)
                    .child(
                        div()
                            .h_full()
                            .rounded(theme::radius::SM)
                            .w(rems(10. * ratio.max(0.12)))
                            .bg(tokens.success),
                    ),
            );
        div()
            .flex()
            .flex_col()
            .justify_between()
            .h_full()
            .w(rems(14.))
            .p(space(3.))
            .border_r_1()
            .border_color(tokens.split)
            .bg(crate::platform::material::chrome_surface(cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space(3.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space(2.))
                            .px(space(1.))
                            .child(img(ImageSource::Image(logo())).size(rems(2.5)))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .child(div().kp_text(TextSize::Base).child("KwikPaste"))
                                    .child(
                                        div()
                                            .kp_text(TextSize::Xs)
                                            .text_color(tokens.secondary)
                                            .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
                                    ),
                            ),
                    )
                    .child(nav),
            )
            .child(storage_card)
    }

    fn render_setting(&self, setting: &Setting, cx: &mut Context<Self>) -> gpui::AnyElement {
        if !self.search.value(cx).is_empty() {
            let query = self.search.value(cx).to_lowercase();
            let title = text::setting_title(setting);
            if !search_matches(&query, &title, setting.keywords) {
                return div().into_any_element();
            }
        }
        if matches!(setting.control, Control::LanSync) {
            return self.render_lan_sync(cx);
        }
        let title = text::setting_title(setting);
        let description = text::setting_description(setting);
        let path = setting.path;
        let settings_json = values::to_json(&self.settings);
        let value = path.and_then(|path| values::get(&settings_json, path).cloned());
        let control = match setting.control {
            Control::Switch => {
                let configured = value
                    .as_ref()
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false);
                let checked = if setting.id == "control.autoStart" {
                    crate::platform::autostart::autostart_registered().unwrap_or(configured)
                } else {
                    configured
                };
                let id = setting.id;
                let entity = cx.entity().downgrade();
                Switch::new(id)
                    .accessibility_label(title.clone())
                    .checked(checked)
                    .on_change(move |checked, _, cx| {
                        if let Some(path) = path {
                            let _ = entity.update(cx, |this, cx| {
                                this.update(path, json!(checked), cx);
                            });
                        }
                    })
                    .into_any_element()
            }
            Control::Select(_) | Control::Tiles(_) => {
                if let Some(state) = self.selects.get(setting.id) {
                    Select::new(state)
                        .small()
                        .width(rems(12.))
                        .disabled(setting.is_disabled(&self.settings))
                        .accessibility_label(title.clone())
                        .into_any_element()
                } else {
                    div().into_any_element()
                }
            }
            Control::Number { suffix, .. } => {
                if let Some(state) = self.number_inputs.get(setting.id) {
                    NumberInput::new(state)
                        .width(rems(10.))
                        .when_some(suffix, |input, suffix| {
                            input.suffix(text::number_suffix(suffix))
                        })
                        .disabled(setting.is_disabled(&self.settings))
                        .accessibility_label(title.clone())
                        .into_any_element()
                } else {
                    div().into_any_element()
                }
            }
            Control::ShortcutRecorder => {
                let current = value
                    .as_ref()
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let recording = self.recording == Some(setting.id);
                let id = setting.id;
                shortcut_recorder_button(id, current, &self.settings, recording)
                    .accessibility_label(title.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.begin_recording(id, window, cx);
                    }))
                    .into_any_element()
            }
            Control::Permission(PermissionKind::RunAsAdministrator) => {
                let status = crate::platform::autostart::admin_status(cx);
                let label = if status.running_as_admin || status.task_ready {
                    i18n::t("preferences:schema.settings.permissions.runAsAdministrator.title")
                } else {
                    i18n::t("common:actions.open")
                };
                Button::new("restart-as-admin", label)
                    .accessibility_label(title.clone())
                    .on_click(move |_, _, cx| {
                        if let Err(error) = crate::platform::autostart::restart_as_admin(cx) {
                            log::warn!("administrator restart was not started: {error:#}");
                        }
                    })
                    .into_any_element()
            }
            Control::StorageOverview => self.render_storage_overview(cx),
            Control::CaptureKinds => self.render_capture_kinds(value, cx),
            Control::CaptureOrder => self.render_capture_order(cx),
            Control::Retention => self.render_retention(cx),
            Control::RetentionRules => self.render_retention_rules(cx),
            Control::AppExclusion => self.render_app_exclusion(value, cx),
            Control::GroupSelect => self.render_group_select(path, value, cx),
            Control::Action { .. } | Control::CleanupStatus => self.render_action(setting.id, cx),
            _ => self.render_action(setting.id, cx),
        };
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(space(3.))
            .when(setting.control.full_width(), |row| {
                row.flex_col().items_start()
            })
            .px(space(1.))
            .py(space(2.5))
            .min_h(rems(3.))
            .when(setting.parent.is_some(), |row| row.pl(space(5.)))
            .border_b_1()
            .border_color(theme::tokens(cx).split)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space(0.5))
                    .flex_1()
                    .min_w_0()
                    .child(div().kp_text(TextSize::Sm).child(title))
                    .when(!description.is_empty(), |row| {
                        row.child(
                            div()
                                .kp_text(TextSize::Xs)
                                .text_color(theme::tokens(cx).tertiary)
                                .child(description),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_none()
                    .justify_end()
                    .when(setting.control.full_width(), |wrapper| {
                        wrapper.w_full().justify_start()
                    })
                    .child(control),
            )
            .into_any_element()
    }

    fn render_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let portable = core_host::core(cx).is_some_and(|core| core.paths().is_portable());
        let tabs = schema::tabs(portable);
        let tab = tabs.into_iter().find(|tab| tab.id == self.tab);
        let sections = tab.map(|tab| tab.sections).unwrap_or_default();
        let show_titles = sections.len() > 1;
        let tokens = theme::tokens(cx);
        let content = ScrollArea::new("preferences-scroll", &self.scroll)
            .flex()
            .flex_col()
            .px(space(7.))
            .py(space(6.))
            .gap(space(7.))
            .children(sections.into_iter().map(|section| {
                if self.tab == TabId::Overview {
                    return self.render_storage_overview(cx);
                }
                // 分组不再套卡片框：标题下面直接是扁平的设置行，行间细分隔线。
                let card = div()
                    .flex()
                    .flex_col()
                    .border_t_1()
                    .border_color(tokens.split)
                    .children(
                        section
                            .settings
                            .iter()
                            .filter(|setting| !setting.is_collapsed(&self.settings))
                            .map(|setting| self.render_setting(setting, cx)),
                    );
                div()
                    .flex()
                    .flex_col()
                    .gap(space(2.))
                    .when(show_titles, |section_view| {
                        section_view.child(
                            div()
                                .px(space(1.))
                                .kp_text(TextSize::Xs)
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_color(tokens.tertiary)
                                .child(text::section_title(&section)),
                        )
                    })
                    .child(card)
                    .into_any_element()
            }));
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .bg(crate::platform::material::shell_surface(
                cx,
                tokens.bg_container,
            ))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .h(rems(4.))
                    .px(space(7.))
                    .border_b_1()
                    .border_color(tokens.split)
                    .child(
                        div()
                            .kp_text(TextSize::Lg)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(text::tab_title(self.tab)),
                    )
                    .child(Input::search(&self.search).small().width(rems(16.))),
            )
            .child(div().flex_1().min_h_0().overflow_hidden().child(content))
    }
}

struct ReadableExportDialog {
    core: kwikpaste_core::Core,
    format: SelectState,
    _subscriptions: Vec<Subscription>,
    favorites_only: bool,
    include_sensitive: bool,
    sensitive_confirmed: bool,
    groups: Vec<kwikpaste_core::db::models::ClipboardGroup>,
    groups_ready: bool,
    group_ids: Option<Vec<String>>,
    include_ungrouped: bool,
    split_by_group: bool,
    preview: Option<ExportPreview>,
    preview_failed: bool,
}

impl ReadableExportDialog {
    fn new(core: kwikpaste_core::Core, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let format = SelectState::new(
            vec![
                SelectOption::new("xlsx", i18n::t("preferences:readableExport.formatXlsx")),
                SelectOption::new(
                    "markdown",
                    i18n::t("preferences:readableExport.formatMarkdown"),
                ),
            ],
            Some("xlsx"),
            window,
            cx,
        );
        let format_subscription = format.on_change(cx, |this, _, cx| {
            this.invalidate_preview(cx);
        });
        Self {
            core,
            format,
            _subscriptions: vec![format_subscription],
            favorites_only: false,
            include_sensitive: false,
            sensitive_confirmed: false,
            groups: Vec::new(),
            groups_ready: false,
            group_ids: None,
            include_ungrouped: true,
            split_by_group: false,
            preview: None,
            preview_failed: false,
        }
    }

    fn export_options(&self, cx: &App) -> ExportOptions {
        ExportOptions {
            format: if self
                .format
                .selected_value(cx)
                .is_some_and(|format| format == "markdown")
            {
                ExportFormat::Markdown
            } else {
                ExportFormat::Xlsx
            },
            favorites_only: self.favorites_only,
            group_ids: self.group_ids.clone(),
            include_ungrouped: self.include_ungrouped,
            split_by_group: self.split_by_group,
            include_sensitive: self.include_sensitive,
        }
    }

    fn invalidate_preview(&mut self, cx: &mut Context<Self>) {
        self.preview = None;
        self.preview_failed = false;
        cx.notify();
    }

    fn prepare_preview(&mut self, cx: &mut Context<Self>) {
        let options = self.export_options(cx);
        let core = self.core.clone();
        self.preview = None;
        self.preview_failed = false;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = core.preview_readable_export(options).await;
            let _ = this.update(cx, |dialog, cx| {
                match result {
                    Ok(preview) => dialog.preview = Some(preview),
                    Err(error) => {
                        dialog.preview_failed = true;
                        log::warn!("readable export preview failed: {error:#}");
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for ReadableExportDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().downgrade();
        let selected_groups = self.group_ids.clone();
        let group_rows = selected_groups.map(|selected| {
            let rows = self.groups.iter().map(|group| {
                let id = group.id.clone();
                let checked = selected.contains(&id);
                let entity = entity.clone();
                Checkbox::new(format!("readable-group-{id}"))
                    .label(group.name.clone())
                    .checked(checked)
                    .disabled(!self.groups_ready)
                    .on_change(move |checked, _, cx| {
                        if let Some(entity) = entity.upgrade() {
                            entity.update(cx, |dialog, cx| {
                                let ids = dialog.group_ids.get_or_insert_with(Vec::new);
                                if checked {
                                    if !ids.contains(&id) {
                                        ids.push(id.clone());
                                    }
                                } else {
                                    ids.retain(|value| value != &id);
                                }
                                dialog.invalidate_preview(cx);
                            });
                        }
                    })
            });
            div().flex().flex_col().gap(space(1.)).children(rows).child(
                Checkbox::new("readable-ungrouped")
                    .label(i18n::t("preferences:readableExport.ungrouped"))
                    .checked(self.include_ungrouped)
                    .on_change({
                        let entity = entity.clone();
                        move |checked, _, cx| {
                            if let Some(entity) = entity.upgrade() {
                                entity.update(cx, |dialog, cx| {
                                    dialog.include_ungrouped = checked;
                                    dialog.invalidate_preview(cx);
                                });
                            }
                        }
                    }),
            )
        });
        let preview_summary = self.preview.as_ref().map(|preview| {
            let summary = i18n::t_args(
                "preferences:readableExport.summary",
                &[
                    ("groups", &preview.groups.len().to_string()),
                    ("items", &preview.item_count.to_string()),
                    ("files", &preview.file_count.to_string()),
                ],
            );
            div()
                .flex()
                .flex_col()
                .gap(space(1.))
                .child(summary)
                .child(i18n::t_args(
                    "preferences:readableExport.excluded",
                    &[("count", &preview.excluded_sensitive.to_string())],
                ))
                .children(
                    preview
                        .groups
                        .iter()
                        .map(|group| format!("{} · {}", group.name, group.count)),
                )
                .when(preview.item_count == 0, |element| {
                    element.child(i18n::t("preferences:readableExport.empty"))
                })
        });
        div()
            .flex()
            .flex_col()
            .gap(space(2.))
            .child(
                Select::new(&self.format)
                    .width(rems(16.))
                    .accessibility_label(i18n::t("preferences:readableExport.format")),
            )
            .child(
                Button::new(
                    "readable-groups-mode",
                    if self.group_ids.is_some() {
                        i18n::t("preferences:readableExport.selectedGroups")
                    } else {
                        i18n::t("preferences:readableExport.allGroups")
                    },
                )
                .ghost()
                .on_click(cx.listener(|dialog, _, _, cx| {
                    dialog.group_ids = if dialog.group_ids.is_some() {
                        None
                    } else {
                        Some(Vec::new())
                    };
                    dialog.invalidate_preview(cx);
                })),
            )
            .when_some(group_rows, |element, rows| element.child(rows))
            .child(
                Checkbox::new("readable-split")
                    .label(i18n::t("preferences:readableExport.split"))
                    .checked(self.split_by_group)
                    .on_change({
                        let entity = entity.clone();
                        move |checked, _, cx| {
                            if let Some(entity) = entity.upgrade() {
                                entity.update(cx, |dialog, cx| {
                                    dialog.split_by_group = checked;
                                    dialog.invalidate_preview(cx);
                                });
                            }
                        }
                    }),
            )
            .child(
                Checkbox::new("readable-favorites")
                    .label(i18n::t("preferences:readableExport.favorites"))
                    .checked(self.favorites_only)
                    .on_change({
                        let entity = entity.clone();
                        move |checked, _, cx| {
                            if let Some(entity) = entity.upgrade() {
                                entity.update(cx, |dialog, cx| {
                                    dialog.favorites_only = checked;
                                    dialog.invalidate_preview(cx);
                                });
                            }
                        }
                    }),
            )
            .child(
                Checkbox::new("readable-sensitive")
                    .label(i18n::t("preferences:readableExport.includeSensitive"))
                    .checked(self.include_sensitive)
                    .on_change({
                        let entity = entity.clone();
                        move |checked, _, cx| {
                            if let Some(entity) = entity.upgrade() {
                                entity.update(cx, |dialog, cx| {
                                    dialog.include_sensitive = checked;
                                    dialog.sensitive_confirmed = false;
                                    dialog.invalidate_preview(cx);
                                });
                            }
                        }
                    }),
            )
            .when(self.include_sensitive, |element| {
                let entity = entity.clone();
                element.child(
                    Checkbox::new("readable-sensitive-confirm")
                        .label(i18n::t("preferences:readableExport.confirmSensitive"))
                        .checked(self.sensitive_confirmed)
                        .on_change(move |checked, _, cx| {
                            if let Some(entity) = entity.upgrade() {
                                entity.update(cx, |dialog, cx| {
                                    dialog.sensitive_confirmed = checked;
                                    cx.notify();
                                });
                            }
                        }),
                )
            })
            .child(
                Button::new(
                    "readable-preview",
                    i18n::t("preferences:readableExport.preview"),
                )
                .primary()
                .on_click(cx.listener(|dialog, _, _, cx| {
                    dialog.prepare_preview(cx);
                })),
            )
            .when(self.preview_failed, |element| {
                element.child(i18n::t("preferences:readableExport.retryPreview"))
            })
            .when_some(preview_summary, |element, summary| element.child(summary))
    }
}

struct SourceAppsDialog {
    apps: Vec<kwikpaste_core::ops::ClipboardAppView>,
    selected: HashSet<String>,
}

impl Render for SourceAppsDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.apps.iter().map(|app| {
            let id = app.id.clone();
            let checked = self.selected.contains(&id);
            let label = if app.name.is_empty() {
                id.clone()
            } else {
                format!("{} ({id})", app.name)
            };
            let entity = cx.entity().downgrade();
            Checkbox::new(format!("excluded-app-{id}"))
                .label(label)
                .checked(checked)
                .on_change(move |checked, _, cx| {
                    if let Some(entity) = entity.upgrade() {
                        entity.update(cx, |this, cx| {
                            if checked {
                                this.selected.insert(id.clone());
                            } else {
                                this.selected.remove(&id);
                            }
                            cx.notify();
                        });
                    }
                })
        });
        div().flex().flex_col().gap(space(1.)).children(rows)
    }
}

fn open_source_apps_dialog(
    apps: Vec<kwikpaste_core::ops::ClipboardAppView>,
    excluded: Vec<String>,
    core: kwikpaste_core::Core,
    parent: WeakEntity<Preferences>,
    window: &mut Window,
    cx: &mut App,
) {
    let dialog = cx.new(|_| SourceAppsDialog {
        apps,
        selected: excluded.into_iter().collect(),
    });
    let content = dialog.clone();
    let adder = dialog.downgrade();
    let answer = form_dialog(
        DialogSpec::new(i18n::t(
            "preferences:schema.settings.source.excludedApps.title",
        ))
        .ok_text(i18n::t("common:actions.save"))
        .cancel_text(i18n::t("common:actions.cancel"))
        .footer_extra(move |_, _cx| {
            let adder = adder.clone();
            let core = core.clone();
            Button::new(
                "source-app-add",
                i18n::t("preferences:schema.settings.source.appTransfer.addApp"),
            )
            .ghost()
            .on_click(move |_, _, cx| {
                let prompt = clipboard::view::pin::prompt_for_paths(
                    gpui::PathPromptOptions {
                        files: true,
                        directories: false,
                        multiple: false,
                        prompt: None,
                    },
                    cx,
                );
                let adder = adder.clone();
                let core = core.clone();
                cx.spawn(async move |cx| {
                    let Some(path) = prompt.await.and_then(|paths| paths.into_iter().next()) else {
                        return;
                    };
                    match core.add_app_from_path(path).await {
                        Ok(app) => {
                            let _ = adder.update(cx, |dialog, cx| {
                                if !dialog.apps.iter().any(|known| known.id == app.id) {
                                    dialog.apps.push(app);
                                }
                                cx.notify();
                            });
                        }
                        Err(error) => log::warn!("could not add source app: {error:#}"),
                    }
                })
                .detach();
            })
            .into_any_element()
        }),
        move |_, _| content.clone().into_any_element(),
        window,
        cx,
    );
    window
        .spawn(cx, async move |cx| {
            if !answer.await.unwrap_or(false) {
                return;
            }
            let selected = cx
                .update(|_, cx| dialog.read(cx).selected.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            let _ = parent.update(cx, |preferences, cx| {
                preferences.update("clipboard.filters.excludedAppIds", json!(selected), cx);
            });
        })
        .detach();
}

fn format_storage_row(name: gpui::SharedString, bytes: u64) -> gpui::AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .kp_text(TextSize::Sm)
        .child(name.to_owned())
        .child(format_bytes(bytes))
        .into_any_element()
}

#[derive(Clone, Copy)]
struct CaptureOrderDrag {
    kind: CaptureKind,
}

impl Render for CaptureOrderDrag {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx);
        div()
            .flex()
            .items_center()
            .gap(space(2.))
            .p(space(2.))
            .rounded(theme::radius::SM)
            .border_1()
            .border_color(tokens.primary)
            .bg(tokens.bg_elevated)
            .kp_text(TextSize::Sm)
            .child(capture_kind_icon(self.kind).view(rems(1.), tokens.primary))
            .child(capture_kind_label(self.kind))
    }
}

/// 移动而不是交换两项，保证被跨过的格式保持原来的相对顺序。
fn reorder_capture_kinds(order: &mut Vec<CaptureKind>, kind: CaptureKind, target: usize) -> bool {
    let Some(index) = order.iter().position(|candidate| *candidate == kind) else {
        return false;
    };
    if target >= order.len() || index == target {
        return false;
    }
    let kind = order.remove(index);
    order.insert(target, kind);
    true
}

struct RetentionRuleEditor {
    categories: Vec<ContentCategory>,
    min_size: NumberInputState,
    sensitive_only: bool,
    unused_only: bool,
    keep_value: NumberInputState,
    keep_unit: SelectState,
}

impl RetentionRuleEditor {
    fn new(rule: &RetentionRule, window: &mut Window, cx: &mut App) -> Self {
        let keep_value = NumberInputState::new(
            u64::from(rule.keep.value),
            0,
            u64::from(u32::MAX),
            window,
            cx,
        );
        let keep_unit = SelectState::new(
            ["minutes", "hours", "days", "weeks", "months", "forever"]
                .into_iter()
                .map(|key| {
                    SelectOption::new(
                        key,
                        i18n::t(&format!("preferences:schema.retentionUnits.{key}")),
                    )
                })
                .collect(),
            Some(retention_unit_key(rule.keep.unit)),
            window,
            cx,
        );
        Self {
            categories: rule.categories.clone(),
            min_size: NumberInputState::new(
                u64::from(rule.min_size_kb),
                0,
                u64::from(u32::MAX),
                window,
                cx,
            ),
            sensitive_only: rule.sensitive_only,
            unused_only: rule.unused_only,
            keep_value,
            keep_unit,
        }
    }

    fn to_rule(&self, base: &RetentionRule, cx: &App) -> RetentionRule {
        let mut rule = base.clone();
        rule.categories = self.categories.clone();
        rule.min_size_kb = self.min_size.clamped(cx) as u32;
        rule.sensitive_only = self.sensitive_only;
        rule.unused_only = self.unused_only;
        rule.keep.value = self.keep_value.clamped(cx) as u32;
        rule.keep.unit = self
            .keep_unit
            .selected_value(cx)
            .map(|value| retention_unit_from_key(value.as_ref()))
            .unwrap_or(RetentionUnit::Forever);
        rule
    }
}

impl Render for RetentionRuleEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx);
        let entity = cx.entity().downgrade();
        let keep_forever = self
            .keep_unit
            .selected_value(cx)
            .is_some_and(|value| value.as_ref() == "forever");
        let categories = ContentCategory::ALL.into_iter().map(|category| {
            let checked = self.categories.contains(&category);
            let entity = entity.clone();
            Checkbox::new(format!("retention-category-{category:?}"))
                .label(category_label(category))
                .checked(checked)
                .on_change(move |checked, _, cx| {
                    if let Some(entity) = entity.upgrade() {
                        entity.update(cx, |editor, cx| {
                            if checked {
                                if !editor.categories.contains(&category) {
                                    editor.categories.push(category);
                                }
                            } else {
                                editor.categories.retain(|item| *item != category);
                            }
                            cx.notify();
                        });
                    }
                })
        });
        let condition_entity = cx.entity().downgrade();
        let sensitive = Checkbox::new("retention-sensitive")
            .label(i18n::t("preferences:retentionRules.form.sensitiveOnly"))
            .checked(self.sensitive_only)
            .on_change(move |checked, _, cx| {
                if let Some(entity) = condition_entity.upgrade() {
                    entity.update(cx, |editor, cx| {
                        editor.sensitive_only = checked;
                        cx.notify();
                    });
                }
            });
        let unused_entity = cx.entity().downgrade();
        let unused = Checkbox::new("retention-unused")
            .label(i18n::t("preferences:retentionRules.form.unusedOnly"))
            .checked(self.unused_only)
            .on_change(move |checked, _, cx| {
                if let Some(entity) = unused_entity.upgrade() {
                    entity.update(cx, |editor, cx| {
                        editor.unused_only = checked;
                        cx.notify();
                    });
                }
            });
        div()
            .flex()
            .flex_col()
            .gap(space(2.))
            .child(
                div()
                    .kp_text(TextSize::Xs)
                    .text_color(tokens.secondary)
                    .child(i18n::t("preferences:retentionRules.form.categories")),
            )
            .child(div().flex().flex_wrap().gap(space(1.)).children(categories))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(space(2.))
                    .child(
                        div()
                            .kp_text(TextSize::Xs)
                            .text_color(tokens.secondary)
                            .child(i18n::t("preferences:retentionRules.form.minSize")),
                    )
                    .child(
                        NumberInput::new(&self.min_size)
                            .width(rems(12.))
                            .suffix("KB"),
                    ),
            )
            .child(
                div()
                    .kp_text(TextSize::Xs)
                    .text_color(tokens.secondary)
                    .child(i18n::t("preferences:retentionRules.form.conditions")),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space(1.))
                    .child(sensitive)
                    .child(unused),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(space(2.))
                    .child(
                        div()
                            .kp_text(TextSize::Xs)
                            .text_color(tokens.secondary)
                            .child(i18n::t("preferences:retentionRules.form.keep")),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space(1.))
                            .when(!keep_forever, |row| {
                                row.child(NumberInput::new(&self.keep_value).width(rems(9.)))
                            })
                            .child(Select::new(&self.keep_unit).width(rems(11.))),
                    ),
            )
    }
}

fn capture_kind_label(kind: CaptureKind) -> gpui::SharedString {
    let key = match kind {
        CaptureKind::Text => "text",
        CaptureKind::Html => "html",
        CaptureKind::Rtf => "rtf",
        CaptureKind::Image => "image",
        CaptureKind::Files => "files",
    };
    i18n::t(&format!("preferences:schema.captureKinds.{key}"))
}

fn capture_kind_icon(kind: CaptureKind) -> PrefIcon {
    match kind {
        CaptureKind::Text => PrefIcon::ClipboardType,
        CaptureKind::Html => PrefIcon::FileCode,
        CaptureKind::Rtf => PrefIcon::FileType,
        CaptureKind::Image => PrefIcon::FileImage,
        CaptureKind::Files => PrefIcon::Files,
    }
}

fn retention_label(value: u32, unit: RetentionUnit) -> gpui::SharedString {
    if matches!(unit, RetentionUnit::Forever) || value == 0 {
        return i18n::t("preferences:retentionRules.summary.keepForever");
    }
    let key = match unit {
        RetentionUnit::Minutes => "minutes",
        RetentionUnit::Hours => "hours",
        RetentionUnit::Days => "days",
        RetentionUnit::Weeks => "weeks",
        RetentionUnit::Months => "months",
        RetentionUnit::Forever => "forever",
    };
    i18n::t_args(
        &format!("preferences:retentionRules.durations.{key}"),
        &[("count", &value.to_string())],
    )
}

fn retention_unit_key(unit: RetentionUnit) -> &'static str {
    match unit {
        RetentionUnit::Minutes => "minutes",
        RetentionUnit::Hours => "hours",
        RetentionUnit::Days => "days",
        RetentionUnit::Weeks => "weeks",
        RetentionUnit::Months => "months",
        RetentionUnit::Forever => "forever",
    }
}

fn retention_unit_from_key(key: &str) -> RetentionUnit {
    match key {
        "minutes" => RetentionUnit::Minutes,
        "hours" => RetentionUnit::Hours,
        "days" => RetentionUnit::Days,
        "weeks" => RetentionUnit::Weeks,
        "months" => RetentionUnit::Months,
        _ => RetentionUnit::Forever,
    }
}

fn retention_rule_summary(rule: &RetentionRule) -> gpui::SharedString {
    let mut parts = Vec::new();
    if rule.categories.is_empty() {
        parts.push(i18n::t("preferences:retentionRules.summary.all").to_string());
    } else {
        parts.push(
            rule.categories
                .iter()
                .map(|category| category_label(*category).to_string())
                .collect::<Vec<_>>()
                .join("、"),
        );
    }
    if rule.min_size_kb > 0 {
        parts.push(
            i18n::t_args(
                "preferences:retentionRules.summary.larger",
                &[("size", &format!("{} KB", rule.min_size_kb))],
            )
            .to_string(),
        );
    }
    if rule.sensitive_only {
        parts.push(i18n::t("preferences:retentionRules.summary.sensitive").to_string());
    }
    if rule.unused_only {
        parts.push(i18n::t("preferences:retentionRules.summary.unused").to_string());
    }
    parts.join(" · ").into()
}

fn setting_row(
    title: gpui::SharedString,
    description: gpui::SharedString,
    control: gpui::AnyElement,
) -> gpui::AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(space(3.))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(space(1.))
                .flex_1()
                .child(div().kp_text(TextSize::Sm).child(title))
                .when(!description.is_empty(), |row| {
                    row.child(div().kp_text(TextSize::Xs).child(description))
                }),
        )
        .child(control)
        .into_any_element()
}

fn category_label(category: ContentCategory) -> gpui::SharedString {
    let key = match category {
        ContentCategory::Text => "text",
        ContentCategory::Html => "html",
        ContentCategory::Rtf => "rtf",
        ContentCategory::Url => "url",
        ContentCategory::Email => "email",
        ContentCategory::Color => "color",
        ContentCategory::Path => "path",
        ContentCategory::Image => "image",
        ContentCategory::Files => "files",
    };
    i18n::t(&format!("preferences:overview.categories.names.{key}"))
}

fn format_socket_address(address: &str, port: u16) -> String {
    if address.starts_with('[') || !address.contains(':') {
        format!("{address}:{port}")
    } else {
        format!("[{address}]:{port}")
    }
}

fn platform_label(platform: kwikpaste_core::db::models::Platform) -> &'static str {
    match platform {
        kwikpaste_core::db::models::Platform::Windows => "Windows",
        kwikpaste_core::db::models::Platform::Macos => "macOS",
    }
}

fn format_bytes(bytes: u64) -> String {
    const KB: f64 = 1024.;
    const MB: f64 = KB * 1024.;
    const GB: f64 = MB * 1024.;
    let value = bytes as f64;
    if value >= GB {
        format!("{:.1} GB", value / GB)
    } else if value >= MB {
        format!("{:.1} MB", value / MB)
    } else if value >= KB {
        format!("{:.1} KB", value / KB)
    } else {
        format!("{bytes} B")
    }
}

fn overview_metric_card(
    title: gpui::SharedString,
    value: String,
    hint: gpui::SharedString,
    cx: &mut Context<Preferences>,
) -> gpui::AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(space(1.))
        .rounded(theme::radius::LG)
        .bg(theme::tokens(cx).fill_tertiary)
        .p(space(3.))
        .child(
            div()
                .kp_text(TextSize::Xs)
                .text_color(theme::tokens(cx).secondary)
                .child(title),
        )
        .child(div().kp_text(TextSize::Lg).child(value))
        .child(
            div()
                .kp_text(TextSize::Xs)
                .text_color(theme::tokens(cx).secondary)
                .child(hint),
        )
        .into_any_element()
}

fn overview_detail_card<I>(
    title: gpui::SharedString,
    rows: I,
    cx: &mut Context<Preferences>,
) -> gpui::AnyElement
where
    I: IntoIterator<Item = gpui::AnyElement>,
{
    let tokens = theme::tokens(cx);
    div()
        .flex()
        .flex_col()
        .gap(space(2.))
        .rounded(theme::radius::LG)
        .bg(tokens.fill_tertiary)
        .p(space(3.))
        .child(
            div()
                .kp_text(TextSize::Sm)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(title),
        )
        .children(rows)
        .into_any_element()
}

fn overview_trend_card(
    daily: &[kwikpaste_core::db::overview::DailyCount],
    cx: &mut Context<Preferences>,
) -> gpui::AnyElement {
    let tokens = theme::tokens(cx);
    let max_count = daily.iter().map(|day| day.count).max().unwrap_or(1).max(1);
    let recent = daily.iter().rev().take(7).collect::<Vec<_>>();
    let bars = recent.into_iter().rev().map(|day| {
        let ratio = day.count as f32 / max_count as f32;
        div()
            .flex()
            .flex_col()
            .items_center()
            .justify_end()
            .gap(space(0.5))
            .flex_1()
            .child(
                div()
                    .w(rems(1.25))
                    .h(rems((ratio * 5.0).max(0.25)))
                    .rounded(theme::radius::SM)
                    .bg(tokens.primary),
            )
            .child(
                div()
                    .kp_text(TextSize::Xs)
                    .text_color(tokens.secondary)
                    .child(day.date.format("%m/%d").to_string()),
            )
            .into_any_element()
    });
    div()
        .flex()
        .flex_col()
        .gap(space(2.))
        .rounded(theme::radius::LG)
        .bg(tokens.fill_tertiary)
        .p(space(3.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(space(1.))
                .kp_text(TextSize::Sm)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(i18n::t("preferences:overview.trend.title"))
                .child(
                    div()
                        .kp_text(TextSize::Xs)
                        .text_color(tokens.secondary)
                        .child(i18n::t_args(
                            "preferences:overview.trend.subtitle",
                            &[("count", "30")],
                        )),
                ),
        )
        .child(
            div()
                .flex()
                .items_end()
                .gap(space(2.))
                .h(rems(8.))
                .children(bars),
        )
        .into_any_element()
}

fn overview_groups_card(
    groups: &[kwikpaste_core::db::overview::GroupStat],
    totals: &kwikpaste_core::db::overview::ItemTotals,
    cx: &mut Context<Preferences>,
) -> gpui::AnyElement {
    let tokens = theme::tokens(cx);
    let rows = [
        (
            i18n::t("preferences:overview.organize.favorites"),
            totals.favorites,
        ),
        (
            i18n::t("preferences:overview.organize.pinned"),
            totals.pinned,
        ),
        (i18n::t("preferences:overview.organize.noted"), totals.noted),
        (
            i18n::t("preferences:overview.organize.sensitive"),
            totals.sensitive,
        ),
        (
            i18n::t("preferences:overview.organize.ungrouped"),
            totals.total.saturating_sub(totals.grouped),
        ),
    ];
    let marks = rows.into_iter().map(|(label, count)| {
        div()
            .flex()
            .items_center()
            .gap(space(2.))
            .child(div().kp_text(TextSize::Sm).child(label))
            .child(
                div()
                    .flex_1()
                    .h(px(4.))
                    .rounded(theme::radius::SM)
                    .bg(tokens.fill_secondary)
                    .child(
                        div()
                            .h_full()
                            .rounded(theme::radius::SM)
                            .w(rems(
                                (count as f32 / totals.total.max(1) as f32 * 12.0).max(0.2),
                            ))
                            .bg(tokens.primary),
                    ),
            )
            .child(div().kp_text(TextSize::Xs).child(count.to_string()))
            .into_any_element()
    });
    let group_rows = groups.iter().take(5).map(|group| {
        div()
            .flex()
            .items_center()
            .justify_between()
            .kp_text(TextSize::Sm)
            .child(group.name.clone())
            .child(group.count.to_string())
            .into_any_element()
    });
    div()
        .flex()
        .flex_col()
        .gap(space(2.))
        .rounded(theme::radius::LG)
        .bg(tokens.fill_tertiary)
        .p(space(3.))
        .child(
            div()
                .kp_text(TextSize::Sm)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(i18n::t("preferences:overview.organize.title")),
        )
        .child(
            div().grid().grid_cols(2).gap(space(4.)).children([
                div()
                    .flex()
                    .flex_col()
                    .gap(space(1.))
                    .children(group_rows)
                    .into_any_element(),
                div()
                    .flex()
                    .flex_col()
                    .gap(space(1.))
                    .children(marks)
                    .into_any_element(),
            ]),
        )
        .into_any_element()
}

impl Render for Preferences {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.capture_shortcut(event, window, cx);
            }))
            .flex()
            .overflow_hidden()
            .bg(theme::tokens(cx).bg_layout)
            .text_color(theme::tokens(cx).text)
            .child(self.render_sidebar(cx))
            .child(self.render_page(cx))
    }
}

pub(crate) fn shortcut_conflicts(left: &str, right: &str) -> bool {
    fn normalized(value: &str) -> Vec<String> {
        let mut keys: Vec<String> = value
            .split('+')
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(|key| key.to_ascii_lowercase())
            .collect();
        keys.sort_unstable();
        keys
    }

    !left.trim().is_empty() && normalized(left) == normalized(right)
}

pub(crate) fn shortcut_conflicts_with_settings(
    setting_id: &str,
    candidate: &str,
    settings: &Settings,
) -> bool {
    if candidate.trim().is_empty() {
        return false;
    }
    let other = match setting_id {
        "shortcuts.openClipboard" => Some(settings.shortcuts.open_preference.as_str()),
        "shortcuts.openPreference" => Some(settings.shortcuts.open_clipboard.as_str()),
        _ => None,
    };
    if other.is_some_and(|other| shortcut_conflicts(candidate, other)) {
        return true;
    }
    settings.shortcuts.quick_paste.enabled
        && (0..10).any(|index| {
            let key = if index == 9 {
                "0".to_owned()
            } else {
                (index + 1).to_string()
            };
            let quick = format!(
                "{}+{key}",
                settings.shortcuts.quick_paste.modifiers.accelerator()
            );
            shortcut_conflicts(candidate, &quick)
        })
}

pub(crate) fn backup_confirmation_required(_: BackupContainerMode) -> bool {
    true
}

pub(crate) fn shortcut_from_keystroke(keystroke: &Keystroke) -> Option<String> {
    let mut modifiers = Vec::new();
    if keystroke.modifiers.platform {
        #[cfg(target_os = "macos")]
        modifiers.push("Command");
        #[cfg(not(target_os = "macos"))]
        modifiers.push("Control");
    }
    if keystroke.modifiers.control {
        modifiers.push("Control");
    }
    if keystroke.modifiers.alt {
        modifiers.push("Alt");
    }
    if keystroke.modifiers.shift {
        modifiers.push("Shift");
    }
    let key = normalize_recorded_key(keystroke.key.as_str())?;
    if modifiers.is_empty() && !key.starts_with('F') {
        return None;
    }
    modifiers.push(key.as_str());
    Some(modifiers.join("+"))
}

fn shortcut_path(id: &str) -> Option<&'static str> {
    match id {
        "shortcuts.openClipboard" => Some("shortcuts.openClipboard"),
        "shortcuts.openPreference" => Some("shortcuts.openPreference"),
        _ => None,
    }
}

fn normalize_recorded_key(key: &str) -> Option<String> {
    let lower = key.to_ascii_lowercase();
    if lower.len() == 1 && lower.as_bytes()[0].is_ascii_alphanumeric() {
        return Some(lower.to_ascii_uppercase());
    }
    if let Some(number) = lower.strip_prefix('f')
        && let Ok(number) = number.parse::<u8>()
        && (1..=24).contains(&number)
    {
        return Some(format!("F{number}"));
    }
    let name = match lower.as_str() {
        "backspace" => "Backspace",
        "delete" | "del" => "Delete",
        "enter" | "return" => "Enter",
        "escape" | "esc" => "Esc",
        "home" => "Home",
        "end" => "End",
        "insert" => "Insert",
        "pageup" | "page_up" => "PageUp",
        "pagedown" | "page_down" => "PageDown",
        "arrowup" => "ArrowUp",
        "arrowdown" => "ArrowDown",
        "arrowleft" => "ArrowLeft",
        "arrowright" => "ArrowRight",
        "space" => "Space",
        "tab" => "Tab",
        "backquote" | "grave" => "Backquote",
        "backslash" => "Backslash",
        "bracketleft" => "BracketLeft",
        "bracketright" => "BracketRight",
        "comma" => "Comma",
        "equal" => "Equal",
        "minus" => "Minus",
        "period" => "Period",
        "quote" => "Quote",
        "semicolon" => "Semicolon",
        "slash" => "Slash",
        _ => return None,
    };
    Some(name.to_owned())
}

/// 偏好设置与首次引导共用的快捷键录制控件外观。
pub(crate) fn shortcut_recorder_button(
    id: &'static str,
    current: &str,
    settings: &Settings,
    recording: bool,
) -> Button {
    let conflict = shortcut_conflicts_with_settings(id, current, settings);
    let label: gpui::SharedString = if recording {
        i18n::t("preferences:controls.recordShortcut")
    } else if conflict {
        format!("{} ⚠", text::format_shortcut(current)).into()
    } else {
        text::format_shortcut(current).into()
    };
    Button::new(format!("shortcut-{id}"), label)
        .when(conflict && !recording, |button| button.danger())
        .tooltip(if recording {
            i18n::t("preferences:controls.recordShortcut")
        } else if conflict {
            i18n::t("preferences:controls.shortcutConflict")
        } else {
            i18n::t("preferences:controls.recordShortcut")
        })
}

pub(crate) fn search_matches(query: &str, title: &str, keywords: &[&str]) -> bool {
    let query = query.trim().to_ascii_lowercase();
    query.is_empty()
        || title.to_ascii_lowercase().contains(&query)
        || keywords
            .iter()
            .any(|keyword| keyword.to_ascii_lowercase().contains(&query))
}

#[cfg(test)]
mod tests {
    use gpui::{Keystroke, Modifiers};

    use kwikpaste_core::backup::BackupContainerMode;
    use kwikpaste_core::settings::CaptureKind;

    use super::{
        backup_confirmation_required, format_socket_address, reorder_capture_kinds, search_matches,
        shortcut_conflicts, shortcut_from_keystroke,
    };

    #[test]
    fn shortcut_conflicts_ignore_modifier_order() {
        assert!(shortcut_conflicts("Alt+X", "X+Alt"));
        assert!(!shortcut_conflicts("Alt+X", "Alt+C"));
        assert!(!shortcut_conflicts("", "Alt+X"));
    }

    #[test]
    fn search_matches_titles_and_keywords() {
        assert!(search_matches(
            "tray",
            "System startup",
            &["tray", "system"]
        ));
        assert!(!search_matches(
            "missing",
            "System startup",
            &["tray", "system"]
        ));
    }

    #[test]
    fn socket_addresses_keep_ipv6_literals_parseable() {
        assert_eq!(format_socket_address("127.0.0.1", 41573), "127.0.0.1:41573");
        assert_eq!(
            format_socket_address("fe80::1%12", 41573),
            "[fe80::1%12]:41573"
        );
        assert_eq!(
            format_socket_address("[fe80::1%12]", 41573),
            "[fe80::1%12]:41573"
        );
    }

    #[test]
    fn recorded_shortcut_uses_1x_modifier_order_and_physical_key() {
        let keystroke = Keystroke {
            modifiers: Modifiers {
                alt: true,
                shift: true,
                ..Modifiers::default()
            },
            key: "x".to_owned(),
            key_char: Some("x".to_owned()),
        };
        assert_eq!(
            shortcut_from_keystroke(&keystroke).as_deref(),
            Some("Alt+Shift+X")
        );
    }

    #[test]
    fn every_backup_mode_requires_import_confirmation() {
        assert!(backup_confirmation_required(BackupContainerMode::Plain));
        assert!(backup_confirmation_required(BackupContainerMode::Encrypted));
    }

    #[test]
    fn capture_order_drag_reorders_without_dropping_kinds() {
        let mut order = CaptureKind::default_order();
        let last = order.len() - 1;
        assert!(reorder_capture_kinds(&mut order, CaptureKind::Files, last));
        assert_eq!(order.len(), CaptureKind::default_order().len());
        assert_eq!(order.last(), Some(&CaptureKind::Files));
        assert!(!reorder_capture_kinds(&mut order, CaptureKind::Files, last));
    }
}
