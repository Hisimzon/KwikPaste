//! 预览窗的开合（1.x `useClipboardPreviewController`）。
//!
//! - 悬停：指针停在卡片上（显示后指针真的动过）`hoverDelayMs` 后打开；已有悬停预览时换卡片立即换内容；
//!   离开卡片 240 ms 后关，期间指针进了预览窗就不关。
//! - 空格（设置打开时）：按住预览当前项，↑/↓ 换当前项时跟着换，松开关；松开时指针在预览窗里就转成
//!   悬停式（离开预览窗再关）。
//! - 关：面板隐藏、换筛选条件、滚动（悬停式）、Esc、对这一条做了粘贴 / 复制 / 删除等操作。
//!
//! 预览窗的几何在打开时按卡片在屏幕上的位置算（见 [`crate::clipboard::model::preview::geometry`]）：
//! 卡片的位置由列表渲染时挂在卡片上的 canvas 记下。

use std::{cell::RefCell, rc::Rc, sync::Arc, time::Duration};

use gpui::{
    AnyElement, Bounds, Context, IntoElement as _, Pixels, Point, Styled as _, Subscription, Task,
    Window, canvas,
};
use kwikpaste_core::{clipboard::ClipboardFragment, settings::PreviewHoverDelayMs};
use kwikpaste_ui::theme;

use super::{ClipboardList, Pointer, ops::error_toast};
use crate::{
    clipboard::{
        model::preview::{self, HEADER_HEIGHT, RectF},
        source::{Preview, PreviewContentMetrics, PreviewTextView},
        view::preview::{PreviewEvent, PreviewWindow},
    },
    i18n::t,
};

/// 离开卡片后等这么久再关（1.x `HOVER_HIDE_BUFFER_MS`），留时间把指针挪进预览窗。
const HIDE_BUFFER: Duration = Duration::from_millis(240);
/// 键盘预览换当前项后等卡片布局稳定再打开。
const FOLLOW_DELAY: Duration = Duration::from_millis(32);

/// 预览是怎么打开的。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewTrigger {
    Hover,
    /// 按住空格。
    Keyboard,
    /// 松开空格时指针在预览窗里：之后与悬停一样，离开预览窗再关。
    Held,
}

impl PreviewTrigger {
    fn pointer(self) -> bool {
        matches!(self, Self::Hover | Self::Held)
    }
}

/// 卡片在面板窗口里的位置（渲染时由 canvas 记下）。
pub(super) type Anchors = Rc<RefCell<Vec<(Arc<str>, Bounds<Pixels>)>>>;

/// 列表里的预览状态。
#[derive(Default)]
pub(super) struct Previewing {
    window: Option<PreviewWindow>,
    /// 这台机器上建不了预览窗（macOS 还没做，或建窗失败）。
    unavailable: bool,
    session: Option<(Arc<str>, PreviewTrigger)>,
    /// 每次打开、关闭都加一，晚到的结果按它作废。
    request: u64,
    hover_timer: Option<Task<()>>,
    hide_timer: Option<Task<()>>,
    follow_timer: Option<Task<()>>,
    pointer_inside: bool,
    pub(super) anchors: Anchors,
    /// 面板里最后的指针位置（悬停预览朝指针所在的一半弹出）。
    pub(super) pointer: Option<Point<Pixels>>,
    _subscription: Option<Subscription>,
}

impl Previewing {
    fn anchor(&self, id: &str) -> Option<Bounds<Pixels>> {
        self.anchors
            .borrow()
            .iter()
            .find(|(anchor, _)| **anchor == *id)
            .map(|(_, bounds)| *bounds)
    }
}

