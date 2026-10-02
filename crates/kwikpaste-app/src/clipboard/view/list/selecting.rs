//! 多选（与 1.3.8 一致）：点击勾选、Shift 连选、Mod+A 全选 / 取消全选、批量删除。
//!
//! 受删除保护的收藏、置顶记录勾不上，第一次碰到时提示可以去设置里放开（同一条提示只留一份）。
//! 连选两端都已加载时就地取中间的行，否则向数据源要当前视图的完整顺序再截取；全选也是。
//! 请求在路上时换了视图（换代）就作废。

use std::sync::Arc;

use gpui::{Context, Window};
use kwikpaste_ui::{
    ConfirmSpec, confirm,
    toast::{self, Toast},
};

use super::ClipboardList;
use crate::{
    clipboard::{
        model::{
            item::{ItemRef, ListItem},
            selection::{loaded_range, range_in},
        },
        source::ListQuery,
    },
    i18n::{t, t_count},
};

/// 受保护提示的 key：连续碰到时原地刷新，不叠出多条（1.x `PROTECTED_HINT_KEY`）。
const PROTECTED_HINT_KEY: &str = "clipboard-selection-protected";

impl ClipboardList {
    pub fn selecting(&self) -> bool {
        self.selection.active()
    }

    pub fn checked_count(&self) -> usize {
        self.selection.count()
    }

    pub fn enter_selection(&mut self, cx: &mut Context<Self>) {
        self.selection.enter();
        cx.notify();
    }

    pub fn exit_selection(&mut self, cx: &mut Context<Self>) {
        if self.selection.active() {
            self.selection.exit();
            cx.notify();
        }
    }

    fn show_protected_hint(window: &mut Window, cx: &mut Context<Self>) {
        toast::show(
            Toast::info(t("clipboard:selection.protected")).key(PROTECTED_HINT_KEY),
            window,
            cx,
        );
    }

    fn deletable(&self, refs: Vec<ItemRef>) -> Vec<Arc<str>> {
        refs.into_iter()
            .filter(|item| self.can_delete(item.is_favorite, item.is_pinned))
            .map(|item| item.id)
            .collect()
    }

    /// 当前视图的查询（全选、跨页连选向数据源要完整顺序时用）。
    fn view_query(&self) -> ListQuery {
        ListQuery {
            offset: 0,
            limit: 0,
            filter: self.filter().clone(),
            sort: self.settings.clipboard.content.sort,
        }
    }

    /// 切换一条的勾选，并把它记为连选起点。
    pub fn toggle_checked(&mut self, item: &ListItem, window: &mut Window, cx: &mut Context<Self>) {
        if !self.can_delete(item.is_favorite, item.is_pinned) {
            Self::show_protected_hint(window, cx);
            return;
        }

        self.selection.toggle(&item.id);
        cx.notify();
    }

    /// Shift 点击：勾上起点到目标之间（含两端）能删的记录，起点不变。
    pub fn check_range(
        &mut self,
        target: Arc<ListItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(anchor) = self
            .selection
            .anchor()
            .filter(|anchor| **anchor != target.id)
            .cloned()
        else {
            self.toggle_checked(&target, window, cx);
            return;
        };

        if let Some(range) = loaded_range(&self.model, &anchor, &target.id) {
            self.check_refs(range, window, cx);
            return;
        }

        let token = self.selection.token();
        let future = self.source.item_refs(self.view_query());
        self.selection.set_busy(true);
        cx.notify();
        cx.spawn_in(window, async move |list, cx| {
            let result = future.await;
            list.update_in(cx, |list, window, cx| {
                list.selection.set_busy(false);
                cx.notify();
                if token != list.selection.token() {
                    return;
                }
                match result {
                    Ok(refs) => match range_in(&refs, &anchor, &target.id) {
                        Some(range) => list.check_refs(range, window, cx),
                        None => list.toggle_checked(&target, window, cx),
                    },
                    Err(err) => {
                        Self::toast_error("commands:labels.selectClipboardItems", &err, window, cx)
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn check_refs(&mut self, refs: Vec<ItemRef>, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.deletable(refs);
        if ids.is_empty() {
            Self::show_protected_hint(window, cx);
            return;
        }

        self.selection.add(ids);
        cx.notify();
    }

    /// Mod+A / 全选按钮：全选当前视图里能删的记录，已全选时全部取消；还没进入多选时顺带进入。
    pub fn toggle_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.selection.enter();
        if self.selection.all_checked() {
            self.selection.reset();
            cx.notify();
            return;
        }

        let token = self.selection.token();
        let future = self.source.item_refs(self.view_query());
        self.selection.set_busy(true);
        cx.notify();
        cx.spawn_in(window, async move |list, cx| {
            let result = future.await;
            list.update_in(cx, |list, window, cx| {
                list.selection.set_busy(false);
                cx.notify();
                if token != list.selection.token() {
                    return;
                }
                match result {
                    Ok(refs) => {
                        let any = !refs.is_empty();
                        let ids = list.deletable(refs);
                        if ids.is_empty() {
                            if any {
                                Self::show_protected_hint(window, cx);
                            }
                            return;
                        }
                        list.selection.check_all(ids);
                    }
                    Err(err) => {
                        Self::toast_error("commands:labels.selectClipboardItems", &err, window, cx)
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// 删除已勾选的记录：总是二次确认；成功后退出多选，按新数据刷新列表；取消或失败时保留勾选。
    pub fn delete_checked(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let count = self.selection.count();
        if count == 0 || self.selection.busy() {
            return;
        }

        let count = i64::try_from(count).unwrap_or(i64::MAX);
        let answer = confirm(
            ConfirmSpec::new(t("commands:batchDeleteConfirm.title"))
                .content(t_count("commands:batchDeleteConfirm.content", count, &[]))
                .ok_text(t("common:actions.delete"))
                .cancel_text(t("common:actions.cancel"))
                .danger(),
            window,
            cx,
        );
        cx.spawn_in(window, async move |list, cx| {
            if !answer.await.unwrap_or(false) {
                return;
            }
            list.update_in(cx, |list, window, cx| list.delete_checked_now(window, cx))
                .ok();
        })
        .detach();
    }

    fn delete_checked_now(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let future = self.source.delete_many(self.selection.ids());
        self.selection.set_busy(true);
        cx.notify();

        cx.spawn_in(window, async move |list, cx| {
            let result = future.await;
            list.update_in(cx, |list, window, cx| {
                list.selection.set_busy(false);
                match result {
                    Ok(removed) => {
                        let removed = i64::try_from(removed).unwrap_or(i64::MAX);
                        toast::show(
                            Toast::success(t_count("commands:messages.itemsDeleted", removed, &[])),
                            window,
                            cx,
                        );
                        list.selection.exit();
                        list.controller.clear_selection();
                        list.refresh_after_removal(cx);
                    }
                    Err(err) => Self::toast_error("commands:labels.delete", &err, window, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// 批量删除后刷新：先用第一页替换整份缓存，第一页落地后再补视图范围（1.x `refreshAfterRemoval`）。
    fn refresh_after_removal(&mut self, cx: &mut Context<Self>) {
        let request = self.model.refresh_after_removal();
        self.refetch_view_after_first_page = true;
        self.fetch(request, cx);
    }
}
