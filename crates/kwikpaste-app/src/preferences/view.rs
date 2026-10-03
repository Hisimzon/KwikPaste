use gpui::{
    App, AppContext as _, Context, FocusHandle, InteractiveElement as _, IntoElement, KeyDownEvent,
    Keystroke, ParentElement as _, Render, ScrollHandle, Styled as _, Subscription,
    TitlebarOptions, WeakEntity, Window, WindowBounds, WindowOptions, div,
    prelude::FluentBuilder as _, px, rems, size,
};
use kwikpaste_core::{
    backup::{self, BackupContainerMode, BackupExportMode, BackupImportStrategy},
    ops::{PreferenceDirectory, StorageOverview},
    readable_export::{ExportFormat, ExportOptions},
    settings::Settings,
};
use kwikpaste_ui::{
    Button, Checkbox, DialogSpec, Input, KpStyled as _, ScrollArea, Select, SelectOption,
    SelectState, Switch, TextInput, form_dialog,
    theme::{self, TextSize, space},
    toast::{self, Toast},
};
use serde_json::json;
use std::{collections::HashSet, path::PathBuf, sync::Arc};

use super::{
    schema::{self, Control, PermissionKind, Setting, TabId},
    text, values,
};
use crate::{
    clipboard::{self, source::ClipboardSource, view::group_dialogs},
    core_host, i18n,
    platform::hotkey,
};

const WINDOW_SIZE: gpui::Size<gpui::Pixels> = size(px(980.), px(700.));

pub(super) fn open(cx: &mut App) -> anyhow::Result<()> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(WINDOW_SIZE, cx)),
        titlebar: Some(TitlebarOptions {
            title: Some(i18n::t("preferences:title")),
            ..Default::default()
        }),
        focus: false,
        ..Default::default()
    };
    kwikpaste_ui::open_window(options, cx, |window, cx| {
        let view = cx.new(|cx| Preferences::new(window, cx));
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        bring_window_to_front(window);
        view
    })
    .map(|_| ())
    .map_err(|error| anyhow::anyhow!("failed to open preferences window: {error:#}"))
}

pub(super) fn open_import(path: PathBuf, cx: &mut App) -> anyhow::Result<()> {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(WINDOW_SIZE, cx)),
        titlebar: Some(TitlebarOptions {
            title: Some(i18n::t("preferences:title")),
            ..Default::default()
        }),
        focus: false,
        ..Default::default()
    };
    kwikpaste_ui::open_window(options, cx, |window, cx| {
        let view = cx.new(|cx| Preferences::new(window, cx));
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        bring_window_to_front(window);
        Preferences::show_import_confirmation(path.clone(), window, cx);
        view
    })
    .map(|_| ())
    .map_err(|error| anyhow::anyhow!("failed to open preferences window: {error:#}"))
}

