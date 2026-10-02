//! 剪贴板面板：启动时以隐藏状态预创建、永不销毁；显示、隐藏、定位只走原生调用，从不激活。
//!
//! 热键、托盘、第二实例和 UI 自己的请求都送进同一个 channel，由一个 `cx.spawn` 循环按顺序执行。
//! 原生调用都在 `cx.update` 之外：GPUI 的窗口过程因此能同步拿到 `App`，`ShowWindow` 发出的
//! `WM_SHOWWINDOW` 会当场画出首帧，改位置、改尺寸的回调也不会因为借用冲突被丢掉。

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Context as _;
use async_channel::{Receiver, Sender};
use gpui::{
    AnyView, AnyWindowHandle, App, AppContext as _, AsyncApp, Bounds, Context, Entity,
    EventEmitter, FocusHandle, Global, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, Styled as _, Window, WindowBackgroundAppearance, WindowBounds, WindowKind,
    WindowOptions, div, point, px, size,
};
use kwikpaste_os::clock;

use super::native::NativePanel;
use super::probe;

/// 面板的默认、最小内容区尺寸（逻辑像素），与 1.x 相同；Windows 上再乘系统「文本大小」。
pub const PANEL_SIZE: (f64, f64) = (360.0, 600.0);

/// 显示 / 隐藏请求的来源。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TriggerSource {
    Hotkey,
    Tray,
    SecondInstance,
    Selftest,
}

impl TriggerSource {
    pub fn name(self) -> &'static str {
        match self {
            Self::Hotkey => "hotkey",
            Self::Tray => "tray",
            Self::SecondInstance => "second-instance",
            Self::Selftest => "selftest",
        }
    }
}

/// 一次请求的来源和发出时刻（[`clock::now_ticks`]），用来量“触发到首帧”。
#[derive(Debug, Clone, Copy)]
pub struct Trigger {
    pub source: TriggerSource,
    pub ticks: i64,
}

impl Trigger {
    pub fn now(source: TriggerSource) -> Self {
        Self {
            source,
            ticks: clock::now_ticks(),
        }
    }
}

/// 送给面板循环的命令。
#[derive(Debug, Clone, Copy)]
pub enum PanelCommand {
    Toggle(Trigger),
    Show(Trigger),
    Hide(Trigger),
}

/// 面板可见性变化，从 [`PanelRoot`] 发出。
///
/// `Shown` 在原生显示之前、与重置焦点同一次 update 里发出，订阅者在这里重置的视图状态会进首帧；
/// `Hidden` 在原生隐藏之后发出，订阅者据此停止刷新、释放缓存。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelEvent {
    Shown,
    Hidden,
}

/// 面板窗口的根视图：包一层 UI 的视图，提供兜底焦点和首帧计时。
pub struct PanelRoot {
    content: AnyView,
    focus: FocusHandle,
}

impl EventEmitter<PanelEvent> for PanelRoot {}

impl Render for PanelRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        frame_rendered();

        div()
            .track_focus(&self.focus)
            .size_full()
            .child(self.content.clone())
    }
}

/// 面板的全局句柄。
pub struct Panel {
    root: Entity<PanelRoot>,
    commands: Sender<PanelCommand>,
}

impl Global for Panel {}

impl Panel {
    pub fn commands(&self) -> Sender<PanelCommand> {
        self.commands.clone()
    }

    /// 请求显示、隐藏或切换面板，在下一轮主循环里执行。
    pub fn request(&self, command: PanelCommand) {
        if self.commands.try_send(command).is_err() {
            log::warn!("panel command loop has stopped; dropped {command:?}");
        }
    }

    /// 面板根视图，订阅 [`PanelEvent`] 用。
    pub fn root(&self) -> &Entity<PanelRoot> {
        &self.root
    }
}

thread_local! {
    static FIRST_FRAME: Cell<Option<Sender<i64>>> = const { Cell::new(None) };
}

static RENDERED_FRAMES: AtomicU64 = AtomicU64::new(0);

/// 面板根视图渲染过的帧数。
pub fn rendered_frames() -> u64 {
    RENDERED_FRAMES.load(Ordering::Relaxed)
}

fn frame_rendered() {
    RENDERED_FRAMES.fetch_add(1, Ordering::Relaxed);
    if let Some(sender) = FIRST_FRAME.take() {
        let _ = sender.try_send(clock::now_ticks());
    }
}

/// 下一次渲染面板根视图时，把渲染时刻送进返回的 channel。
fn arm_first_frame() -> Receiver<i64> {
    let (sender, receiver) = async_channel::bounded(1);
    FIRST_FRAME.set(Some(sender));
    receiver
}

fn window_options() -> WindowOptions {
    let panel_size = size(px(PANEL_SIZE.0 as f32), px(PANEL_SIZE.1 as f32));

    WindowOptions {
        // 隐藏建窗时 GPUI 不应用这里的位置；每次显示都由原生代码重设几何。
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(
            point(px(0.), px(0.)),
            panel_size,
        ))),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: true,
        is_resizable: true,
        is_minimizable: false,
        // 面板从不激活；默认值会把非活动窗口的动画压到 30 fps。面板因此不能放常驻动画。
        inactive_frame_interval: None,
        window_min_size: Some(panel_size),
        window_background: WindowBackgroundAppearance::Opaque,
        ..Default::default()
    }
}

