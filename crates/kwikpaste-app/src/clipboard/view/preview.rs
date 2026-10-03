//! 预览窗（1.x `pages/Preview` 与 `window/preview.rs`）：卡片旁边的一个独立的不激活窗口，头部是标题、
//! 说明、文本方式切换和类型标签，内容是大图、长文本（纯文本或选词）、HTML / RTF 的纯文本、文件列表。
//!
//! 窗口启动后按需建一次、永不销毁；显示、隐藏、定位只走原生调用（Windows 与主面板一样是
//! `WS_EX_NOACTIVATE` 的 topmost 工具窗口），点它、滚它、选词都不抢前台。原生调用都放在任务里、
//! GPUI 的借用之外（见 `platform::panel` 的说明）。macOS 的不激活面板还没接上（TODO(macOS)），
//! 预览先不出现。

use std::{ops::Range, rc::Rc, sync::Arc};

use gpui::{
    AnyElement, AnyWindowHandle, App, AppContext as _, Bounds, Context, Entity, EventEmitter,
    FontWeight, ImageSource, InteractiveElement as _, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, UniformListScrollHandle, Window, WindowBounds,
    WindowKind, WindowOptions, div, img, point, prelude::FluentBuilder as _, px, size,
    uniform_list,
};
use kwikpaste_core::db::models::{ClipboardKind, ClipboardSubKind};
use kwikpaste_ui::{
    Button, Icon, IconName, KpStyled as _,
    theme::{self, TextSize, radius},
};

use super::{
    card::dp,
    image_cache::{ImageKey, ImageState, KpImageCache, path_of},
};
use crate::{
    clipboard::{
        model::preview::{
            FILE_ROW_HEIGHT, HEADER_HEIGHT, RectF, TEXT_ROW_HEIGHT, WordSelection, format_bytes,
            text_rows, utf16_range,
        },
        source::{Preview, PreviewTextView},
    },
    i18n::{t, t_args, t_count},
};

/// 预览窗发给列表的事件。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PreviewEvent {
    /// 指针进出预览窗（悬停预览离开卡片后，指针在预览窗里就不关）。
    Pointer(bool),
    /// 切换文本方式（写进设置）。
    TextView(PreviewTextView),
    /// 选词后点了复制或粘贴。
    Words { paste: bool },
}

/// 预览窗的内容视图。
pub struct PreviewPanel {
    preview: Option<Preview>,
    /// 记录已经不在了：只显示空状态。
    missing: bool,
    text_view: PreviewTextView,
    rows: Rc<Vec<Range<usize>>>,
    selection: WordSelection,
    /// 图片在面板里的显示尺寸（逻辑像素），由列表按面板尺寸算好。
    image_box: Option<(f32, f32)>,
    images: Entity<KpImageCache>,
    text_scroll: UniformListScrollHandle,
    scroll: ScrollHandle,
}

impl EventEmitter<PreviewEvent> for PreviewPanel {}

