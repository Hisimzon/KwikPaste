//! 编辑内容窗：与大图预览窗一样的普通窗口（带到前台、能拖大、最大化），每条记录一个。
//!
//! 面板只有 360 px 宽，长文本在弹框里看不全，所以正文在这里编辑，文本框随窗口伸缩。
//! 保存成功才关窗；空白、写入失败时提示原因，改动留着。有改动时 Esc 不关窗，关窗走取消或标题栏。

use std::sync::Arc;

use gpui::{
    AnyWindowHandle, App, AppContext as _, Context, Global, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, Styled as _, Task, WeakEntity, Window, WindowBounds,
    WindowOptions, div, prelude::FluentBuilder as _, px, size,
};
use kwikpaste_ui::{
    Button, KpStyled as _, TextArea, TextAreaInput, TitleBar,
    theme::{self, TextSize, space},
};

use super::list::{ClipboardList, error_toast};
use crate::{clipboard::source::ClipboardSource, i18n::t};

/// 窗口初始大小与最小尺寸（逻辑像素）。
const WINDOW: (f32, f32) = (760., 560.);
const MIN_WINDOW: (f32, f32) = (480., 320.);
/// 失败提示里的动作名。
const LABEL: &str = "clipboard:menu.editContent";

/// 要编辑的记录。
pub struct EditItem {
    pub id: Arc<str>,
    /// 完整正文。
    pub original: String,
    /// HTML / RTF 记录：保存后只剩纯文本，要提示。
    pub rich: bool,
}

/// 开着的编辑窗，按记录区分。
#[derive(Default)]
struct EditorWindows(Vec<(Arc<str>, AnyWindowHandle)>);

impl Global for EditorWindows {}

/// 打开记录的编辑窗；这条记录已经在编辑就把那个窗口带到前台。保存成功后交给 `list` 就地更新。
pub fn open(
    item: EditItem,
    source: Arc<dyn ClipboardSource>,
    list: WeakEntity<ClipboardList>,
    cx: &mut App,
) -> anyhow::Result<()> {
    let open = cx
        .default_global::<EditorWindows>()
        .0
        .iter()
        .find(|(id, _)| *id == item.id)
        .map(|(_, handle)| *handle);
    if let Some(handle) = open {
        return handle.update(cx, |_, window, cx| {
            crate::preferences::view::bring_window_to_front(window, cx);
        });
    }

    let id = item.id.clone();
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(WINDOW.0), px(WINDOW.1)), cx)),
        window_min_size: Some(size(px(MIN_WINDOW.0), px(MIN_WINDOW.1))),
        titlebar: Some(kwikpaste_ui::titlebar_options(t(LABEL))),
        focus: false,
        ..Default::default()
    };
    let (handle, _) = crate::platform::open_window(options, cx, |window, cx| {
        let view = cx.new(|cx| ContentEditor::new(item, source, list, window, cx));
        crate::platform::reveal_after_first_frame(window, cx, |window, cx| {
            crate::preferences::view::bring_window_to_front(window, cx);
        });
        view
    })?;
    cx.global_mut::<EditorWindows>().0.push((id, handle));
    let window_id = handle.window_id();
    cx.on_window_closed(move |cx, closed_id| {
        if closed_id == window_id {
            cx.global_mut::<EditorWindows>()
                .0
                .retain(|(_, handle)| handle.window_id() != window_id);
        }
    })
    .detach();
    Ok(())
}

pub struct ContentEditor {
    id: Arc<str>,
    original: String,
    rich: bool,
    input: TextAreaInput,
    source: Arc<dyn ClipboardSource>,
    list: WeakEntity<ClipboardList>,
    saving: Option<Task<()>>,
}

impl ContentEditor {
    fn new(
        item: EditItem,
        source: Arc<dyn ClipboardSource>,
        list: WeakEntity<ClipboardList>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let input = TextAreaInput::filling(item.original.clone(), window, cx);
        input.focus(window, cx);
        Self {
            id: item.id,
            original: item.original,
            rich: item.rich,
            input,
            source,
            list,
            saving: None,
        }
    }

    /// 空白报错；没改动直接关窗；否则原样保存，成功后关窗。
    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving.is_some() {
            return;
        }
        let value = self.input.value(cx).to_string();
        if value.trim().is_empty() {
            error_toast(
                LABEL,
                &anyhow::anyhow!(t("clipboard:content.empty")),
                window,
                cx,
            );
            return;
        }
        if value == self.original {
            window.remove_window();
            return;
        }

        let future = self.source.update_text_content(self.id.clone(), value);
        self.saving = Some(cx.spawn_in(window, async move |editor, cx| {
            let result = future.await;
            editor
                .update_in(cx, |editor, window, cx| {
                    editor.saving = None;
                    match result {
                        Ok(saved) => {
                            editor
                                .list
                                .update(cx, |list, cx| list.content_saved(saved, cx))
                                .ok();
                            window.remove_window();
                        }
                        Err(err) => {
                            error_toast(LABEL, &err, window, cx);
                            cx.notify();
                        }
                    }
                })
                .ok();
        }));
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        match keystroke.key.as_str() {
            "s" if keystroke.modifiers.secondary() => self.save(window, cx),
            "escape" if self.input.value(cx) == self.original.as_str() => window.remove_window(),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for ContentEditor {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::semantic(cx);
        let editor = cx.entity().downgrade();

        div()
            .key_context("ContentEditor")
            .on_key_down(cx.listener(Self::on_key_down))
            .flex()
            .flex_col()
            .size_full()
            .bg(crate::platform::material::shell_surface(
                cx,
                tokens.surface.panel,
            ))
            .text_color(tokens.text.primary)
            .child(TitleBar::new().title(t(LABEL)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .gap(space(3.))
                    .px(space(4.))
                    .pt(space(2.))
                    .pb(space(4.))
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .child(TextArea::new(&self.input).fill()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(space(2.))
                            .child(div().flex_1().min_w_0().when(self.rich, |note| {
                                note.kp_text(TextSize::Sm)
                                    .text_color(tokens.text.secondary)
                                    .child(t("clipboard:content.richWarning"))
                            }))
                            .child(
                                Button::new("editor-cancel", t("common:actions.cancel"))
                                    .on_click(|_, window, _| window.remove_window()),
                            )
                            .child(
                                Button::new("editor-save", t("common:actions.save"))
                                    .primary()
                                    .loading(self.saving.is_some())
                                    .on_click(move |_, window, cx| {
                                        editor
                                            .update(cx, |editor, cx| editor.save(window, cx))
                                            .ok();
                                    }),
                            ),
                    ),
            )
    }
}
