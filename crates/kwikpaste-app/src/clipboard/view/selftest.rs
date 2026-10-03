//! 主窗口的交互自测（`--selftest-panel-ui`）与截图用的演示状态（`--selftest-list-demo` 加
//! `KP_PANEL_DEMO`）。
//!
//! 交互自测在示例夹具上跑一段脚本：按键经 `Window::dispatch_keystroke` 派发给面板窗口，与 Windows
//! 钩子转来的按键走同一条路；修饰键经 `ModifiersChanged`。界面发给面板的命令（进出编辑态、隐藏）
//! 只记进 [`RequestLog`] 不执行，脚本再自己发出相应的面板事件，所以不会取前台、不会隐藏面板，
//! 也不碰剪贴板（夹具数据源不写系统剪贴板）。每一步都检查状态，最后以退出码报告：0 通过，1 失败。

use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use gpui::{
    AnyWindowHandle, App, AsyncApp, Capslock, Entity, Focusable as _, Keystroke, Modifiers,
    ModifiersChangedEvent, MouseMoveEvent, PlatformInput, point, px,
};
use kwikpaste_ui::{close_dialog, has_dialog};

use super::{
    editing::RequestLog,
    list::{ClipboardList, ListIntent},
    panel::ClipboardPanel,
};
use crate::{
    clipboard::{
        model::{
            actions::DeletePolicy,
            controller::ListUpdate,
            empty_state::empty_text,
            filter::{ListFilter, Range},
            item::{ItemKind, ListItem},
        },
        source::{ClipboardSource, FixtureStore, ListQuery},
    },
    platform::{EditTrigger, Panel, PanelCommand, PanelEvent},
};

/// 等列表数据、防抖等异步结果的上限。
const SETTLE: Duration = Duration::from_secs(5);

/// 跑交互脚本。
pub fn run(panel: Entity<ClipboardPanel>, store: Arc<Mutex<FixtureStore>>, cx: &mut App) {
    cx.set_global(RequestLog::default());
    let intents: Rc<RefCell<Vec<ListIntent>>> = Rc::default();
    let list = panel.read(cx).list().clone();
    let sink = intents.clone();
    cx.subscribe(&list, move |_, intent: &ListIntent, _| {
        sink.borrow_mut().push(intent.clone());
    })
    .detach();

    let window = list.read(cx).window_handle();
    cx.spawn(async move |cx| {
        let mut driver = Driver {
            panel,
            list,
            window,
            intents,
            store: Some(store),
            passed: 0,
            failed: Vec::new(),
        };
        driver.script(cx).await;

        let passed = driver.passed;
        let failed = driver.failed.len();
        for failure in &driver.failed {
            log::error!("panel ui selftest FAILED: {failure}");
        }
        log::info!("panel ui selftest: {passed} passed, {failed} failed");
        std::process::exit(i32::from(failed > 0));
    })
    .detach();
}

struct Driver {
    panel: Entity<ClipboardPanel>,
    list: Entity<ClipboardList>,
    window: AnyWindowHandle,
    intents: Rc<RefCell<Vec<ListIntent>>>,
    /// 夹具后端：模拟 core 在面板隐藏、显示前后存入新记录。
    store: Option<Arc<Mutex<FixtureStore>>>,
    passed: usize,
    failed: Vec<String>,
}

impl Driver {
    fn check(&mut self, name: &str, ok: bool, detail: impl FnOnce() -> String) {
        if ok {
            self.passed += 1;
            log::info!("panel ui selftest: ok   {name}");
        } else {
            let detail = detail();
            log::error!("panel ui selftest: FAIL {name}: {detail}");
            self.failed.push(format!("{name}: {detail}"));
        }
    }

    async fn pause(&self, cx: &mut AsyncApp, ms: u64) {
        cx.background_executor()
            .timer(Duration::from_millis(ms))
            .await;
    }

    /// 等到 `done` 成立（每 16 ms 看一次），超时返回 false。
    async fn settle(&self, cx: &mut AsyncApp, done: impl Fn(&ClipboardList, &App) -> bool) -> bool {
        let started = Instant::now();
        while started.elapsed() < SETTLE {
            let list = self.list.clone();
            if cx.update(|cx| done(list.read(cx), cx)) {
                return true;
            }
            self.pause(cx, 16).await;
        }
        false
    }

