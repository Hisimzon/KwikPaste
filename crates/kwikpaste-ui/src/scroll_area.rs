//! 竖向滚动区域：内容超出时滚动，带与列表相同的覆盖式滚动条（1.x 的 `ScrollArea`）。
//!
//! 滚动位置由调用方持有的 `ScrollHandle` 记着，需要时可以程序滚动（例如滚到要高亮的设置行）。

use gpui::{
    AnyElement, App, Div, ElementId, InteractiveElement as _, IntoElement, ParentElement,
    RenderOnce, ScrollHandle, StatefulInteractiveElement as _, StyleRefinement, Styled, Window,
    div,
};
use gpui_component::scroll::{Scrollbar, ScrollbarMode};

/// 竖向滚动区域。尺寸由外层决定（通常 `flex_1` + `min_h_0`），内容区的样式写在它自己身上。
#[derive(IntoElement)]
pub struct ScrollArea {
    id: ElementId,
    handle: ScrollHandle,
    content: Div,
}

impl ScrollArea {
    /// `id` 在同一窗口内必须唯一。
    pub fn new(id: impl Into<ElementId>, handle: &ScrollHandle) -> Self {
        Self {
            id: id.into(),
            handle: handle.clone(),
            content: div(),
        }
    }
}

impl ParentElement for ScrollArea {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.content.extend(elements);
    }
}

/// 样式作用在内容上（内边距、间距、最大宽度）。
impl Styled for ScrollArea {
    fn style(&mut self) -> &mut StyleRefinement {
        self.content.style()
    }
}

impl RenderOnce for ScrollArea {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .relative()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .child(
                div()
                    .id(self.id.clone())
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.handle)
                    .child(self.content),
            )
            .child(
                Scrollbar::vertical(&self.handle)
                    .id((self.id, "scrollbar"))
                    .mode(ScrollbarMode::Scrolling),
            )
    }
}
