//! 列表滚动条：gpui-component 的覆盖式滚动条接在 `ListState` 上。
//!
//! 对应 1.x 的 OverlayScrollbars（`autoHide: "move"`）：滚动、拖动或指针移到轨道上时出现，停下约 2 s 后淡出；
//! 滑块颜色由主题映射到 antd 的 quaternary / tertiary 文字色。拖动走 `ListState` 的
//! `scrollbar_drag_started` / `set_offset_from_scrollbar` / `scrollbar_drag_ended`，拖动期间总高冻结。

use gpui::{App, ElementId, IntoElement, ListState, RenderOnce, Window};
use gpui_component::scroll::{Scrollbar, ScrollbarMode};

/// 覆盖在 `list(ListState)` 上的竖向滚动条。放在列表的同一个 `relative` 容器里，铺满容器。
#[derive(IntoElement)]
pub struct ListScrollbar {
    id: ElementId,
    state: ListState,
}

impl ListScrollbar {
    /// `id` 在同一窗口内必须唯一。
    pub fn new(id: impl Into<ElementId>, state: &ListState) -> Self {
        Self {
            id: id.into(),
            state: state.clone(),
        }
    }
}

impl RenderOnce for ListScrollbar {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        Scrollbar::vertical(&self.state)
            .id(self.id)
            .mode(ScrollbarMode::Scrolling)
    }
}