    fn read<R>(&self, cx: &mut AsyncApp, read: impl FnOnce(&ClipboardList, &App) -> R) -> R {
        let list = self.list.clone();
        cx.update(|cx| read(list.read(cx), cx))
    }

    /// 派发一个按键（GPUI keystroke 字符串），与钩子转来的按键同路。
    fn key(&self, cx: &mut AsyncApp, keystroke: &str) {
        let Ok(keystroke) = Keystroke::parse(keystroke) else {
            log::error!("bad keystroke {keystroke}");
            return;
        };
        self.window
            .update(cx, |_, window, cx| {
                window.dispatch_keystroke(keystroke, cx);
            })
            .ok();
    }

    /// 按下 / 松开平台修饰键（Windows 的 Ctrl、macOS 的 ⌘）。
    fn modifier(&self, cx: &mut AsyncApp, down: bool) {
        let modifiers = Modifiers {
            control: down && !cfg!(target_os = "macos"),
            platform: down && cfg!(target_os = "macos"),
            ..Modifiers::default()
        };
        self.window
            .update(cx, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                        modifiers,
                        capslock: Capslock::default(),
                    }),
                    cx,
                );
            })
            .ok();
    }

    /// 代替平台层发出面板事件。
    fn emit(&self, cx: &mut AsyncApp, event: PanelEvent) {
        cx.update(|cx| {
            if let Some(panel) = cx.try_global::<Panel>() {
                panel.events().clone().update(cx, |_, cx| cx.emit(event));
            }
        });
    }

    fn last_request(&self, cx: &mut AsyncApp) -> Option<PanelCommand> {
        cx.update(|cx| {
            cx.try_global::<RequestLog>()
                .and_then(|log| log.commands.last().cloned())
        })
    }

    fn dialog_open(&self, cx: &mut AsyncApp) -> bool {
        self.window
            .update(cx, |_, window, cx| has_dialog(window, cx))
            .unwrap_or(false)
    }

    fn list_focused(&self, cx: &mut AsyncApp) -> bool {
        let list = self.list.clone();
        self.window
            .update(cx, |_, window, cx| {
                list.read(cx).focus_handle(cx).is_focused(window)
            })
            .unwrap_or(false)
    }

    fn search_focused(&self, cx: &mut AsyncApp) -> bool {
        let header = cx.update(|cx| self.panel.read(cx).header().clone());
        self.window
            .update(cx, |_, window, cx| {
                header.read(cx).input().is_focused(window, cx)
            })
            .unwrap_or(false)
    }

    fn source(&self, cx: &mut AsyncApp) -> Arc<dyn ClipboardSource> {
        self.read(cx, |list, _| list.source.clone())
    }

    /// 数据源里符合条件的条数（脚本的期望值，不写死在脚本里）。
    async fn expected(&self, cx: &mut AsyncApp, filter: ListFilter) -> usize {
        let source = self.source(cx);
        source
            .list(ListQuery {
                offset: 0,
                limit: 1,
                filter,
                sort: Default::default(),
            })
            .await
            .map(|page| page.total)
            .unwrap_or(usize::MAX)
    }

    async fn filtered(&mut self, cx: &mut AsyncApp, name: &str, filter: ListFilter) {
        let want = self.expected(cx, filter.clone()).await;
        let ok = self
            .settle(cx, |list, _| {
                *list.filter() == filter && list.model.loaded_initial() && list.total() == want
            })
            .await;
        let (got, total) = self.read(cx, |list, _| (list.filter().clone(), list.total()));
        self.check(name, ok, || {
            format!("want {filter:?} with {want} rows, got {got:?} with {total}")
        });
    }

    fn active_index(&self, cx: &mut AsyncApp) -> usize {
        self.read(cx, |list, _| list.controller.active_index(&list.model))
    }

    /// 选中第一条满足条件的已加载记录，返回它的 id。
    fn select_where(
        &self,
        cx: &mut AsyncApp,
        wanted: impl Fn(&crate::clipboard::model::item::ListItem) -> bool,
    ) -> Option<Arc<str>> {
        let list = self.list.clone();
        cx.update(|cx| {
            list.update(cx, |list, cx| {
                let id = (0..list.total())
                    .filter_map(|index| list.model.get(index))
                    .find(|item| wanted(item))
                    .map(|item| item.id.clone())?;
                list.controller.select(&id);
                cx.notify();
                Some(id)
            })
        })
    }

    async fn script(&mut self, cx: &mut AsyncApp) {
        let loaded = self
            .settle(cx, |list, _| {
                list.model.loaded_initial() && list.total() > 0
            })
            .await;
        self.check("loads the sample fixture", loaded, || "no rows".into());
        if !loaded {
            return;
        }
        let all = self.read(cx, |list, _| list.total());

        // 方向键：从第一个可见的非置顶项往下走一行。
        let before = self.active_index(cx);
        self.key(cx, "down");
        let after = self.active_index(cx);
        self.check("down moves the active row", after == before + 1, || {
            format!("{before} -> {after}")
        });

        // 范围：Mod+Q 在全部与收藏之间切换。
        self.key(cx, "secondary-q");
        self.filtered(
            cx,
            "mod+q shows favorites",
            ListFilter {
                range: Range::Favorite,
                ..ListFilter::default()
            },
        )
        .await;
        self.key(cx, "secondary-q");
        self.filtered(cx, "mod+q back to all", ListFilter::default())
            .await;

        // 分类：←/→ 循环，Esc 先退分类。
        self.key(cx, "right");
        self.filtered(
            cx,
            "right selects text",
            ListFilter {
                category: Some(ItemKind::Text),
                ..ListFilter::default()
            },
        )
        .await;
        self.key(cx, "right");
        self.key(cx, "left");
        self.key(cx, "left");
        self.filtered(
            cx,
            "left wraps to files",
            ListFilter {
                category: Some(ItemKind::Files),
                ..ListFilter::default()
            },
        )
        .await;
        self.key(cx, "escape");
        self.filtered(cx, "escape clears the category", ListFilter::default())
            .await;

        // 自定义分组：Tab / Shift+Tab 在可见分组间循环（隐藏的跳过），Esc 先退分组。
        self.key(cx, "tab");
        let work = ListFilter {
            group_id: Some("demo-work".into()),
            ..ListFilter::default()
        };
        self.filtered(cx, "tab selects the first group", work.clone())
            .await;
        self.key(cx, "tab");
        self.filtered(
            cx,
            "tab skips the hidden group",
            ListFilter {
                group_id: Some("demo-code".into()),
                ..ListFilter::default()
            },
        )
        .await;
        self.key(cx, "shift-tab");
        self.filtered(cx, "shift-tab goes back", work).await;
        self.key(cx, "escape");
        self.filtered(cx, "escape clears the group", ListFilter::default())
            .await;

        self.search(cx, all).await;
        self.hints(cx).await;
        self.item_actions(cx).await;
        self.multi_select(cx).await;
        self.note(cx).await;
        self.snippet(cx).await;
        self.show_then_enter(cx).await;
        self.escape_layers(cx).await;
    }

    /// 把指针挪到窗口里的某个位置（逻辑像素），像光标停在那里一样。
    fn pointer_at(&self, cx: &mut AsyncApp, x: f32, y: f32) {
        self.window
            .update(cx, |_, window, cx| {
                window.dispatch_event(
                    PlatformInput::MouseMove(MouseMoveEvent {
                        position: point(px(x), px(y)),
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }),
                    cx,
                );
            })
            .ok();
    }

    /// 像 core 存入新记录那样：夹具里插到置顶块之后，列表收到 `ClipboardUpserted`。
    fn store_new_record(&self, cx: &mut AsyncApp, template: &ListItem, id: &str) {
        let Some(store) = &self.store else {
            return;
        };
        let item = ListItem {
            id: id.into(),
            summary: Some(format!("新记录 {id}").into()),
            note: None,
            is_pinned: false,
            is_favorite: false,
            group_id: None,
            ..template.clone()
        };
        if let Ok(mut store) = store.lock() {
            store.insert_newest(item);
        }
        self.list.update(cx, |list, cx| {
            list.on_update(
                ListUpdate::Upserted {
                    kind: ItemKind::Text,
                    deduplicated: false,
                },
                cx,
            );
        });
    }

    /// 显示面板后立刻按 Enter（平台线的复现：粘的是上一条）。新记录在面板隐藏时或刚显示时到达；
    /// 一半的轮次面板出现在静止的光标下（首帧之后补一次指针移动，光标下是旧的第一行）；夹具查询慢
    /// 60 ms，Enter 一定赶在刷新落地之前。
    /// 粘贴的必须是新记录，刷新落地后当前项也必须是第一个非置顶行。
    async fn show_then_enter(&mut self, cx: &mut AsyncApp) {
        let template = self.read(cx, |list, _| {
            (0..list.total())
                .filter_map(|index| list.model.get(index))
                .find(|item| item.kind == ItemKind::Text)
                .map(|item| (**item).clone())
        });
        let Some(template) = template else {
            self.check("show then enter: a text record to copy", false, || {
                "no text record".into()
            });
            return;
        };

        let rounds = 8;
        let mut pasted_newest = 0;
        let mut active_newest = 0;
        let mut details = Vec::new();
        for round in 0..rounds {
            let id = format!("selftest-fresh-{round}");
            let after_show = round % 2 == 1;
            let over_card = round % 4 < 2;

            self.emit(cx, PanelEvent::Hidden);
            self.pause(cx, 30).await;
            self.pointer_at(cx, 20., 20.);
            if !after_show {
                self.store_new_record(cx, &template, &id);
            }
            self.intents.borrow_mut().clear();
            self.emit(cx, PanelEvent::Shown);
            if after_show {
                self.store_new_record(cx, &template, &id);
            }
            if over_card {
                // 面板出现在静止的光标下时系统会补发一次指针移动：等首帧画出（数据还是旧的），
                // 再在第一个非置顶行（两张置顶卡片之下）的位置补一次移动，然后立刻按 Enter。
                self.pause(cx, 20).await;
                self.pointer_at(cx, 180., 360.);
            }
            self.key(cx, "enter");

            let intents = self.intents.clone();
            let started = Instant::now();
            while started.elapsed() < SETTLE && intents.borrow().is_empty() {
                self.pause(cx, 10).await;
            }
            let pasted = intents.borrow().first().cloned();
            if pasted
                == Some(ListIntent::Paste {
                    id: id.clone().into(),
                    plain: false,
                })
            {
                pasted_newest += 1;
            } else {
                details.push(format!("round {round}: pasted {pasted:?}"));
            }

            let wanted = id.clone();
            let caught_up = self
                .settle(cx, move |list, _| {
                    list.model.loaded_initial()
                        && list.active_item().is_some_and(|item| *item.id == *wanted)
                })
                .await;
            if caught_up {
                active_newest += 1;
            } else {
                let active =
                    self.read(cx, |list, _| list.active_item().map(|item| item.id.clone()));
                details.push(format!("round {round}: active {active:?}"));
            }
        }

        self.check(
            "enter right after showing pastes the newest record",
            pasted_newest == rounds,
            || format!("{pasted_newest}/{rounds}: {}", details.join("; ")),
        );
        self.check(
            "after showing the active row is the newest record",
            active_newest == rounds,
            || format!("{active_newest}/{rounds}: {}", details.join("; ")),
        );
        self.pointer_at(cx, 20., 20.);
    }

    /// 点快捷信息：默认“双击粘贴”时粘贴这个片段（宿主是夹具替身，只核对意图）。
    async fn snippet(&mut self, cx: &mut AsyncApp) {
        let target = self.read(cx, |list, _| {
            (0..list.total())
                .filter_map(|index| list.model.get(index))
                .find_map(|item| {
                    item.quick_snippets
                        .first()
                        .map(|text| (item.clone(), text.clone()))
                })
        });
        let Some((item, text)) = target else {
            self.check("a record with quick snippets", false, || "none".into());
            return;
        };

        self.intents.borrow_mut().clear();
        let list = self.list.clone();
        let (pick_item, pick_text) = (item.clone(), text.clone());
        self.window
            .update(cx, |_, window, cx| {
                list.update(cx, |list, cx| {
                    list.pick_snippet(pick_item, pick_text, window, cx)
                });
            })
            .ok();
        let pasted = self.intents.borrow().first().cloned();
        self.check(
            "clicking a quick snippet pastes it",
            pasted
                == Some(ListIntent::PasteSnippet {
                    id: item.id.clone(),
                    text: text.clone(),
                }),
            || format!("{pasted:?}"),
        );
    }

    async fn search(&mut self, cx: &mut AsyncApp, all: usize) {
        // Mod+F 请求编辑态（键盘触发）；编辑态开始后输入框拿到焦点。
        self.key(cx, "secondary-f");
        let request = self.last_request(cx);
        self.check(
            "mod+f requests keyboard editing",
            matches!(
                request,
                Some(PanelCommand::BeginEditing(EditTrigger::Keyboard))
            ),
            || format!("{request:?}"),
        );
        self.emit(cx, PanelEvent::EditingStarted);
        self.pause(cx, 30).await;
        let focused = self.search_focused(cx);
        self.check("editing focuses the search box", focused, || {
            "search box not focused".into()
        });

        // 输入经 200 ms 防抖、去首尾空白后成为搜索词。
        let header = cx.update(|cx| self.panel.read(cx).header().clone());
        self.window
            .update(cx, |_, window, cx| {
                header.update(cx, |header, cx| {
                    header.type_text("  kwikpaste ", window, cx)
                });
            })
            .ok();
        let keyword = ListFilter {
            keyword: "kwikpaste".into(),
            ..ListFilter::default()
        };
        self.filtered(cx, "typing searches after the debounce", keyword)
            .await;

        // 输入框聚焦时 ↓ 交给列表，焦点不动。
        let before = self.active_index(cx);
        self.key(cx, "down");
        let after = self.active_index(cx);
        let still = self.search_focused(cx);
        self.check(
            "down in the search box moves the list",
            after != before && still,
            || format!("{before} -> {after}, focused {still}"),
        );

        // Esc 退出编辑态，焦点回到列表。
        self.key(cx, "escape");
        let request = self.last_request(cx);
        self.check(
            "escape in the search box ends editing",
            matches!(request, Some(PanelCommand::EndEditing)),
            || format!("{request:?}"),
        );
        self.emit(cx, PanelEvent::EditingEnded);
        self.pause(cx, 30).await;
        let focused = self.list_focused(cx);
        self.check("ending editing focuses the list", focused, || {
            "list not focused".into()
        });

        // 没有结果时显示搜索空态。
        self.window
            .update(cx, |_, window, cx| {
                header.update(cx, |header, cx| {
                    header.type_text("zz-no-such-record", window, cx)
                });
            })
            .ok();
        let ok = self
            .settle(cx, |list, _| {
                &*list.filter().keyword == "zz-no-such-record" && list.model.loaded_initial()
            })
            .await;
        let (total, key) = self.read(cx, |list, _| (list.total(), empty_text(list.filter()).key));
        self.check(
            "an unmatched search shows the search empty state",
            ok && total == 0 && key == "clipboard:empty.searchHistory",
            || format!("total {total}, key {key}"),
        );

        self.window
            .update(cx, |_, window, cx| {
                header.update(cx, |header, cx| header.type_text("", window, cx));
            })
            .ok();
        let back = self
            .settle(cx, |list, _| {
                list.filter().keyword.is_empty()
                    && list.model.loaded_initial()
                    && list.total() == all
            })
            .await;
        self.check("clearing the search restores the list", back, || {
            "list did not come back".into()
        });
    }

    async fn hints(&mut self, cx: &mut AsyncApp) {
        self.modifier(cx, true);
        let shown = self.read(cx, |list, _| list.key_hints);
        self.check("holding the modifier shows the number hints", shown, || {
            "hints off".into()
        });

        // Mod+2 粘贴第二个可见的非置顶项（宿主接上之前是意图）。
        let target = self.read(cx, |list, _| {
            list.controller
                .hint_index('2')
                .and_then(|index| list.model.get(index))
                .map(|item| item.id.clone())
        });
        self.intents.borrow_mut().clear();
        self.key(cx, "secondary-2");
        let pasted = self.intents.borrow().first().cloned();
        self.check(
            "mod+2 pastes the second visible row",
            target.is_some()
                && pasted
                    == target
                        .clone()
                        .map(|id| ListIntent::Paste { id, plain: false }),
            || format!("want {target:?}, got {pasted:?}"),
        );

        self.modifier(cx, false);
        let hidden = self.read(cx, |list, _| !list.key_hints);
        self.check("releasing the modifier hides the hints", hidden, || {
            "hints still on".into()
        });
    }

    async fn item_actions(&mut self, cx: &mut AsyncApp) {
        // 收藏：Mod+D 翻转当前项。
        let id = self.select_where(cx, |item| !item.is_pinned && !item.is_favorite);
        self.key(cx, "secondary-d");
        let favorite = match &id {
            Some(id) => {
                let id = id.clone();
                self.settle(cx, move |list, _| {
                    list.model.find(&id).is_some_and(|item| item.is_favorite)
                })
                .await
            }
            None => false,
        };
        self.check("mod+d favorites the active row", favorite, || {
            format!("{id:?}")
        });
        self.key(cx, "secondary-d");
        if let Some(id) = id.clone() {
            let back = self
                .settle(cx, move |list, _| {
                    list.model.find(&id).is_some_and(|item| !item.is_favorite)
                })
                .await;
            self.check("mod+d again unfavorites it", back, || {
                "still favorite".into()
            });
        }

        // 置顶：Mod+T 后它进入置顶块，再按一次回去。
        let pinned_before = self.read(cx, |list, _| list.model.leading_pinned());
        self.key(cx, "secondary-t");
        let pinned = self
            .settle(cx, |list, _| {
                list.model.loaded_initial() && list.model.leading_pinned() == pinned_before + 1
            })
            .await;
        self.check("mod+t pins the active row", pinned, || {
            format!("pinned block stayed {pinned_before}")
        });
        if let Some(id) = id.clone() {
            self.list.update(cx, |list, cx| {
                list.controller.select(&id);
                cx.notify();
            });
        }
        self.key(cx, "secondary-t");
        let unpinned = self
            .settle(cx, |list, _| {
                list.model.loaded_initial() && list.model.leading_pinned() == pinned_before
            })
            .await;
        self.check("mod+t again unpins it", unpinned, || {
            "pinned block changed".into()
        });

        // 删除保护：收藏、置顶的记录按默认设置删不掉，不弹确认框。
        let total = self.read(cx, |list, _| list.total());
        let protected = self.select_where(cx, |item| item.is_pinned);
        self.key(cx, "secondary-backspace");
        let dialog = self.dialog_open(cx);
        self.check(
            "pinned rows are protected from deletion",
            protected.is_some() && !dialog,
            || format!("{protected:?}, dialog {dialog}"),
        );

        // 删除：Mod+Backspace 弹确认框，Enter 确认。
        let victim = self.select_where(cx, |item| !item.is_pinned && !item.is_favorite);
        self.key(cx, "secondary-backspace");
        self.pause(cx, 50).await;
        let dialog = self.dialog_open(cx);
        self.check("mod+backspace asks before deleting", dialog, || {
            "no confirm dialog".into()
        });
        self.key(cx, "enter");
        let deleted = match victim.clone() {
            Some(victim) => {
                self.settle(cx, move |list, _| {
                    list.total() + 1 == total && list.model.find(&victim).is_none()
                })
                .await
            }
            None => false,
        };
        self.check("enter in the dialog deletes the row", deleted, || {
            format!("{victim:?}")
        });
        let closed = !self.dialog_open(cx);
        self.check("the confirm dialog closes", closed, || "still open".into());
        // 确认框关掉后焦点交还给列表（钩子转来的按键要按列表的绑定匹配）。
        let list = self.list.clone();
        self.window
            .update(cx, |_, window, cx| {
                let focus = list.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            })
            .ok();
    }

    async fn multi_select(&mut self, cx: &mut AsyncApp) {
        let source = self.source(cx);
        let refs = source
            .item_refs(ListQuery {
                offset: 0,
                limit: 0,
                filter: ListFilter::default(),
                sort: Default::default(),
            })
            .await
            .unwrap_or_default();
        let policy = DeletePolicy::default();
        let deletable = refs
            .iter()
            .filter(|item| policy.can_delete(item.is_favorite, item.is_pinned, false))
            .count();

        // Mod+A：进入多选并全选能删的记录；再按一次取消全选。
        self.key(cx, "secondary-a");
        let all = self
            .settle(cx, |list, _| {
                list.selecting() && list.checked_count() == deletable
            })
            .await;
        let count = self.read(cx, |list, _| list.checked_count());
        self.check("mod+a selects every deletable row", all, || {
            format!("want {deletable}, got {count}")
        });
        self.key(cx, "secondary-a");
        let none = self.read(cx, |list, _| list.selecting() && list.checked_count() == 0);
        self.check("mod+a again clears the selection", none, || {
            "selection not cleared".into()
        });

        // Enter 勾选当前项（受保护的勾不上）。
        self.select_where(cx, |item| !item.is_pinned && !item.is_favorite);
        self.key(cx, "enter");
        self.key(cx, "down");
        self.key(cx, "enter");
        let two = self.read(cx, |list, _| list.checked_count());
        self.check("enter toggles rows while selecting", two == 2, || {
            format!("{two} checked")
        });
        let total = self.read(cx, |list, _| list.total());

        // 批量删除：取消时保留勾选，确认后删除并退出多选。
        self.key(cx, "secondary-delete");
        self.pause(cx, 50).await;
        let dialog = self.dialog_open(cx);
        self.check("batch delete asks first", dialog, || "no dialog".into());
        self.key(cx, "escape");
        self.pause(cx, 50).await;
        let kept = self.read(cx, |list, _| list.selecting() && list.checked_count() == 2);
        let closed = !self.dialog_open(cx);
        self.check("cancelling keeps the selection", kept && closed, || {
            format!("kept {kept}, closed {closed}")
        });
        self.focus_list(cx);
        self.key(cx, "secondary-delete");
        self.pause(cx, 50).await;
        self.key(cx, "enter");
        let deleted = self
            .settle(cx, |list, _| {
                !list.selecting() && list.model.loaded_initial() && list.total() + 2 == total
            })
            .await;
        self.check("confirming deletes the checked rows", deleted, || {
            "rows not deleted".into()
        });
        self.focus_list(cx);
    }

    fn focus_list(&self, cx: &mut AsyncApp) {
        let list = self.list.clone();
        self.window
            .update(cx, |_, window, cx| {
                let focus = list.read(cx).focus_handle(cx);
                window.focus(&focus, cx);
            })
            .ok();
    }

    async fn note(&mut self, cx: &mut AsyncApp) {
        let id = self.select_where(cx, |item| item.note.is_none() && !item.is_pinned);
        self.key(cx, "secondary-m");
        self.pause(cx, 50).await;
        let request = self.last_request(cx);
        let dialog = self.dialog_open(cx);
        self.check(
            "mod+m opens the note dialog and asks for editing",
            dialog
                && matches!(
                    request,
                    Some(PanelCommand::BeginEditing(EditTrigger::Keyboard))
                ),
            || format!("dialog {dialog}, {request:?}"),
        );
        self.emit(cx, PanelEvent::EditingStarted);
        self.pause(cx, 30).await;

        let list = self.list.clone();
        let focused = self
            .window
            .update(cx, |_, window, cx| {
                let Some(input) = list.read(cx).note_input() else {
                    return false;
                };
                input.set_value("  自测备注 ", window, cx);
                input.focus_handle(cx).is_focused(window)
            })
            .unwrap_or(false);
        self.check("editing focuses the note box", focused, || {
            "note box not focused".into()
        });

        // 保存（等同点“保存”按钮）：关掉对话框、写入、退出编辑态。
        self.window
            .update(cx, |_, window, cx| {
                close_dialog(window, cx);
                list.update(cx, |list, cx| list.finish_note(true, window, cx));
            })
            .ok();
        let saved = match id.clone() {
            Some(id) => {
                self.settle(cx, move |list, _| {
                    list.model
                        .find(&id)
                        .is_some_and(|item| item.note.as_deref() == Some("自测备注"))
                })
                .await
            }
            None => false,
        };
        let request = self.last_request(cx);
        self.check(
            "saving trims the note and ends editing",
            saved && matches!(request, Some(PanelCommand::EndEditing)),
            || format!("saved {saved}, {request:?}"),
        );
        self.emit(cx, PanelEvent::EditingEnded);
        self.pause(cx, 30).await;
    }

    async fn escape_layers(&mut self, cx: &mut AsyncApp) {
        self.focus_list(cx);
        self.key(cx, "secondary-a");
        self.settle(cx, |list, _| list.selecting()).await;
        self.key(cx, "right");
        self.key(cx, "escape");
        let (selecting, category) =
            self.read(cx, |list, _| (list.selecting(), list.filter().category));
        self.check(
            "escape leaves multi-select before clearing the category",
            !selecting && category.is_some(),
            || format!("selecting {selecting}, category {category:?}"),
        );
        self.key(cx, "escape");
        self.key(cx, "escape");
        let request = self.last_request(cx);
        self.check(
            "the last escape hides the panel",
            matches!(request, Some(PanelCommand::Hide(_))),
            || format!("{request:?}"),
        );

        // 隐藏再显示：退出多选，按“打开窗口时选中”的默认设置回到全部。
        self.key(cx, "secondary-q");
        self.emit(cx, PanelEvent::Hidden);
        self.emit(cx, PanelEvent::Shown);
        self.filtered(cx, "showing resets the range to all", ListFilter::default())
            .await;
    }
}