/// 挂在卡片上、记下卡片位置的 canvas。
pub(super) fn anchor_canvas(id: Arc<str>, anchors: Anchors) -> AnyElement {
    canvas(
        move |bounds, _, _| {
            let mut anchors = anchors.borrow_mut();
            anchors.retain(|(anchor, _)| *anchor != id);
            anchors.push((id.clone(), bounds));
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

fn hover_delay(delay: PreviewHoverDelayMs) -> Duration {
    Duration::from_millis(match delay {
        PreviewHoverDelayMs::Ms300 => 300,
        PreviewHoverDelayMs::Ms500 => 500,
        PreviewHoverDelayMs::Ms1000 => 1000,
    })
}

impl ClipboardList {
    /// 正在预览的记录与打开方式。
    pub fn preview_session(&self) -> Option<(Arc<str>, PreviewTrigger)> {
        self.previewing.session.clone()
    }

    /// 预览窗当前是否可见（自测用）。
    pub fn preview_visible(&self) -> bool {
        self.previewing
            .window
            .as_ref()
            .is_some_and(|window| window.native.is_visible())
    }

    /// 预览窗是不是前台窗口（自测核对“不抢前台”）。
    pub fn preview_foreground(&self) -> bool {
        self.previewing
            .window
            .as_ref()
            .is_some_and(|window| window.native.is_foreground())
    }

    /// 要记下位置的卡片：悬停的、当前项、正在预览的。
    pub(super) fn wants_anchor(&self, id: &Arc<str>, active: bool) -> bool {
        active
            || self.hovered.as_ref() == Some(id)
            || self
                .previewing
                .session
                .as_ref()
                .is_some_and(|(session, _)| session == id)
    }

    fn ensure_preview_window(&mut self, cx: &mut Context<Self>) -> bool {
        if self.previewing.window.is_some() {
            return true;
        }
        if self.previewing.unavailable {
            return false;
        }

        match PreviewWindow::open(cx) {
            Ok(window) => {
                let native = window.native.clone();
                cx.spawn(async move |_, _| native.install()).detach();
                self.previewing._subscription = Some(
                    cx.subscribe(&window.panel, |list, _, event: &PreviewEvent, cx| {
                        list.on_preview_event(event, cx)
                    }),
                );
                self.previewing.window = Some(window);
                true
            }
            Err(err) => {
                log::warn!("the preview window is unavailable: {err:#}");
                self.previewing.unavailable = true;
                false
            }
        }
    }

    fn on_preview_event(&mut self, event: &PreviewEvent, cx: &mut Context<Self>) {
        match event {
            PreviewEvent::Pointer(inside) => {
                self.previewing.pointer_inside = *inside;
                if *inside {
                    self.previewing.hide_timer = None;
                } else {
                    self.schedule_preview_hide(cx);
                }
            }
            PreviewEvent::TextView(view) => self.switch_preview_text_view(*view, cx),
            PreviewEvent::Words { paste } => self.use_preview_words(*paste, cx),
        }
    }

    /// 指针进出卡片（悬停预览）。
    pub(super) fn preview_hover(&mut self, id: &Arc<str>, hovered: bool, cx: &mut Context<Self>) {
        if !hovered {
            self.schedule_preview_hide(cx);
            return;
        }
        if !self.visible || self.pointer != Pointer::Moved || self.selection.active() {
            return;
        }

        let preview = &self.settings.clipboard.preview;
        self.previewing.hide_timer = None;
        match self.previewing.session.clone() {
            Some((session, PreviewTrigger::Keyboard)) => {
                if session != *id {
                    self.open_preview(id.clone(), PreviewTrigger::Keyboard, cx);
                }
                return;
            }
            Some((session, _)) if preview.hover_enabled => {
                if session != *id {
                    self.open_preview(id.clone(), PreviewTrigger::Hover, cx);
                }
                return;
            }
            _ => {}
        }
        if !preview.hover_enabled {
            return;
        }

        let delay = hover_delay(preview.hover_delay_ms);
        let target = id.clone();
        self.previewing.hover_timer = Some(cx.spawn(async move |list, cx| {
            cx.background_executor().timer(delay).await;
            list.update(cx, |list, cx| {
                list.previewing.hover_timer = None;
                let still_here = list.hovered.as_ref() == Some(&target);
                if still_here && list.visible && list.settings.clipboard.preview.hover_enabled {
                    list.open_preview(target, PreviewTrigger::Hover, cx);
                }
            })
            .ok();
        }));
    }

    /// 悬停式预览在指针离开卡片和预览窗 240 ms 后关上。
    fn schedule_preview_hide(&mut self, cx: &mut Context<Self>) {
        self.previewing.hover_timer = None;
        let pointer_session = self
            .previewing
            .session
            .as_ref()
            .is_some_and(|(_, trigger)| trigger.pointer());
        if !pointer_session {
            return;
        }

        self.previewing.hide_timer = Some(cx.spawn(async move |list, cx| {
            cx.background_executor().timer(HIDE_BUFFER).await;
            list.update(cx, |list, cx| {
                list.previewing.hide_timer = None;
                if list.previewing.pointer_inside {
                    return;
                }
                let over_session_card = match (&list.hovered, &list.previewing.session) {
                    (Some(hovered), Some((session, _))) => hovered == session,
                    _ => false,
                };
                if !over_session_card {
                    list.close_preview(cx);
                }
            })
            .ok();
        }));
    }

    /// 空格按下 / 松开（设置“按住空格预览”打开时）。
    pub(super) fn preview_space(&mut self, down: bool, cx: &mut Context<Self>) {
        if !self.settings.clipboard.preview.space_enabled {
            return;
        }
        if down {
            if matches!(self.previewing.session, Some((_, PreviewTrigger::Keyboard))) {
                return;
            }
            if let Some(item) = self.active_item() {
                self.open_preview(item.id.clone(), PreviewTrigger::Keyboard, cx);
            }
            return;
        }

        let Some((id, PreviewTrigger::Keyboard)) = self.previewing.session.clone() else {
            return;
        };
        if self.previewing.pointer_inside {
            self.previewing.session = Some((id, PreviewTrigger::Held));
        } else {
            self.close_preview(cx);
        }
    }

    /// 键盘预览时换了当前项：等卡片布局稳定后换内容。
    pub(super) fn preview_follow_active(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.previewing.session, Some((_, PreviewTrigger::Keyboard))) {
            return;
        }

        self.previewing.follow_timer = Some(cx.spawn(async move |list, cx| {
            cx.background_executor().timer(FOLLOW_DELAY).await;
            list.update(cx, |list, cx| {
                list.previewing.follow_timer = None;
                let keyboard =
                    matches!(list.previewing.session, Some((_, PreviewTrigger::Keyboard)));
                if let (true, Some(item)) = (keyboard, list.active_item()) {
                    list.open_preview(item.id.clone(), PreviewTrigger::Keyboard, cx);
                }
            })
            .ok();
        }));
    }

    /// 打开（或换成）`id` 的预览：取数据，按卡片位置算几何，再在借用之外显示原生窗口。
    pub fn open_preview(&mut self, id: Arc<str>, trigger: PreviewTrigger, cx: &mut Context<Self>) {
        if !self.ensure_preview_window(cx) {
            return;
        }
        self.previewing.request += 1;
        let request = self.previewing.request;
        self.previewing.session = Some((id.clone(), trigger));
        self.previewing.hover_timer = None;
        let future = self.source.preview(id.clone());
        let window = self.window;

        cx.spawn(async move |list, cx| {
            let result = future.await;
            let placed = window
                .update(cx, |_, window, cx| {
                    list.update(cx, |list, cx| {
                        if list.previewing.request != request {
                            return None;
                        }
                        let preview = match result {
                            Ok(preview) => preview,
                            Err(err) => {
                                log::warn!("preview of {id} is unavailable: {err:#}");
                                None
                            }
                        };
                        list.place_preview(&id, trigger, preview, window, cx)
                    })
                    .ok()
                    .flatten()
                })
                .ok()
                .flatten();
            if let Some((native, client, dpi)) = placed {
                native.show(client, dpi);
            }
        })
        .detach();
    }

    /// 关掉预览（返回是否有预览开着）。
    pub fn close_preview(&mut self, cx: &mut Context<Self>) -> bool {
        self.previewing.hover_timer = None;
        self.previewing.hide_timer = None;
        self.previewing.follow_timer = None;
        self.previewing.pointer_inside = false;
        self.previewing.request += 1;
        let was_open = self.previewing.session.take().is_some();
        if !was_open {
            return false;
        }

        if let Some(window) = &self.previewing.window {
            let native = window.native.clone();
            let panel = window.panel.clone();
            window
                .handle
                .update(cx, |_, window, cx| {
                    panel.update(cx, |panel, cx| panel.release(window, cx));
                })
                .ok();
            cx.spawn(async move |_, _| native.hide()).detach();
        }
        true
    }

    /// 对这一条做了操作（粘贴、复制、删除……）：正在预览它就关掉（1.x 各处的 `closePreview`）。
    pub(super) fn close_preview_of(&mut self, id: &str, cx: &mut Context<Self>) {
        let previewing = self
            .previewing
            .session
            .as_ref()
            .is_some_and(|(session, _)| &**session == id);
        if previewing {
            self.close_preview(cx);
        }
    }

    /// 悬停式预览随滚动关上（键盘预览不关）。
    pub(super) fn close_pointer_preview(&mut self, cx: &mut Context<Self>) {
        self.previewing.hover_timer = None;
        let pointer = self
            .previewing
            .session
            .as_ref()
            .is_some_and(|(_, trigger)| trigger.pointer());
        if pointer {
            self.close_preview(cx);
        }
    }

    /// 预览里选中的词（Enter / Mod+C 作用于它们，1.x `getPreviewWordsFragment`）。
    pub fn preview_words(&self, cx: &gpui::App) -> Option<(Arc<str>, Vec<usize>)> {
        let (id, _) = self.previewing.session.as_ref()?;
        let panel = self.previewing.window.as_ref()?.panel.read(cx);
        if panel.item_id() != Some(&**id) {
            return None;
        }
        let words = panel.selected_words();
        (!words.is_empty()).then(|| (id.clone(), words))
    }

    /// 粘贴或复制预览里选中的词，然后关掉预览（1.x `pasteSelection` / `copySelection`）。
    pub fn use_preview_words(&mut self, paste: bool, cx: &mut Context<Self>) {
        let Some((id, indices)) = self.preview_words(cx) else {
            return;
        };
        let fragment = ClipboardFragment::Words { indices };
        if paste {
            log::info!("preview words paste requested for {id}");
            self.close_preview(cx);
            let task = self.host.paste_fragment(id, fragment, cx);
            self.report_host_failure("commands:labels.paste", task, cx);
            return;
        }

        let task = self.host.copy_fragment(id, fragment, cx);
        let window = self.window;
        cx.spawn(async move |_, cx| {
            let result = task.await;
            window
                .update(cx, |_, window, cx| match result {
                    Ok(()) => kwikpaste_ui::toast::show(
                        kwikpaste_ui::toast::Toast::success(t("commands:messages.copied")),
                        window,
                        cx,
                    ),
                    Err(err) => error_toast("commands:labels.copy", &err, window, cx),
                })
                .ok();
        })
        .detach();
    }

    /// 预览设置变了：关掉了悬停 / 空格预览就关掉对应的预览，其余（文本方式）按新设置重开。
    pub(super) fn preview_settings_changed(&mut self, cx: &mut Context<Self>) {
        let preview = &self.settings.clipboard.preview;
        match self.previewing.session.clone() {
            Some((_, PreviewTrigger::Hover)) if !preview.hover_enabled => {
                self.close_preview(cx);
            }
            Some((_, PreviewTrigger::Keyboard)) if !preview.space_enabled => {
                self.close_preview(cx);
            }
            Some((id, trigger)) => self.open_preview(id, trigger, cx),
            None => {}
        }
    }

    /// 切换预览的文本方式：写进设置，当前预览按新方式重开（尺寸也跟着变）。
    fn switch_preview_text_view(&mut self, view: PreviewTextView, cx: &mut Context<Self>) {
        self.settings.clipboard.preview.text_view = view;
        let future = self.source.set_preview_text_view(view);
        cx.spawn(async move |list, cx| {
            let result = future.await;
            list.update(cx, |list, cx| {
                if let Err(err) = result {
                    log::warn!("preview text view could not be saved: {err:#}");
                }
                if let Some((id, trigger)) = list.previewing.session.clone() {
                    list.open_preview(id, trigger, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// 按卡片位置算出预览窗在屏幕上的位置，换好内容，返回要显示的原生窗口与位置。
    #[cfg(target_os = "windows")]
    fn place_preview(
        &mut self,
        id: &Arc<str>,
        trigger: PreviewTrigger,
        preview: Option<Preview>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<(
        Rc<crate::clipboard::view::preview::native::NativePreview>,
        kwikpaste_os::geometry::Rect,
        u32,
    )> {
        use crate::clipboard::view::preview::native;

        let Some(bounds) = self.previewing.anchor(id) else {
            log::debug!("preview of {id} has no card on screen");
            self.close_preview(cx);
            return None;
        };
        let origin = native::client_origin(window)?;
        let scale = f64::from(window.scale_factor());
        let to_screen = |value: Pixels| f64::from(value.as_f32()) * scale;
        let card_left = f64::from(origin.x) + to_screen(bounds.origin.x);
        let card_top = f64::from(origin.y) + to_screen(bounds.origin.y);
        let card_width = to_screen(bounds.size.width);
        let card_height = to_screen(bounds.size.height);
        let center = kwikpaste_os::geometry::Point {
            x: (card_left + card_width / 2.) as i32,
            y: (card_top + card_height / 2.) as i32,
        };
        let monitor = native::monitor_at(center)?;
        let monitor_scale = monitor.scale();
        let card = RectF {
            left: (card_left - f64::from(monitor.monitor.left)) / monitor_scale,
            top: (card_top - f64::from(monitor.monitor.top)) / monitor_scale,
            width: card_width / monitor_scale,
            height: card_height / monitor_scale,
        };
        let screen = RectF {
            left: 0.,
            top: 0.,
            width: f64::from(monitor.monitor.width()) / monitor_scale,
            height: f64::from(monitor.monitor.height()) / monitor_scale,
        };
        let prefer_left = trigger.pointer()
            && self.previewing.pointer.is_some_and(|pointer| {
                pointer.x.as_f32() < window.viewport_size().width.as_f32() / 2.
            });
        let text_scale = f64::from(theme::text_scale(cx));
        let geometry = preview::geometry(
            card,
            screen,
            preview.as_ref().map(|preview| &preview.metrics),
            prefer_left,
            text_scale,
        );
        let image_box = preview
            .as_ref()
            .and_then(|preview| image_box(preview, geometry.panel, text_scale));
        let text_view = self.settings.clipboard.preview.text_view;
        let preview_window = self.previewing.window.as_ref()?;
        preview_window
            .panel
            .update(cx, |panel, cx| panel.set(preview, text_view, image_box, cx));

        let to_physical = |value: f64| (value * monitor_scale).round() as i32;
        let left = monitor.monitor.left + to_physical(geometry.panel.left);
        let top = monitor.monitor.top + to_physical(geometry.panel.top);
        let client = kwikpaste_os::geometry::Rect {
            left,
            top,
            right: left + to_physical(geometry.panel.width),
            bottom: top + to_physical(geometry.panel.height),
        };

        Some((preview_window.native.clone(), client, monitor.dpi))
    }

    #[cfg(target_os = "macos")]
    fn place_preview(
        &mut self,
        _: &Arc<str>,
        _: PreviewTrigger,
        _: Option<Preview>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<(
        Rc<crate::clipboard::view::preview::native::NativePreview>,
        kwikpaste_os::geometry::Rect,
        u32,
    )> {
        None
    }
}

/// 图片在面板里的显示尺寸：等比缩进面板内容区（左右各 16、上下各 16 的内边距，头部 48）。
fn image_box(preview: &Preview, panel: RectF, scale: f64) -> Option<(f32, f32)> {
    let PreviewContentMetrics::Image {
        width: Some(width),
        height: Some(height),
    } = preview.metrics
    else {
        return None;
    };
    if width <= 0. || height <= 0. {
        return None;
    }

    let room_width = (panel.width - 32. * scale).max(1.);
    let room_height = (panel.height - (HEADER_HEIGHT + 32.) * scale).max(1.);
    let fit = 1_f64.min(room_width / width).min(room_height / height);
    Some(((width * fit) as f32, (height * fit) as f32))
}
