//! 偏好设置里可拖动排序的行（采集顺序、快捷动作管理）。

use gpui::{ElementId, InteractiveElement as _, StyleRefinement, Styled as _, div};
use kwikpaste_ui::theme::{self, SemanticTokens, space};

/// 可排序行的共同外观：把手在左侧，整行都能拖。
pub(super) fn row(id: impl Into<ElementId>, tokens: &SemanticTokens) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(space(2.))
        .px(space(3.))
        .py(space(1.5))
        .rounded(theme::radius::SM)
        .hover(|style| style.bg(tokens.fill.subtle))
        .cursor(gpui::CursorStyle::OpenHand)
}

/// 拖到这一行上方时的落点提示：主色浅底。不改边框宽度，拖动途中行高不跳。
pub(super) fn drop_target(style: StyleRefinement, tokens: &SemanticTokens) -> StyleRefinement {
    style.bg(tokens.accent.subtle).border_color(tokens.accent.solid)
}