impl PreviewPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        let images = cx.new(|_| KpImageCache::new());
        cx.observe(&images, |_, _, cx| cx.notify()).detach();

        Self {
            preview: None,
            missing: false,
            text_view: PreviewTextView::default(),
            rows: Rc::default(),
            selection: WordSelection::default(),
            image_box: None,
            images,
            text_scroll: UniformListScrollHandle::new(),
            scroll: ScrollHandle::new(),
        }
    }

    /// 换内容：选词清空，滚动回到顶部。
    pub fn set(
        &mut self,
        preview: Option<Preview>,
        text_view: PreviewTextView,
        image_box: Option<(f32, f32)>,
        cx: &mut Context<Self>,
    ) {
        let same_item = matches!(
            (&self.preview, &preview),
            (Some(old), Some(new)) if old.payload.id == new.payload.id
        );
        if !same_item {
            self.selection.clear();
            self.scroll.set_offset(point(px(0.), px(0.)));
            self.text_scroll
                .scroll_to_item(0, gpui::ScrollStrategy::Top);
        }
        self.rows = Rc::new(
            preview
                .as_ref()
                .and_then(|preview| preview.payload.text.as_deref())
                .map(text_rows)
                .unwrap_or_default(),
        );
        self.missing = preview.is_none();
        self.preview = preview;
        self.text_view = text_view;
        self.image_box = image_box;
        cx.notify();
    }

    pub fn item_id(&self) -> Option<&str> {
        self.preview
            .as_ref()
            .map(|preview| preview.payload.id.as_str())
    }

    /// 选词视图里选中的词（序号升序）；不在选词视图或没选时为空。
    pub fn selected_words(&self) -> Vec<usize> {
        if !self.words_view() {
            return Vec::new();
        }
        self.selection.selected().iter().copied().collect()
    }

    /// 释放图片（预览关上时）。
    pub fn release(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selection.clear();
        self.images
            .update(cx, |images, cx| images.clear(Some(window), cx));
    }

    fn can_pick_words(&self) -> bool {
        self.preview.as_ref().is_some_and(|preview| {
            preview.payload.kind == ClipboardKind::Text && !preview.payload.words.is_empty()
        })
    }

    fn words_view(&self) -> bool {
        self.text_view == PreviewTextView::Words && self.can_pick_words()
    }

    fn header(&self, cx: &mut Context<Self>) -> AnyElement {
        let tokens = theme::tokens(cx);
        let payload = self.preview.as_ref().map(|preview| &preview.payload);
        let (title, meta) = match payload {
            None => (t("preview:title.loading"), t("preview:meta.contentViewer")),
            Some(payload) => match payload.kind {
                ClipboardKind::Files => (
                    t_count(
                        "preview:title.files",
                        i64::try_from(payload.total_files).unwrap_or(i64::MAX),
                        &[],
                    ),
                    t_count(
                        "preview:meta.filesLoaded",
                        i64::try_from(payload.files.len()).unwrap_or(i64::MAX),
                        &[],
                    ),
                ),
                ClipboardKind::Image => {
                    let dimensions = match (payload.image_width, payload.image_height) {
                        (Some(width), Some(height)) => format!("{width} x {height}"),
                        _ => t("preview:meta.unknownSize").to_string(),
                    };
                    let size = payload
                        .size
                        .map(|size| format!(" · {}", format_bytes(size)))
                        .unwrap_or_default();
                    (
                        t("preview:title.image"),
                        SharedString::from(format!("{dimensions}{size}")),
                    )
                }
                ClipboardKind::Text => {
                    let count = payload.size.unwrap_or_else(|| {
                        let units = payload
                            .text
                            .as_deref()
                            .map_or(0, |text| text.encode_utf16().count());
                        i64::try_from(units).unwrap_or(i64::MAX)
                    });
                    (
                        t("preview:title.text"),
                        t_count("preview:meta.characters", count, &[]),
                    )
                }
            },
        };

        div()
            .flex()
            .flex_none()
            .h(dp(HEADER_HEIGHT as f32))
            .items_center()
            .justify_between()
            .gap(dp(12.))
            .px(dp(16.))
            .border_b_1()
            .border_color(tokens.border)
            .child(
                div()
                    .min_w_0()
                    .child(
                        div()
                            .truncate()
                            .kp_text(TextSize::Sm)
                            .font_weight(FontWeight::MEDIUM)
                            .child(title),
                    )
                    .child(
                        div()
                            .truncate()
                            .kp_text(TextSize::Xs)
                            .text_color(tokens.secondary)
                            .child(meta),
                    ),
            )
            .when_some(payload, |header, payload| {
                header.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(dp(8.))
                        .when(self.can_pick_words(), |row| row.child(self.view_switch(cx)))
                        .child(
                            div()
                                .rounded(radius::SM)
                                .bg(tokens.fill_secondary)
                                .px(dp(8.))
                                .py(dp(2.))
                                .kp_text(TextSize::Xs)
                                .text_color(tokens.secondary)
                                .child(t(type_key(payload.kind, payload.sub_kind))),
                        ),
                )
            })
            .into_any_element()
    }

    /// 文本方式切换（1.x antd `Segmented size="small"`）。
    fn view_switch(&self, cx: &mut Context<Self>) -> AnyElement {
        let tokens = theme::tokens(cx);
        let segment = |view: PreviewTextView, key: &str, cx: &mut Context<Self>| {
            let selected = self.text_view == view;
            div()
                .id(SharedString::from(format!("preview-view-{key}")))
                .flex()
                .items_center()
                .h(dp(20.))
                .px(dp(8.))
                .rounded(radius::XS)
                .kp_text(TextSize::Xs)
                .cursor_pointer()
                .map(|segment| {
                    if selected {
                        segment.bg(tokens.fill).text_color(tokens.text)
                    } else {
                        segment
                            .text_color(tokens.secondary)
                            .hover(|style| style.text_color(tokens.text))
                    }
                })
                .child(t(key))
                .on_click(cx.listener(move |panel, _, _, cx| {
                    if panel.text_view != view {
                        panel.text_view = view;
                        panel.selection.clear();
                        cx.emit(PreviewEvent::TextView(view));
                        cx.notify();
                    }
                }))
        };

        div()
            .flex()
            .gap(dp(2.))
            .p(dp(2.))
            .rounded(radius::SM)
            .bg(tokens.fill_tertiary)
            .child(segment(PreviewTextView::Plain, "preview:view.plain", cx))
            .child(segment(PreviewTextView::Words, "preview:view.words", cx))
            .into_any_element()
    }

    fn empty(key: &str, cx: &App) -> AnyElement {
        let tokens = theme::tokens(cx);

        div()
            .flex()
            .flex_col()
            .size_full()
            .min_h(dp(96.))
            .items_center()
            .justify_center()
            .gap(dp(8.))
            .child(
                Icon::new(IconName::Inbox)
                    .size(dp(32.))
                    .color(tokens.quaternary),
            )
            .child(
                div()
                    .kp_text(TextSize::Sm)
                    .text_color(tokens.secondary)
                    .child(t(key)),
            )
            .into_any_element()
    }

    fn content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(preview) = self.preview.clone() else {
            return if self.missing {
                Self::empty("preview:empty.content", cx)
            } else {
                div().into_any_element()
            };
        };
        let payload = &preview.payload;

        match payload.kind {
            ClipboardKind::Image => self.image(&preview, window, cx),
            ClipboardKind::Files => self.files(&preview, cx),
            ClipboardKind::Text => {
                let text = payload.text.as_deref().unwrap_or_default();
                if text.is_empty() {
                    Self::empty("preview:empty.text", cx)
                } else if self.words_view() {
                    self.words(&preview, cx)
                } else {
                    self.plain_text(&preview, cx)
                }
            }
        }
    }

    fn image(
        &mut self,
        preview: &Preview,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let payload = &preview.payload;
        let (Some(path), true, Some((width, height))) = (
            payload.image_path.as_deref(),
            payload.image_exists,
            self.image_box,
        ) else {
            return Self::empty("preview:empty.imageMissing", cx);
        };
        let scale = window.scale_factor();
        let key = ImageKey {
            path: path_of(path),
            width: (width * scale).round().max(1.) as u32,
            height: (height * scale).round().max(1.) as u32,
        };
        let state = self
            .images
            .update(cx, |images, cx| images.request(key, window, cx));

        div()
            .flex()
            .size_full()
            .items_center()
            .justify_center()
            .p(dp(16.))
            .child(match state {
                ImageState::Ready(image) => img(ImageSource::Render(image))
                    .w(px(width))
                    .h(px(height))
                    .into_any_element(),
                ImageState::Loading => div().w(px(width)).h(px(height)).into_any_element(),
                ImageState::Failed => Self::empty("preview:empty.imageMissing", cx),
            })
            .into_any_element()
    }

    fn plain_text(&self, preview: &Preview, cx: &mut Context<Self>) -> AnyElement {
        let text: Arc<str> = Arc::from(preview.payload.text.as_deref().unwrap_or_default());
        let rows = self.rows.clone();

        uniform_list(
            "preview-text",
            rows.len(),
            cx.processor(move |_, range: Range<usize>, _, _| {
                range
                    .map(|index| {
                        let line = rows
                            .get(index)
                            .and_then(|row| text.get(row.clone()))
                            .filter(|line| !line.is_empty())
                            .unwrap_or(" ");
                        div()
                            .h(dp(TEXT_ROW_HEIGHT as f32))
                            .px(dp(16.))
                            .kp_mono()
                            .kp_text(TextSize::Xs)
                            .line_height(dp(TEXT_ROW_HEIGHT as f32))
                            .whitespace_nowrap()
                            .child(SharedString::from(line.to_owned()))
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.text_scroll)
        .size_full()
        .py(dp(16.))
        .into_any_element()
    }

    fn words(&self, preview: &Preview, cx: &mut Context<Self>) -> AnyElement {
        let tokens = theme::tokens(cx);
        let payload = &preview.payload;
        let text = payload.text.as_deref().unwrap_or_default();
        let mut chips: Vec<AnyElement> = Vec::with_capacity(payload.words.len() + 8);
        let mut previous_end = 0;

        for (index, span) in payload.words.iter().enumerate() {
            let range = utf16_range(text, span.0, span.1);
            let between = text.get(previous_end.min(range.start)..range.start);
            if index > 0 && between.is_some_and(|between| between.contains('\n')) {
                chips.push(div().w_full().h(px(0.)).into_any_element());
            }
            previous_end = range.end;
            let selected = self.selection.is_selected(index);
            let word = SharedString::from(text.get(range).unwrap_or_default().to_owned());

            chips.push(
                div()
                    .id(("preview-word", index))
                    .min_w(dp(24.))
                    .max_w_full()
                    .px(dp(6.))
                    .py(dp(2.))
                    .rounded(radius::MD)
                    .kp_text(TextSize::Sm)
                    .cursor_pointer()
                    .map(|chip| {
                        if selected {
                            chip.bg(tokens.primary).text_color(tokens.light_solid)
                        } else {
                            chip.bg(tokens.fill_tertiary)
                                .hover(|style| style.bg(tokens.fill_secondary))
                        }
                    })
                    .child(div().text_center().child(word))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |panel, _: &MouseDownEvent, _, cx| {
                            panel.selection.press(index);
                            cx.notify();
                        }),
                    )
                    .on_mouse_move(cx.listener(move |panel, event: &MouseMoveEvent, _, cx| {
                        if event.pressed_button == Some(MouseButton::Left)
                            && panel.selection.dragging()
                        {
                            panel.selection.extend(index);
                            cx.notify();
                        }
                    }))
                    .into_any_element(),
            );
        }

        let count = self.selection.selected().len();
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(
                div()
                    .id("preview-words")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .content_start()
                            .gap(dp(4.))
                            .p(dp(16.))
                            .children(chips),
                    )
                    .when(payload.words_truncated, |area| {
                        area.child(
                            div()
                                .px(dp(16.))
                                .pb(dp(16.))
                                .kp_text(TextSize::Xs)
                                .text_color(tokens.secondary)
                                .child(t("preview:words.truncated")),
                        )
                    }),
            )
            .when(count > 0, |area| {
                area.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(dp(8.))
                        .border_t_1()
                        .border_color(tokens.border)
                        .py(dp(8.))
                        .pr(dp(12.))
                        .pl(dp(16.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .kp_text(TextSize::Xs)
                                .text_color(tokens.secondary)
                                .child(t_count(
                                    "preview:words.selected",
                                    i64::try_from(count).unwrap_or(i64::MAX),
                                    &[],
                                )),
                        )
                        .child(
                            Button::new("preview-words-clear", t("preview:words.clear"))
                                .small()
                                .ghost()
                                .on_click(cx.listener(|panel, _, _, cx| {
                                    panel.selection.clear();
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("preview-words-copy", t("preview:words.copy"))
                                .small()
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(PreviewEvent::Words { paste: false });
                                })),
                        )
                        .child(
                            Button::new("preview-words-paste", t("preview:words.paste"))
                                .small()
                                .primary()
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(PreviewEvent::Words { paste: true });
                                })),
                        ),
                )
            })
            .into_any_element()
    }

    fn files(&self, preview: &Preview, cx: &mut Context<Self>) -> AnyElement {
        let tokens = theme::tokens(cx);
        let payload = &preview.payload;
        if payload.files.is_empty() {
            return Self::empty("preview:empty.files", cx);
        }

        let rows = payload.files.iter().map(|file| {
            let kind = if file.is_dir {
                t("preview:file.folder")
            } else {
                t("preview:file.item")
            };
            let size_label = file
                .size
                .map(|size| SharedString::from(format_bytes(size)))
                .unwrap_or(kind);
            let path = if file.exists {
                SharedString::from(file.path.clone())
            } else {
                t("preview:file.missingPath")
            };

            div().px(dp(8.)).child(
                div()
                    .flex()
                    .min_h(dp(FILE_ROW_HEIGHT as f32))
                    .items_center()
                    .gap(dp(8.))
                    .rounded(radius::MD)
                    .px(dp(8.))
                    .py(dp(6.))
                    .when(!file.exists, |row| row.opacity(0.5))
                    .child(match &file.icon_path {
                        Some(icon) => img(path_of(icon))
                            .flex_none()
                            .size(dp(24.))
                            .into_any_element(),
                        None => Icon::new(IconName::Folder)
                            .size(dp(20.))
                            .color(tokens.secondary)
                            .into_any_element(),
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .kp_text(TextSize::Xs)
                                    .when(!file.exists, |name| name.line_through())
                                    .child(file.name.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .kp_text(TextSize::Xs)
                                    .text_color(tokens.secondary)
                                    .child(path),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .kp_text(TextSize::Xs)
                            .text_color(tokens.secondary)
                            .child(size_label),
                    ),
            )
        });

        div()
            .id("preview-files")
            .size_full()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .py(dp(8.))
            .children(rows)
            .when(payload.total_files > payload.files.len(), |list| {
                list.child(
                    div()
                        .px(dp(16.))
                        .py(dp(8.))
                        .kp_text(TextSize::Xs)
                        .text_color(tokens.secondary)
                        .child(t_args(
                            "preview:file.shownCount",
                            &[
                                ("shown", &payload.files.len().to_string()),
                                ("total", &payload.total_files.to_string()),
                            ],
                        )),
                )
            })
            .into_any_element()
    }
}

/// 类型标签的文案 key（1.x `clipboard:types.{subKind ?? kind}`）。
fn type_key(kind: ClipboardKind, sub_kind: Option<ClipboardSubKind>) -> &'static str {
    match (sub_kind, kind) {
        (Some(ClipboardSubKind::Rtf), _) => "clipboard:types.rtf",
        (Some(ClipboardSubKind::Html), _) => "clipboard:types.html",
        (Some(ClipboardSubKind::Url), _) => "clipboard:types.url",
        (Some(ClipboardSubKind::Email), _) => "clipboard:types.email",
        (Some(ClipboardSubKind::Color), _) => "clipboard:types.color",
        (Some(ClipboardSubKind::Path), _) => "clipboard:types.path",
        (None, ClipboardKind::Text) => "clipboard:types.text",
        (None, ClipboardKind::Image) => "clipboard:types.image",
        (None, ClipboardKind::Files) => "clipboard:types.files",
    }
}

