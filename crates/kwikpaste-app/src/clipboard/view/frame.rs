//! 每帧的布局快照与计时。
//!
//! [`ListFrame`] 包在 `list(ListState)` 外面：列表 prepaint 一结束就从 `ListState` 读出这一帧真正要画的
//! 几何（可见行、逻辑顶部、视口），存进共享的 [`Snapshot`]。下一帧的 render 用它算加载范围、
//! 第一个可见行、平均行高（附录 D L3：不依赖只在滚轮时触发、还报滚动前范围的 scroll handler）。
//! 跑分时它顺便记下列表 prepaint / paint 的耗时；[`FrameTimer`] 包在整个视图外，记 render→paint 的总耗时。

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Instant,
};

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, GlobalElementId, InspectorElementId, IntoElement,
    LayoutId, ListState, Pixels, Window, px,
};

/// 一行在视口里的位置。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RowGeom {
    /// 列表行号，也是模型下标（包含置顶行）。
    pub ix: usize,
    /// 行顶相对视口顶的偏移（px，可为负）。
    pub top: f32,
    pub height: f32,
}

/// 最近一次布局的快照。
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// 每取一次快照加 1。
    pub seq: u64,
    pub rows: Vec<RowGeom>,
    /// 逻辑顶部：（行号，行内偏移）。
    pub top: (usize, f32),
    pub viewport: Bounds<Pixels>,
    /// 滚动条口径的滚动距离与内容总高（px）。
    pub scroll_px: f32,
    pub content_height: f32,
}

impl Snapshot {
    /// 从 `ListState` 读出当前几何。只在 prepaint 之后调用，这时读到的就是这一帧要画的样子。
    pub fn capture(state: &ListState, seq: u64) -> Self {
        let viewport = state.viewport_bounds();
        let top = state.logical_scroll_top();
        let mut rows = Vec::new();
        if viewport.size.height > px(0.) {
            let count = state.item_count();
            let mut ix = top.item_ix;
            // 视口最多容纳几十行，设个上限防止异常数据下死循环。
            while ix < count && rows.len() < 256 {
                let Some(bounds) = state.bounds_for_item(ix) else {
                    break;
                };
                if bounds.top() >= viewport.bottom() {
                    break;
                }
                rows.push(RowGeom {
                    ix,
                    top: (bounds.top() - viewport.top()).as_f32(),
                    height: bounds.size.height.as_f32(),
                });
                ix += 1;
            }
        }

        Self {
            seq,
            rows,
            top: (top.item_ix, top.offset_in_item.as_f32()),
            viewport,
            scroll_px: -state.scroll_px_offset_for_scrollbar().y.as_f32(),
            content_height: state.max_offset_for_scrollbar().y.as_f32()
                + viewport.size.height.as_f32(),
        }
    }

    /// 完整落在视口里的行（自测判断“露出来了没有”用）。
    pub fn fully_visible(&self, ix: usize) -> bool {
        let height = self.viewport.size.height.as_f32();
        self.rows
            .iter()
            .find(|row| row.ix == ix)
            .is_some_and(|row| row.top >= -0.5 && row.top + row.height <= height + 0.5)
    }
}

/// 跑分用的单帧计时（微秒）。
#[derive(Clone, Copy, Debug, Default)]
pub struct FrameTiming {
    pub render_start: Option<Instant>,
    pub list_prepaint_us: f64,
    pub list_paint_us: f64,
    /// 视图 render 开始到整个视图 paint 结束。
    pub span_us: f64,
    pub items_rendered: u32,
    pub placeholders_rendered: u32,
}

/// 宽度变化时当场重新施加的 hint（附录 D L2）。
///
/// `list` 在宽度变化的那一帧把所有行的测量和 hint 清空，滚动条总高塌到只剩可见行；等下一帧
/// render 再施加的话，中间那一帧（以及之后没有新帧时的整段静止期）滚动条都是错的。列表 prepaint
/// 一结束就在这里补上，同一帧里后画的滚动条拿到的就是正确的总高。
pub struct WidthHint {
    /// 最近一次施加 hint 时的宽度，与视图共用。
    pub width: Rc<Cell<Option<Pixels>>>,
    pub estimate: Pixels,
}

/// 包住列表元素：prepaint 后补施加 hint、取快照，可选记录耗时。
pub struct ListFrame {
    child: AnyElement,
    state: ListState,
    snapshot: Rc<RefCell<Snapshot>>,
    timing: Option<Rc<RefCell<FrameTiming>>>,
    hint: Option<WidthHint>,
}

impl ListFrame {
    pub fn new(
        child: AnyElement,
        state: ListState,
        snapshot: Rc<RefCell<Snapshot>>,
        timing: Option<Rc<RefCell<FrameTiming>>>,
        hint: Option<WidthHint>,
    ) -> Self {
        Self {
            child,
            state,
            snapshot,
            timing,
            hint,
        }
    }
}

impl IntoElement for ListFrame {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for ListFrame {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        self.child.prepaint(window, cx);
        if let Some(timing) = &self.timing {
            timing.borrow_mut().list_prepaint_us += started.elapsed().as_secs_f64() * 1e6;
        }

        if let Some(hint) = &self.hint {
            let width = self.state.viewport_bounds().size.width;
            if width > px(0.) && hint.width.get() != Some(width) {
                // ListState 是 Rc 句柄，对克隆调用 builder 改的是同一份状态。
                let _ = self.state.clone().with_uniform_item_height(hint.estimate);
                hint.width.set(Some(width));
            }
        }

        let mut snapshot = self.snapshot.borrow_mut();
        let seq = snapshot.seq + 1;
        *snapshot = Snapshot::capture(&self.state, seq);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let started = Instant::now();
        self.child.paint(window, cx);
        if let Some(timing) = &self.timing {
            timing.borrow_mut().list_paint_us += started.elapsed().as_secs_f64() * 1e6;
        }
    }
}

/// 整帧画完时的回调。
pub type PaintedCallback = Box<dyn FnOnce(&FrameTiming)>;

/// 包住整个视图，paint 结束时记下从 render 开始算起的总耗时（跑分用）。
pub struct FrameTimer {
    child: AnyElement,
    timing: Rc<RefCell<FrameTiming>>,
    on_painted: Option<PaintedCallback>,
}

impl FrameTimer {
    pub fn new(
        child: AnyElement,
        timing: Rc<RefCell<FrameTiming>>,
        on_painted: Option<PaintedCallback>,
    ) -> Self {
        Self {
            child,
            timing,
            on_painted,
        }
    }
}

impl IntoElement for FrameTimer {
    type Element = Self;

    fn into_element(self) -> Self {
        self
    }
}

impl Element for FrameTimer {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);

        let mut timing = self.timing.borrow_mut();
        if let Some(started) = timing.render_start {
            timing.span_us = started.elapsed().as_secs_f64() * 1e6;
        }
        if let Some(on_painted) = self.on_painted.take() {
            on_painted(&timing);
        }
    }
}