/// 以隐藏状态创建面板，挂上全局句柄并启动命令循环；返回 UI 的视图。
pub fn open<V: Render>(
    cx: &mut App,
    build: impl FnOnce(&mut Window, &mut App) -> Entity<V>,
) -> anyhow::Result<Entity<V>> {
    let mut native = None;
    let mut content = None;
    let (window, root) = kwikpaste_ui::open_window(window_options(), cx, |window, cx| {
        native = Some(NativePanel::attach(window));
        let view = build(window, cx);
        content = Some(view.clone());
        let focus = cx.focus_handle();
        cx.new(|_| PanelRoot {
            content: view.into(),
            focus,
        })
    })?;
    let native = native.context("the panel window was not built")??;
    let content = content.context("the panel window was not built")?;

    let (commands, receiver) = async_channel::unbounded();
    cx.set_global(Panel {
        root: root.clone(),
        commands,
    });
    cx.spawn(async move |cx| run(native, window, root, receiver, cx).await)
        .detach();

    Ok(content)
}

async fn run(
    native: NativePanel,
    window: AnyWindowHandle,
    root: Entity<PanelRoot>,
    commands: Receiver<PanelCommand>,
    cx: &mut AsyncApp,
) {
    if let Err(err) = native.install() {
        log::error!("panel native setup failed: {err:#}");
    }
    probe::ready(&native);

    while let Ok(command) = commands.recv().await {
        let visible = native.is_visible();
        let (want_visible, trigger) = match command {
            PanelCommand::Toggle(trigger) => (!visible, trigger),
            PanelCommand::Show(trigger) => (true, trigger),
            PanelCommand::Hide(trigger) => (false, trigger),
        };

        match (want_visible, visible) {
            (true, false) => show(&native, window, &root, trigger, cx),
            (true, true) => native.raise(),
            (false, true) => hide(&native, &root, trigger, cx),
            (false, false) => {}
        }
    }
}

fn show(
    native: &NativePanel,
    window: AnyWindowHandle,
    root: &Entity<PanelRoot>,
    trigger: Trigger,
    cx: &mut AsyncApp,
) {
    let placement = native
        .place_near_cursor()
        .inspect_err(|err| log::error!("panel placement failed, showing in place: {err:#}"))
        .ok();

    let prepared = cx.update(|cx| {
        window.update(cx, |_, window, cx| {
            root.update(cx, |_, cx| cx.emit(PanelEvent::Shown));
            if window.focused(cx).is_none() {
                let focus = root.read(cx).focus.clone();
                window.focus(&focus, cx);
            }
            window.refresh();
        })
    });
    if let Err(err) = prepared {
        log::error!("panel could not prepare its first frame: {err:#}");
    }

    let first_frame = arm_first_frame();
    let show_started = clock::now_ticks();
    native.show();
    let show_returned = clock::now_ticks();
    if let Some(placement) = &placement {
        native.verify(placement);
    }

    let report = ShowReport {
        trigger,
        show_started,
        show_returned,
        native_fields: probe::enabled().then(|| native.probe_fields(placement.as_ref())),
    };
    cx.foreground_executor()
        .spawn(async move {
            if let Ok(rendered) = first_frame.recv().await {
                report.finish(rendered);
            }
        })
        .detach();
}

fn hide(native: &NativePanel, root: &Entity<PanelRoot>, trigger: Trigger, cx: &mut AsyncApp) {
    native.hide();
    cx.update(|cx| root.update(cx, |_, cx| cx.emit(PanelEvent::Hidden)));

    if probe::enabled() {
        probe::hidden(trigger, &native.probe_fields(None));
    }
}

/// 一次显示从触发到首帧的计时。
struct ShowReport {
    trigger: Trigger,
    show_started: i64,
    show_returned: i64,
    native_fields: Option<String>,
}

impl ShowReport {
    /// `rendered` 是首帧渲染时刻。首帧在 `ShowWindow` 里同步画完时，它返回就已经呈现，
    /// 以返回时刻为准；否则以渲染时刻为准（之后紧跟着呈现，差不到 1 ms）。
    fn finish(self, rendered: i64) {
        let inside_show = rendered <= self.show_returned;
        let frame = if inside_show {
            self.show_returned
        } else {
            rendered
        };
        let latency = clock::ticks_to_ms(frame - self.trigger.ticks);
        log::debug!(
            "panel shown by {} in {latency:.1} ms (first frame {} the native show)",
            self.trigger.source.name(),
            if inside_show { "inside" } else { "after" }
        );

        if let Some(native_fields) = self.native_fields {
            probe::shown(
                self.trigger,
                [self.show_started, self.show_returned, rendered, frame],
                latency,
                inside_show,
                &native_fields,
            );
        }
    }
}