impl Render for PreviewPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tokens = theme::tokens(cx);
        let content = self.content(window, cx);

        div()
            .id("preview-panel")
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(tokens.bg_elevated)
            .border_1()
            .border_color(tokens.border_secondary)
            .text_color(tokens.text)
            .on_hover(cx.listener(|_, hovered: &bool, _, cx| {
                cx.emit(PreviewEvent::Pointer(*hovered));
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|panel, _, _, _| panel.selection.release()),
            )
            .child(self.header(cx))
            .child(div().flex_1().min_h_0().relative().child(content))
    }
}

/// 预览窗：GPUI 窗口、内容视图和原生句柄。
pub struct PreviewWindow {
    pub handle: AnyWindowHandle,
    pub panel: Entity<PreviewPanel>,
    pub native: Rc<native::NativePreview>,
}

impl PreviewWindow {
    /// 以隐藏状态建窗。原生样式（不激活、topmost）要在借用之外补上，见 [`native::NativePreview::install`]。
    pub fn open(cx: &mut App) -> anyhow::Result<Self> {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(px(320.), px(240.)),
            ))),
            titlebar: None,
            focus: false,
            show: false,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            inactive_frame_interval: None,
            window_background: gpui::WindowBackgroundAppearance::Opaque,
            ..Default::default()
        };
        let mut native = None;
        let (handle, panel) = kwikpaste_ui::open_window(options, cx, |window, cx| {
            native = Some(native::NativePreview::attach(window));
            cx.new(PreviewPanel::new)
        })?;
        let native =
            native.ok_or_else(|| anyhow::anyhow!("the preview window was not built"))??;

        Ok(Self {
            handle,
            panel,
            native: Rc::new(native),
        })
    }
}