/// 截图用的演示状态（`KP_PANEL_DEMO`）：`hover`、`hints`、`selection`、`search-empty`、`group`、
/// `note`、`delete`。数据加载完后摆好，供 PrintWindow 截图。
pub fn stage_demo(panel: Entity<ClipboardPanel>, cx: &mut App) {
    let Ok(stage) = std::env::var("KP_PANEL_DEMO") else {
        return;
    };
    cx.set_global(RequestLog::default());
    let list = panel.read(cx).list().clone();
    let window = list.read(cx).window_handle();

    cx.spawn(async move |cx| {
        let driver = Driver {
            panel,
            list,
            window,
            intents: Rc::default(),
            store: None,
            passed: 0,
            failed: Vec::new(),
        };
        driver
            .settle(cx, |list, _| {
                list.model.loaded_initial() && list.total() > 0
            })
            .await;
        driver.stage(&stage, cx).await;
        log::info!("demo stage {stage} ready");
    })
    .detach();
}

impl Driver {
    async fn stage(&self, stage: &str, cx: &mut AsyncApp) {
        match stage {
            "hover" => {
                let id =
                    self.select_where(cx, |item| item.kind == ItemKind::Text && !item.is_pinned);
                self.list.update(cx, |list, cx| {
                    list.hovered = id;
                    cx.notify();
                });
            }
            "hints" => self.modifier(cx, true),
            "selection" => {
                self.key(cx, "secondary-a");
                self.settle(cx, |list, _| list.checked_count() > 0).await;
                self.key(cx, "secondary-a");
                self.select_where(cx, |item| !item.is_pinned && !item.is_favorite);
                self.key(cx, "enter");
                self.key(cx, "down");
                self.key(cx, "enter");
            }
            "search-empty" => {
                let header = cx.update(|cx| self.panel.read(cx).header().clone());
                self.window
                    .update(cx, |_, window, cx| {
                        header.update(cx, |header, cx| header.type_text("没有这条", window, cx));
                    })
                    .ok();
            }
            "group" => {
                self.key(cx, "tab");
                self.key(cx, "right");
            }
            "group-empty" => {
                self.key(cx, "tab");
                self.key(cx, "secondary-q");
                self.key(cx, "left");
            }
            "note" => {
                self.select_where(cx, |item| item.kind == ItemKind::Text && !item.is_pinned);
                self.key(cx, "secondary-m");
                self.pause(cx, 50).await;
                self.emit(cx, PanelEvent::EditingStarted);
            }
            "delete" => {
                self.select_where(cx, |item| !item.is_pinned && !item.is_favorite);
                self.key(cx, "secondary-backspace");
            }
            other => log::warn!("unknown demo stage {other}"),
        }
    }
}
