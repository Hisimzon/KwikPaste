//! 首次启动引导窗：步骤和设置契约与 1.x `src/pages/Onboarding` 保持一致。

use gpui::{
    AnyWindowHandle, App, AppContext as _, Context, Div, Entity, FocusHandle, Global,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, ScrollHandle,
    SharedString, Styled as _, TitlebarOptions, Window, WindowBounds, WindowOptions, div, px, rems,
    size,
};
use kwikpaste_core::settings::Settings;
use kwikpaste_ui::{
    Button, ButtonSize, Checkbox, KpStyled as _, ScrollArea, Switch,
    theme::{self, TextSize, space},
};
use serde_json::json;

use super::{
    schema::{self, Control},
    text, values, view,
};
use crate::{core_host, i18n, platform::hotkey};

const WINDOW_SIZE: gpui::Size<gpui::Pixels> = size(px(760.), px(600.));
const WELCOME: usize = 0;
const PERMISSIONS: usize = 1;
const SHORTCUTS: usize = 2;
const IGNORE_APPS: usize = 3;
const DONE: usize = 4;

struct OnboardingWindow {
    handle: AnyWindowHandle,
    view: Entity<Onboarding>,
}

impl Global for OnboardingWindow {}

/// 打开首次引导窗。窗口不抢焦点，但按偏好窗相同的方式带到前台。
pub fn open(cx: &mut App) -> anyhow::Result<()> {
    if let Some(handle) = cx.try_global::<OnboardingWindow>().map(|host| host.handle) {
        return handle.update(cx, |_, window, _| {
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            view::bring_window_to_front(window);
        });
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(WINDOW_SIZE, cx)),
        titlebar: Some(TitlebarOptions {
            title: Some(i18n::t("onboarding:welcome.title")),
            ..Default::default()
        }),
        focus: false,
        ..Default::default()
    };
    let (handle, view) = kwikpaste_ui::open_window(options, cx, |window, cx| {
        let view = cx.new(|cx| Onboarding::new(window, cx));
        #[cfg(any(target_os = "windows", target_os = "macos"))]
        view::bring_window_to_front(window);
        view
    })?;
    cx.set_global(OnboardingWindow { handle, view });
    cx.on_window_closed(move |cx, closed_id| {
        if closed_id == handle.window_id() {
            hotkey::resume(cx);
            let _ = cx.remove_global::<OnboardingWindow>();
        }
    })
    .detach();
    Ok(())
}

struct Onboarding {
    step: usize,
    settings: Settings,
    has_permissions: bool,
    source_apps: Vec<kwikpaste_core::ops::ClipboardAppView>,
    recording: Option<&'static str>,
    focus: FocusHandle,
    scroll: ScrollHandle,
    finishing: bool,
}