/// 原生预览窗（Windows）：与主面板同一套不激活的工具窗口。
#[cfg(target_os = "windows")]
pub mod native {
    use anyhow::{anyhow, bail};
    use gpui::{Bounds, Pixels, Window};
    use kwikpaste_os::geometry::{Point, Rect};
    use kwikpaste_os::win::{
        monitor::{self, MonitorInfo},
        panel::{self as win_panel, PanelOptions},
    };
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    use super::ScreenPlace;
    use crate::clipboard::model::preview::RectF;

    pub struct NativePreview {
        panel: win_panel::Panel,
    }

    fn hwnd(window: &Window) -> anyhow::Result<isize> {
        let handle = HasWindowHandle::window_handle(window)
            .map_err(|err| anyhow!("window handle: {err:?}"))?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            bail!("not a Win32 window");
        };
        Ok(handle.hwnd.get())
    }

    impl NativePreview {
        pub fn attach(window: &Window) -> anyhow::Result<Self> {
            // GPUI 在主线程建窗，预览窗永不销毁。
            Ok(Self {
                panel: unsafe { win_panel::Panel::from_raw(hwnd(window)?) },
            })
        }

        /// 补上不激活、topmost 的样式。要在 GPUI 的借用之外调用。
        pub fn install(&self) {
            if let Err(err) = self.panel.install(PanelOptions {
                min_logical_size: (1., 1.),
                text_scale: 1.,
            }) {
                log::error!("preview window setup failed: {err}");
            }
        }

        /// 把内容区放到 `client`（物理像素）并不激活地显示。要在 GPUI 的借用之外调用。
        pub fn show(&self, client: Rect, dpi: u32) {
            if let Err(err) = self.panel.place(client, dpi) {
                log::warn!("preview window could not be placed: {err}");
                return;
            }
            self.panel.show_without_activating();
        }

        /// 隐藏。要在 GPUI 的借用之外调用。
        pub fn hide(&self) {
            self.panel.hide();
        }

        pub fn is_visible(&self) -> bool {
            self.panel.is_visible()
        }

        /// 预览窗是不是前台窗口（自测核对“不抢前台”）。
        pub fn is_foreground(&self) -> bool {
            kwikpaste_os::win::foreground_window() == self.panel.raw()
        }
    }

    /// 窗口内容区左上角的屏幕坐标（物理像素）。
    fn client_origin(window: &Window) -> Option<Point> {
        let panel = unsafe { win_panel::Panel::from_raw(hwnd(window).ok()?) };
        let rect = panel.client_rect().ok()?;
        Some(Point {
            x: rect.left,
            y: rect.top,
        })
    }

    /// 包含这个点（物理像素）的显示器；都不包含时取第一块。
    fn monitor_at(point: Point) -> Option<MonitorInfo> {
        let monitors = monitor::all();
        monitors
            .iter()
            .copied()
            .find(|info| {
                let rect = info.monitor;
                point.x >= rect.left
                    && point.x < rect.right
                    && point.y >= rect.top
                    && point.y < rect.bottom
            })
            .or_else(|| monitors.first().copied())
    }

    /// 面板窗口里 `card`（逻辑像素）所在显示器上的几何。
    pub fn screen_place(window: &Window, card: Bounds<Pixels>) -> Option<ScreenPlace> {
        let origin = client_origin(window)?;
        let scale = f64::from(window.scale_factor());
        let to_screen = |value: Pixels| f64::from(value.as_f32()) * scale;
        let left = f64::from(origin.x) + to_screen(card.origin.x);
        let top = f64::from(origin.y) + to_screen(card.origin.y);
        let width = to_screen(card.size.width);
        let height = to_screen(card.size.height);
        let monitor = monitor_at(Point {
            x: (left + width / 2.) as i32,
            y: (top + height / 2.) as i32,
        })?;
        let monitor_scale = monitor.scale();

        Some(ScreenPlace {
            card: RectF {
                left: (left - f64::from(monitor.monitor.left)) / monitor_scale,
                top: (top - f64::from(monitor.monitor.top)) / monitor_scale,
                width: width / monitor_scale,
                height: height / monitor_scale,
            },
            monitor: RectF {
                left: 0.,
                top: 0.,
                width: f64::from(monitor.monitor.width()) / monitor_scale,
                height: f64::from(monitor.monitor.height()) / monitor_scale,
            },
            origin: (monitor.monitor.left, monitor.monitor.top),
            scale: monitor_scale,
            dpi: monitor.dpi,
        })
    }
}