#[cfg(target_os = "windows")]
fn bring_window_to_front(window: &Window) {
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
fn bring_window_to_front(_: &Window) {
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
    scroll: ScrollHandle,
    focus: FocusHandle,
    recording: Option<&'static str>,
    storage_overview: Option<StorageOverview>,
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
        Self {
            tab: TabId::Overview,
            settings,
            search,
            _subscriptions: vec![search_subscription, appearance_sub, language_sub],
            appearance,
            language,
            scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            recording: None,
            storage_overview: None,
        }
    }

    fn update(&mut self, path: &'static str, value: serde_json::Value, cx: &mut Context<Self>) {
        let patch = values::patch(path, value);
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
                    Err(error) => log::error!("preferences update {path} failed: {error}"),
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
        } else {
            i18n::t("common:actions.open")
        };
        let entity = cx.entity().downgrade();
        Button::new(format!("action-{id}"), label)
            .ghost()
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
                    "about.checkUpdates" => {
                        log::info!("manual update check requested from preferences")
                    }
                    _ => log::info!("preferences action requested: {id}"),
                });
            })
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
            .children([
                format_storage_row(
                    i18n::t("preferences:overview.space.segments.database"),
                    usage.database_bytes,
                ),
                format_storage_row(
                    i18n::t("preferences:overview.space.segments.other"),
                    usage.resources_bytes,
                ),
                format_storage_row(
                    i18n::t("preferences:overview.space.hints.other"),
                    usage.settings_bytes,
                ),
                format_storage_row(
                    i18n::t("preferences:overview.tiles.total.label"),
                    usage.total_bytes,
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
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space(1.))
                    .kp_text(TextSize::Sm)
                    .child(format!(
                        "{}: {}",
                        i18n::t("preferences:overview.tiles.total.label"),
                        overview.history.totals.total
                    ))
                    .child(format!(
                        "{}: {}",
                        i18n::t("preferences:overview.categories.title"),
                        overview.history.categories.len()
                    ))
                    .child(format!(
                        "{}: {}",
                        i18n::t("preferences:overview.sources.title"),
                        overview.history.source_apps.len()
                    )),
            )
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
        let entity = cx.entity().downgrade();
        Button::new("group-select", selected.clone())
            .ghost()
            .on_click(move |_, _, cx| {
                if let Some(path) = path
                    && let Some(entity) = entity.upgrade()
                {
                    entity.update(cx, |this, cx| this.update(path, json!(next), cx));
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
                    .child(Select::new(&mode_content).width(rems(16.)))
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
                    .child(Select::new(&strategy_for_content).width(rems(16.)))
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
        let dialog = cx.new(|cx| ReadableExportDialog::new(window, cx));
        let content = dialog.clone();
        let answer = form_dialog(
            DialogSpec::new(i18n::t("preferences:readableExport.title"))
                .ok_text(i18n::t("preferences:readableExport.export"))
                .cancel_text(i18n::t("common:actions.cancel")),
            move |_, _| content.clone().into_any_element(),
            window,
            cx,
        );
        window
            .spawn(cx, async move |cx| {
                if !answer.await.unwrap_or(false) {
                    return;
                }
                let (format, favorites_only, include_sensitive) = cx
                    .update(|_, cx| {
                        let dialog = dialog.read(cx);
                        let format = if dialog
                            .format
                            .selected_value(cx)
                            .is_some_and(|format| format == "markdown")
                        {
                            ExportFormat::Markdown
                        } else {
                            ExportFormat::Xlsx
                        };
                        (format, dialog.favorites_only, dialog.include_sensitive)
                    })
                    .unwrap_or((ExportFormat::Xlsx, false, false));
                let Ok(prompt) = cx.update(|_, cx| {
                    let directory = core
                        .preference_directory(PreferenceDirectory::Data)
                        .unwrap_or_else(|_| std::env::temp_dir());
                    let name = if format == ExportFormat::Markdown {
                        "KwikPaste-readable.md"
                    } else {
                        "KwikPaste-readable.xlsx"
                    };
                    clipboard::view::pin::prompt_for_new_path(&directory, name, cx)
                }) else {
                    log::warn!("could not open readable export save dialog");
                    return;
                };
                let Some(path) = prompt.await else {
                    return;
                };
                let options = ExportOptions {
                    format,
                    favorites_only,
                    group_ids: None,
                    include_ungrouped: true,
                    split_by_group: false,
                    include_sensitive,
                };
                let Ok(preview) = core.preview_readable_export(options.clone()).await else {
                    log::warn!("readable export preview failed");
                    return;
                };
                match core
                    .export_readable_data(options, preview.fingerprint, path)
                    .await
                {
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
        div()
            .flex()
            .flex_col()
            .w(rems(15.))
            .gap(space(1.))
            .p(space(3.))
            .border_r_1()
            .children(tabs.into_iter().map(|tab| {
                let selected = tab.id == self.tab;
                let id = tab.id;
                Button::new(id.key(), text::tab_title_of(&tab))
                    .when(selected, |button| button.primary())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.tab = id;
                        if id == TabId::Overview {
                            this.refresh_storage_overview(cx);
                        }
                        cx.notify();
                    }))
            }))
    }

    fn render_setting(&self, setting: &Setting, cx: &mut Context<Self>) -> gpui::AnyElement {
        if !self.search.value(cx).is_empty() {
            let query = self.search.value(cx).to_lowercase();
            let title = text::setting_title(setting);
            if !search_matches(&query, &title, setting.keywords) {
                return div().into_any_element();
            }
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
                if setting.id == "appearance.theme" {
                    Select::new(&self.appearance)
                        .small()
                        .width(rems(12.))
                        .into_any_element()
                } else if setting.id == "appearance.language" {
                    Select::new(&self.language)
                        .small()
                        .width(rems(12.))
                        .into_any_element()
                } else {
                    let selected = value
                        .as_ref()
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("auto");
                    let options: Vec<String> = match setting.control {
                        Control::Select(_) => text::options(setting)
                            .into_iter()
                            .map(|(value, _)| value.to_string())
                            .collect(),
                        Control::Tiles(kind) => kind
                            .values()
                            .iter()
                            .map(|value| (*value).to_owned())
                            .collect(),
                        _ => Vec::new(),
                    };
                    let next = options
                        .iter()
                        .position(|option| option == selected)
                        .and_then(|index| options.get((index + 1) % options.len().max(1)))
                        .cloned()
                        .unwrap_or_else(|| selected.to_owned());
                    let entity = cx.entity().downgrade();
                    Button::new(format!("choice-{}", setting.id), selected.to_owned())
                        .ghost()
                        .on_click(move |_, _, cx| {
                            if let Some(path) = path {
                                let _ = entity.update(cx, |this, cx| {
                                    this.update(path, json!(next.clone()), cx);
                                });
                            }
                        })
                        .into_any_element()
                }
            }
            Control::ShortcutRecorder => {
                let current = value
                    .as_ref()
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let conflict =
                    shortcut_conflicts_with_settings(setting.id, current, &self.settings);
                let recording = self.recording == Some(setting.id);
                let label: gpui::SharedString = if recording {
                    i18n::t("preferences:controls.recordShortcut")
                } else if conflict {
                    format!("{} ⚠", text::format_shortcut(current)).into()
                } else {
                    text::format_shortcut(current).into()
                };
                let id = setting.id;
                Button::new(format!("shortcut-{}", setting.id), label)
                    .ghost()
                    .when(conflict && !recording, |button| button.danger())
                    .tooltip(if recording {
                        i18n::t("preferences:controls.recordShortcut")
                    } else if conflict {
                        i18n::t("preferences:controls.shortcutConflict")
                    } else {
                        i18n::t("preferences:controls.recordShortcut")
                    })
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
                    .ghost()
                    .on_click(move |_, _, cx| {
                        if let Err(error) = crate::platform::autostart::restart_as_admin(cx) {
                            log::warn!("administrator restart was not started: {error:#}");
                        }
                    })
                    .into_any_element()
            }
            Control::StorageOverview => self.render_storage_overview(cx),
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
            .py(space(2.))
            .border_b_1()
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(space(1.))
                    .flex_1()
                    .child(div().kp_text(TextSize::Sm).child(title))
                    .when(!description.is_empty(), |row| {
                        row.child(
                            div()
                                .kp_text(TextSize::Xs)
                                .text_color(theme::tokens(cx).secondary)
                                .child(description),
                        )
                    }),
            )
            .child(control)
            .into_any_element()
    }

    fn render_page(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let portable = core_host::core(cx).is_some_and(|core| core.paths().is_portable());
        let tabs = schema::tabs(portable);
        let tab = tabs.into_iter().find(|tab| tab.id == self.tab);
        let sections = tab.map(|tab| tab.sections).unwrap_or_default();
        let content = ScrollArea::new("preferences-scroll", &self.scroll)
            .p(space(5.))
            .gap(space(3.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(div().kp_text(TextSize::Lg).child(text::tab_title(self.tab)))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space(3.))
                            .when(self.tab == TabId::About, |row| {
                                row.child(
                                    div()
                                        .kp_text(TextSize::Xs)
                                        .text_color(theme::tokens(cx).secondary)
                                        .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
                                )
                            })
                            .child(Input::search(&self.search).small().width(rems(14.))),
                    ),
            )
            .children(sections.into_iter().map(|section| {
                div()
                    .flex()
                    .flex_col()
                    .gap(space(1.))
                    .child(
                        div()
                            .kp_text(TextSize::Sm)
                            .text_color(theme::tokens(cx).secondary)
                            .child(text::section_title(&section)),
                    )
                    .children(
                        section
                            .settings
                            .iter()
                            .filter(|setting| !setting.is_collapsed(&self.settings))
                            .map(|setting| self.render_setting(setting, cx)),
                    )
            }));

        div().flex_1().min_h_0().child(content)
    }
}