impl Onboarding {
    fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = core_host::core(cx).map_or_else(Settings::default, |core| core.settings());
        let has_permissions = core_host::core(cx).is_some_and(|core| {
            #[cfg(target_os = "windows")]
            {
                !core.paths().is_portable()
            }
            #[cfg(target_os = "macos")]
            {
                true
            }
            #[cfg(not(any(target_os = "windows", target_os = "macos")))]
            {
                false
            }
        });
        let steps: usize = if has_permissions { 5 } else { 4 };
        let entity = cx.entity().downgrade();
        if let Some(core) = core_host::core(cx).cloned() {
            cx.spawn(async move |_, cx| match core.list_all_apps().await {
                Ok(source_apps) => {
                    let _ = entity.update(cx, |this, cx| {
                        this.source_apps = source_apps;
                        cx.notify();
                    });
                }
                Err(error) => log::warn!("could not load onboarding source apps: {error:#}"),
            })
            .detach();
        }
        Self {
            step: usize::try_from(settings.onboarding.last_step)
                .unwrap_or(0)
                .min(steps.saturating_sub(1)),
            settings,
            has_permissions,
            source_apps: Vec::new(),
            recording: None,
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            finishing: false,
        }
    }

    fn steps(&self) -> Vec<usize> {
        if self.has_permissions {
            vec![WELCOME, PERMISSIONS, SHORTCUTS, IGNORE_APPS, DONE]
        } else {
            vec![WELCOME, SHORTCUTS, IGNORE_APPS, DONE]
        }
    }

    fn current_position(&self) -> usize {
        self.step
    }

    fn current_kind(&self) -> usize {
        self.steps().get(self.step).copied().unwrap_or(WELCOME)
    }

    fn update_settings(&mut self, patch: serde_json::Value, cx: &mut Context<Self>) {
        if let Some(core) = core_host::core(cx).cloned() {
            cx.spawn(
                async move |this, cx| match core.update_settings(patch).await {
                    Ok(settings) => {
                        let _ = this.update(cx, |this, cx| {
                            this.settings = settings;
                            cx.notify();
                        });
                    }
                    Err(error) => log::error!("onboarding settings update failed: {error}"),
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

    fn save_step(&mut self, cx: &mut Context<Self>) {
        self.update_settings(json!({ "onboarding": { "lastStep": self.step } }), cx);
    }

    fn move_step(&mut self, delta: isize, cx: &mut Context<Self>) {
        self.finish_recording(cx);
        let steps = self.steps();
        let position = self.current_position() as isize + delta;
        let position = position.clamp(0, steps.len().saturating_sub(1) as isize) as usize;
        self.step = position;
        self.save_step(cx);
        cx.notify();
    }

    fn finish(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.finishing {
            return;
        }
        let Some(core) = core_host::core(cx).cloned() else {
            log::error!("onboarding cannot finish without the settings core");
            return;
        };
        self.finishing = true;
        self.finish_recording(cx);
        let patch = json!({ "onboarding": { "completed": true, "lastStep": self.step } });
        let handle = window.window_handle();
        cx.spawn(
            async move |this, cx| match core.update_settings(patch).await {
                Ok(_) => {
                    let _ = handle.update(cx, |_, window, _| window.remove_window());
                }
                Err(error) => {
                    log::error!("onboarding completion could not be saved: {error}");
                    let _ = this.update(cx, |this, cx| {
                        this.finishing = false;
                        cx.notify();
                    });
                }
            },
        )
        .detach();
        cx.notify();
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
        let Some(id) = self.recording else { return };
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
            self.save_shortcut(id, String::new(), cx);
            return;
        }
        let Some(shortcut) = view::shortcut_from_keystroke(&event.keystroke) else {
            return;
        };
        if view::shortcut_conflicts_with_settings(id, &shortcut, &self.settings) {
            log::warn!(
                "onboarding shortcut {shortcut:?} conflicts with an existing preference shortcut"
            );
            self.finish_recording(cx);
            return;
        }
        self.save_shortcut(id, shortcut, cx);
        let _ = window;
    }

    fn save_shortcut(&mut self, id: &'static str, value: String, cx: &mut Context<Self>) {
        let Some(path) = shortcut_path(id) else {
            self.finish_recording(cx);
            return;
        };
        self.finish_recording(cx);
        self.update_settings(values::patch(path, json!(value)), cx);
    }

    fn set_excluded_app(&mut self, id: String, excluded: bool, cx: &mut Context<Self>) {
        if excluded {
            if !self
                .settings
                .clipboard
                .filters
                .excluded_app_ids
                .contains(&id)
            {
                self.settings.clipboard.filters.excluded_app_ids.push(id);
            }
        } else {
            self.settings
                .clipboard
                .filters
                .excluded_app_ids
                .retain(|known| known != &id);
        }
        self.update_settings(
            json!({ "clipboard": { "filters": { "excludedAppIds": self.settings.clipboard.filters.excluded_app_ids } } }),
            cx,
        );
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let steps = self.steps();
        let position = self.current_position();
        div()
            .flex()
            .items_center()
            .justify_between()
            .px(space(5.))
            .pt(space(4.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space(2.))
                    .child(div().kp_text(TextSize::Sm).child(format!(
                        "{}/{}",
                        position + 1,
                        steps.len()
                    )))
                    .child(
                        div()
                            .kp_text(TextSize::Sm)
                            .child(i18n::t(match self.current_kind() {
                                WELCOME => "onboarding:steps.welcome",
                                PERMISSIONS => "onboarding:steps.permissions",
                                SHORTCUTS => "onboarding:steps.shortcuts",
                                IGNORE_APPS => "onboarding:steps.ignoreApps",
                                _ => "onboarding:steps.done",
                            })),
                    )
                    .child(
                        div()
                            .h(px(4.))
                            .w(rems(16.))
                            .rounded(theme::radius::MD)
                            .bg(theme::tokens(cx).fill_secondary)
                            .child(
                                div()
                                    .h_full()
                                    .w(rems(16. * ((position + 1) as f32 / steps.len() as f32)))
                                    .rounded(theme::radius::MD)
                                    .bg(theme::tokens(cx).primary),
                            ),
                    ),
            )
    }

    fn render_card(
        &self,
        title: SharedString,
        description: SharedString,
        cx: &mut Context<Self>,
    ) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(space(1.))
            .rounded(theme::radius::MD)
            .border_1()
            .border_color(theme::tokens(cx).border)
            .bg(theme::tokens(cx).bg_container)
            .p(space(4.))
            .child(div().kp_text(TextSize::Base).child(title))
            .child(
                div()
                    .kp_text(TextSize::Sm)
                    .text_color(theme::tokens(cx).secondary)
                    .child(description),
            )
    }

    fn render_welcome(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(space(3.))
            .child(
                div()
                    .kp_text(TextSize::Lg)
                    .child(i18n::t("onboarding:welcome.title")),
            )
            .child(
                div()
                    .kp_text(TextSize::Sm)
                    .text_color(theme::tokens(cx).secondary)
                    .child(i18n::t("onboarding:welcome.description")),
            )
            .child(div().grid_cols(3).gap(space(3.)).children([
                self.render_card(
                    i18n::t("onboarding:welcome.features.capture.title"),
                    i18n::t("onboarding:welcome.features.capture.description"),
                    cx,
                ),
                self.render_card(
                    i18n::t("onboarding:welcome.features.search.title"),
                    i18n::t("onboarding:welcome.features.search.description"),
                    cx,
                ),
                self.render_card(
                    i18n::t("onboarding:welcome.features.reuse.title"),
                    i18n::t("onboarding:welcome.features.reuse.description"),
                    cx,
                ),
            ]))
            .into_any_element()
    }

    fn render_permissions(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mut rows = Vec::new();
        let description_key = if cfg!(target_os = "macos") {
            "onboarding:permissions.description.macos"
        } else {
            "onboarding:permissions.description.windows"
        };
        #[cfg(target_os = "macos")]
        {
            let accessibility =
                Button::new("onboarding-accessibility", i18n::t("common:actions.open"))
                    .small()
                    .on_click(|_, _, _| {
                        if let Err(error) = kwikpaste_os::keystroke::ensure_accessibility_trusted()
                        {
                            log::debug!("accessibility permission is not available: {error}");
                        }
                    });
            rows.push(
                self.render_card(
                    i18n::t("preferences:schema.settings.permissions.accessibility.title"),
                    i18n::t("preferences:schema.settings.permissions.accessibility.description"),
                    cx,
                )
                .child(accessibility)
                .into_any_element(),
            );
            let disk = Button::new("onboarding-full-disk", i18n::t("common:actions.open"))
                .small()
                .on_click(|_, _, _| {
                    if let Err(error) =
                        kwikpaste_os::mac::permissions::open_full_disk_access_settings()
                    {
                        log::warn!("full disk access settings could not be opened: {error}");
                    }
                });
            rows.push(
                self.render_card(
                    i18n::t("preferences:schema.settings.permissions.fullDiskAccess.title"),
                    i18n::t("preferences:schema.settings.permissions.fullDiskAccess.description"),
                    cx,
                )
                .child(disk)
                .into_any_element(),
            );
        }
        #[cfg(target_os = "windows")]
        {
            let admin = Button::new("onboarding-admin", i18n::t("common:actions.open"))
                .small()
                .on_click(|_, _, cx| {
                    if let Err(error) = crate::platform::autostart::restart_as_admin(cx) {
                        log::warn!("administrator restart was not started: {error:#}");
                    }
                });
            rows.push(
                self.render_card(
                    i18n::t("preferences:schema.settings.permissions.runAsAdministrator.title"),
                    i18n::t(
                        "preferences:schema.settings.permissions.runAsAdministrator.description",
                    ),
                    cx,
                )
                .child(admin)
                .into_any_element(),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(space(3.))
            .child(
                div()
                    .kp_text(TextSize::Lg)
                    .child(i18n::t("onboarding:permissions.title")),
            )
            .child(
                div()
                    .kp_text(TextSize::Sm)
                    .text_color(theme::tokens(cx).secondary)
                    .child(i18n::t(description_key)),
            )
            .children(rows)
            .into_any_element()
    }

    fn render_shortcuts(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let settings_json = values::to_json(&self.settings);
        let mut rows = Vec::new();
        for setting in schema::shortcut_settings() {
            if setting.is_collapsed(&self.settings) {
                continue;
            }
            let Some(path) = setting.path else { continue };
            let value = values::get(&settings_json, path);
            let id = setting.id;
            let control =
                match setting.control {
                    Control::ShortcutRecorder => {
                        let current = value
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default();
                        view::shortcut_recorder_button(
                            id,
                            current,
                            &self.settings,
                            self.recording == Some(id),
                        )
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.begin_recording(id, window, cx)
                        }))
                        .into_any_element()
                    }
                    Control::Switch => {
                        let entity = cx.entity().downgrade();
                        Switch::new(id)
                            .checked(value.and_then(serde_json::Value::as_bool).unwrap_or(false))
                            .on_change(move |checked, _, cx| {
                                let _ = entity.update(cx, |this, cx| {
                                    this.update_settings(values::patch(path, json!(checked)), cx);
                                });
                            })
                            .into_any_element()
                    }
                    Control::Select(_) => {
                        let selected = value
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default();
                        let options = text::options(&setting);
                        let label = options
                            .iter()
                            .find(|(value, _)| value.as_ref() == selected)
                            .map(|(_, label)| label.clone())
                            .unwrap_or_else(|| selected.to_owned().into());
                        let next = options
                            .iter()
                            .position(|(value, _)| value.as_ref() == selected)
                            .and_then(|index| options.get((index + 1) % options.len().max(1)))
                            .or_else(|| options.first())
                            .map(|(value, _)| value.to_string());
                        Button::new(format!("choice-{id}"), label)
                            .ghost()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(next) = &next {
                                    this.update_settings(values::patch(path, json!(next)), cx);
                                }
                            }))
                            .into_any_element()
                    }
                    _ => continue,
                };
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(space(3.))
                    .py(space(3.))
                    .border_b_1()
                    .border_color(theme::tokens(cx).border)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(space(1.))
                            .child(
                                div()
                                    .kp_text(TextSize::Sm)
                                    .child(text::setting_title(&setting)),
                            )
                            .child(
                                div()
                                    .kp_text(TextSize::Xs)
                                    .text_color(theme::tokens(cx).secondary)
                                    .child(text::setting_description(&setting)),
                            ),
                    )
                    .child(control)
                    .into_any_element(),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(space(1.))
            .child(
                div()
                    .kp_text(TextSize::Lg)
                    .child(i18n::t("onboarding:shortcuts.title")),
            )
            .child(
                div()
                    .kp_text(TextSize::Sm)
                    .text_color(theme::tokens(cx).secondary)
                    .child(i18n::t("onboarding:shortcuts.description")),
            )
            .children(rows)
            .into_any_element()
    }

    fn render_ignore_apps(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let entity = cx.entity().downgrade();
        let rows = self.source_apps.iter().map(|app| {
            let id = app.id.clone();
            let label = if app.name.is_empty() {
                id.clone()
            } else {
                format!("{} ({id})", app.name)
            };
            let checked = self
                .settings
                .clipboard
                .filters
                .excluded_app_ids
                .contains(&id);
            Checkbox::new(format!("onboarding-ignore-{id}"))
                .label(label)
                .checked(checked)
                .on_change({
                    let entity = entity.clone();
                    move |checked, _, cx| {
                        if let Some(entity) = entity.upgrade() {
                            entity.update(cx, |this, cx| {
                                this.set_excluded_app(id.clone(), checked, cx);
                            });
                        }
                    }
                })
                .into_any_element()
        });
        div()
            .flex()
            .flex_col()
            .gap(space(2.))
            .child(
                div()
                    .kp_text(TextSize::Sm)
                    .text_color(theme::tokens(cx).secondary)
                    .child(i18n::t("onboarding:ignoreApps.description")),
            )
            .children(rows)
            .child(
                Button::new(
                    "onboarding-open-preferences",
                    i18n::t("common:actions.open"),
                )
                .small()
                .on_click(|_, _, cx| {
                    if let Err(error) = crate::preferences::open(cx) {
                        log::warn!("could not open preferences from onboarding: {error:#}");
                    }
                }),
            )
            .into_any_element()
    }

    fn render_step(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        match self.current_kind() {
            WELCOME => self.render_welcome(cx),
            PERMISSIONS => self.render_permissions(cx),
            SHORTCUTS => self.render_shortcuts(cx),
            IGNORE_APPS => div()
                .flex()
                .flex_col()
                .gap(space(3.))
                .child(
                    div()
                        .kp_text(TextSize::Lg)
                        .child(i18n::t("onboarding:ignoreApps.title")),
                )
                .child(self.render_ignore_apps(cx))
                .into_any_element(),
            DONE => self.render_done(cx),
            _ => div().into_any_element(),
        }
    }
}

