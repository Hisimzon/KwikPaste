use gpui::{
    App, AppContext as _, Context, IntoElement, ParentElement as _, Render, ScrollHandle,
    Styled as _, Subscription, TitlebarOptions, Window, WindowBounds, WindowOptions, div,
    prelude::FluentBuilder as _, px, rems, size,
};
use kwikpaste_core::settings::Settings;
use kwikpaste_ui::{
    Button, Input, KpStyled as _, ScrollArea, Select, SelectOption, SelectState, Switch, TextInput,
    theme::{self, TextSize, space},
};
use serde_json::json;

use super::{
    schema::{self, Control, PermissionKind, Setting, TabId},
    text, values,
};
use crate::{core_host, i18n};

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
                let other = if setting.id == "shortcuts.openClipboard" {
                    values::get(&settings_json, "shortcuts.openPreference")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                } else {
                    values::get(&settings_json, "shortcuts.openClipboard")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                };
                let conflict = shortcut_conflicts(current, other);
                let label = if conflict {
                    format!("{} ⚠", text::format_shortcut(current))
                } else {
                    text::format_shortcut(current)
                };
                Button::new(format!("shortcut-{}", setting.id), label)
                    .ghost()
                    .when(conflict, |button| button.danger())
                    .tooltip(if conflict {
                        i18n::t("preferences:controls.shortcutConflict")
                    } else {
                        i18n::t("preferences:controls.recordShortcut")
                    })
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
            _ => {
                let id = setting.id;
                Button::new(format!("action-{id}"), i18n::t("common:actions.open"))
                    .ghost()
                    .on_click(move |_, _, _| {
                        if id == "about.checkUpdates" {
                            log::info!("manual update check requested from preferences");
                        } else {
                            log::info!("preferences action requested: {id}");
                        }
                    })
                    .into_any_element()
            }
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

impl Render for Preferences {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
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
    use super::{search_matches, shortcut_conflicts};

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
}
