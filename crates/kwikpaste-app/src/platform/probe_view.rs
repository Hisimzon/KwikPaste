//! `--selftest-platform` 下的面板内容：一个搜索框、一个当作列表的焦点区和一行状态，供
//! `tools/platform-probes` 验证钩子按键、编辑态和输入法。不是正式界面，接法与正式 UI 相同：
//! 收到 `Shown` 把焦点放到列表上，在输入框上按下鼠标或按 Ctrl+F 请求编辑态，
//! `EditingStarted` 后聚焦输入框，Esc 退出编辑态。Enter 粘贴最新一条记录，Ctrl+Enter 纯文本粘贴，
//! 走 [`super::paste`]（正式列表粘贴选中的记录）。

use gpui::{
    App, Context, FocusHandle, InteractiveElement as _, IntoElement, KeyBinding, MouseButton,
    MouseDownEvent, ParentElement as _, Render, SharedString, Styled as _, Subscription,
    TaskExt as _, Window, actions, div,
};
use kwikpaste_core::db::models::ClipboardItemQuery;
use kwikpaste_ui::{Input, TextInput, theme};

use super::editing::EditTrigger;
use super::panel::{Panel, PanelCommand, PanelEvent, Trigger, TriggerSource};
use super::{paste, probe};
use crate::core_host;

const CONTEXT: &str = "PlatformProbe";

actions!(
    platform_probe,
    [EnterEditing, Next, Previous, Confirm, PastePlain, Dismiss]
);

pub struct ProbeView {
    input: TextInput,
    list: FocusHandle,
    selected: i32,
    editing: bool,
    _subscriptions: Vec<Subscription>,
}

impl ProbeView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.bind_keys([
            KeyBinding::new("ctrl-f", EnterEditing, Some(CONTEXT)),
            KeyBinding::new("down", Next, Some(CONTEXT)),
            KeyBinding::new("up", Previous, Some(CONTEXT)),
            KeyBinding::new("enter", Confirm, Some(CONTEXT)),
            KeyBinding::new("ctrl-enter", PastePlain, Some(CONTEXT)),
            KeyBinding::new("escape", Dismiss, Some(CONTEXT)),
        ]);
        let input = TextInput::new("搜索 / search", window, cx);
        let changes = input.on_change(cx, |_, value, _| probe::input(&value));
        let keystrokes = cx.observe_keystrokes(|_, event, _, _| {
            let action = event.action.as_ref().map(|action| action.name());
            probe::keystroke(&event.keystroke.to_string(), action);
        });

        // 面板的全局句柄在窗口打开之前就已挂上，构造时即可订阅。
        let panel_events = cx
            .try_global::<Panel>()
            .map(|panel| panel.events().clone())
            .map(|events| {
                cx.subscribe_in(&events, window, |this, _, event, window, cx| {
                    this.on_panel_event(*event, window, cx);
                })
            });
        let mut subscriptions = vec![changes, keystrokes];
        subscriptions.extend(panel_events);

        Self {
            input,
            list: cx.focus_handle(),
            selected: 0,
            editing: false,
            _subscriptions: subscriptions,
        }
    }

    fn on_panel_event(&mut self, event: PanelEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            PanelEvent::Shown => window.focus(&self.list, cx),
            PanelEvent::EditingStarted => {
                self.editing = true;
                self.input.focus(window, cx);
            }
            PanelEvent::EditingEnded | PanelEvent::EditingRefused => {
                self.editing = false;
                window.focus(&self.list, cx);
            }
            PanelEvent::Hidden => {}
        }
        cx.notify();
    }

    fn request(command: PanelCommand, cx: &App) {
        super::request(cx, command);
    }

    /// 粘贴「全部」视图里最新的一条记录；历史为空时什么都不做。
    fn paste_newest(plain: bool, cx: &mut App) {
        let Some(core) = core_host::core(cx).cloned() else {
            return;
        };
        cx.spawn(async move |cx| {
            let query = ClipboardItemQuery {
                limit: 1,
                ..ClipboardItemQuery::default()
            };
            let Some(newest) = core.list_items(query).await?.list.into_iter().next() else {
                log::info!("nothing to paste: the history is empty");
                return Ok(());
            };
            cx.update(|cx| paste::paste(cx, newest.item.id, plain, false))
                .await
        })
        .detach_and_log_err(cx);
    }
}

impl Render for ProbeView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx);
        let status: SharedString = format!(
            "{} · selected {} · text scale {}",
            if self.editing { "editing" } else { "list" },
            self.selected,
            theme::text_scale(cx)
        )
        .into();

        div()
            .key_context(CONTEXT)
            .track_focus(&self.list)
            .size_full()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .bg(tokens.bg_container)
            .text_color(tokens.text)
            .on_action(cx.listener(|_, _: &EnterEditing, _, cx| {
                Self::request(PanelCommand::BeginEditing(EditTrigger::Keyboard), cx);
            }))
            .on_action(cx.listener(|this, _: &Next, _, cx| {
                this.selected += 1;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &Previous, _, cx| {
                this.selected -= 1;
                cx.notify();
            }))
            .on_action(cx.listener(|_, _: &Confirm, _, cx| Self::paste_newest(false, cx)))
            .on_action(cx.listener(|_, _: &PastePlain, _, cx| Self::paste_newest(true, cx)))
            .on_action(cx.listener(|this, _: &Dismiss, _, cx| {
                // Esc 先退出编辑态，再按一次才隐藏面板。
                let command = if this.editing {
                    PanelCommand::EndEditing
                } else {
                    PanelCommand::Hide(Trigger::now(TriggerSource::Ui))
                };
                Self::request(command, cx);
            }))
            .child(
                div()
                    .capture_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        if event.button == MouseButton::Left && !this.editing {
                            Self::request(PanelCommand::BeginEditing(EditTrigger::Mouse), cx);
                        }
                    }))
                    .child(Input::search(&self.input)),
            )
            .child(div().flex_1().child("KwikPaste platform probe"))
            .child(div().text_color(tokens.secondary).child(status))
    }
}