impl Onboarding {
    fn render_done(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(space(3.))
            .child(
                div()
                    .kp_text(TextSize::Lg)
                    .child(i18n::t("onboarding:done.title")),
            )
            .child(
                div()
                    .kp_text(TextSize::Sm)
                    .text_color(theme::tokens(cx).secondary)
                    .child(i18n::t("onboarding:done.description")),
            )
            .child(div().grid_cols(3).gap(space(3.)).children([
                self.render_card(
                    i18n::t("onboarding:done.cards.open.title"),
                    i18n::t("onboarding:done.cards.open.description"),
                    cx,
                ),
                self.render_card(
                    i18n::t("onboarding:done.cards.search.title"),
                    i18n::t("onboarding:done.cards.search.description"),
                    cx,
                ),
                self.render_card(
                    i18n::t("onboarding:done.cards.preferences.title"),
                    i18n::t("onboarding:done.cards.preferences.description"),
                    cx,
                ),
            ]))
            .into_any_element()
    }
}

impl Render for Onboarding {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let steps = self.steps();
        let position = self.current_position();
        let is_last = position + 1 == steps.len();
        let is_first = position == 0;
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.capture_shortcut(event, window, cx)
            }))
            .flex()
            .flex_col()
            .bg(theme::tokens(cx).bg_container)
            .text_color(theme::tokens(cx).text)
            .child(self.render_header(cx))
            .child(
                ScrollArea::new("onboarding-scroll", &self.scroll)
                    .flex_1()
                    .p(space(6.))
                    .child(self.render_step(cx)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(space(5.))
                    .py(space(4.))
                    .border_t_1()
                    .border_color(theme::tokens(cx).border)
                    .child(
                        Button::new("onboarding-back", i18n::t("onboarding:actions.previous"))
                            .size(ButtonSize::Medium)
                            .disabled(is_first || self.finishing)
                            .on_click(cx.listener(|this, _, _, cx| this.move_step(-1, cx))),
                    )
                    .child(
                        div().flex().items_center().gap(space(2.)).children([
                            Button::new("onboarding-skip", i18n::t("onboarding:actions.skip"))
                                .ghost()
                                .disabled(self.finishing)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.finish(window, cx)),
                                )
                                .into_any_element(),
                            Button::new(
                                "onboarding-next",
                                if is_last {
                                    i18n::t("onboarding:actions.finish")
                                } else if is_first {
                                    i18n::t("onboarding:actions.start")
                                } else {
                                    i18n::t("onboarding:actions.next")
                                },
                            )
                            .primary()
                            .disabled(self.finishing)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if is_last {
                                    this.finish(window, cx);
                                } else {
                                    this.move_step(1, cx);
                                }
                            }))
                            .into_any_element(),
                        ]),
                    ),
            )
    }
}

fn shortcut_path(id: &str) -> Option<&'static str> {
    match id {
        "shortcuts.openClipboard" => Some("shortcuts.openClipboard"),
        "shortcuts.openPreference" => Some("shortcuts.openPreference"),
        _ => None,
    }
}