struct ReadableExportDialog {
    format: SelectState,
    favorites_only: bool,
    include_sensitive: bool,
}

impl ReadableExportDialog {
    fn new(window: &mut Window, cx: &mut App) -> Self {
        Self {
            format: SelectState::new(
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
            ),
            favorites_only: false,
            include_sensitive: false,
        }
    }
}

impl Render for ReadableExportDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity().downgrade();
        div()
            .flex()
            .flex_col()
            .gap(space(2.))
            .child(Select::new(&self.format).width(rems(16.)))
            .child(
                Checkbox::new("readable-favorites")
                    .label(i18n::t("preferences:readableExport.favorites"))
                    .checked(self.favorites_only)
                    .on_change({
                        let entity = entity.clone();
                        move |checked, _, cx| {
                            let _ = entity.update(cx, |dialog, cx| {
                                dialog.favorites_only = checked;
                                cx.notify();
                            });
                        }
                    }),
            )
            .child(
                Checkbox::new("readable-sensitive")
                    .label(i18n::t("preferences:readableExport.includeSensitive"))
                    .checked(self.include_sensitive)
                    .on_change(move |checked, _, cx| {
                        let _ = entity.update(cx, |dialog, cx| {
                            dialog.include_sensitive = checked;
                            cx.notify();
                        });
                    }),
            )
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

impl Render for Preferences {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.capture_shortcut(event, window, cx);
            }))
            .flex()
            .bg(theme::tokens(cx).bg_container)
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

    use super::{
        backup_confirmation_required, search_matches, shortcut_conflicts, shortcut_from_keystroke,
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
}