/// 卡片所在显示器上的几何：卡片与显示器都是以显示器左上角为原点的逻辑像素。
pub struct ScreenPlace {
    pub card: RectF,
    pub monitor: RectF,
    /// 显示器左上角的屏幕坐标（物理像素）。
    pub origin: (i32, i32),
    /// 显示器的缩放比例。
    pub scale: f64,
    pub dpi: u32,
}

impl ScreenPlace {
    /// 显示器上的逻辑矩形换成屏幕上的物理矩形。
    pub fn to_screen(&self, rect: RectF) -> kwikpaste_os::geometry::Rect {
        let to_physical = |value: f64| (value * self.scale).round() as i32;
        let left = self.origin.0 + to_physical(rect.left);
        let top = self.origin.1 + to_physical(rect.top);
        kwikpaste_os::geometry::Rect {
            left,
            top,
            right: left + to_physical(rect.width),
            bottom: top + to_physical(rect.height),
        }
    }
}

/// TODO(macOS)：预览窗要做成不激活的 NSPanel 才能显示；先不显示。
#[cfg(target_os = "macos")]
pub mod native {
    use gpui::{Bounds, Pixels, Window};

    use super::ScreenPlace;

    pub struct NativePreview;

    pub fn screen_place(_: &Window, _: Bounds<Pixels>) -> Option<ScreenPlace> {
        None
    }

    impl NativePreview {
        pub fn attach(_: &Window) -> anyhow::Result<Self> {
            anyhow::bail!("the preview window is not available on macOS yet")
        }

        pub fn install(&self) {}

        pub fn show(&self, _: kwikpaste_os::geometry::Rect, _: u32) {}

        pub fn hide(&self) {}

        pub fn is_visible(&self) -> bool {
            false
        }

        pub fn is_foreground(&self) -> bool {
            false
        }
    }
}
