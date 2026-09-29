import { useMemoizedFn, useMount } from "ahooks";
import { Empty, Spin } from "antd";
import type { TFunction } from "i18next";
import type {
  FC,
  MouseEvent as ReactMouseEvent,
  PointerEvent as ReactPointerEvent,
} from "react";
import { Fragment, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Virtuoso, type VirtuosoHandle } from "react-virtuoso";
import { useSnapshot } from "valtio";
import {
  copyClipboardFragment,
  deleteClipboardItem,
  deleteClipboardItems,
  hideWindow,
  listClipboardGroups,
  listClipboardItemRefs,
  openClipboardItemLink,
  pasteClipboardFragment,
  pasteClipboardItem,
  revealClipboardItem,
  saveClipboardImageToFile,
  toggleClipboardItemFavorite,
  toggleClipboardItemPinned,
  updateClipboardItemGroup,
  writeToClipboard,
} from "@/commands";
import VirtuosoScroller, {
  type VirtuosoScrollerChildrenProps,
} from "@/components/VirtuosoScroller";
import { TAURI_EVENT } from "@/constants/events";
import { buildItemActionLabels } from "@/constants/itemActions";
import {
  parseWindowOpenGroupId,
  WINDOW_OPEN_SELECTION_ALL,
  WINDOW_OPEN_SELECTION_PRESERVE,
} from "@/constants/windowOpenSelection";
import { WINDOW_LABEL } from "@/constants/windows";
import { useClipboardItems } from "@/hooks/useClipboardItems";
import { useKeyboardEvent } from "@/hooks/useKeyboardEvent";
import { useTauriListen } from "@/hooks/useTauriListen";
import {
  clipboardSelectionState,
  enterClipboardSelection,
  exitClipboardSelection,
} from "@/stores/clipboardSelection";
import { clipboardStatsState } from "@/stores/clipboardStats";
import { clipboardViewState } from "@/stores/clipboardView";
import { settingsState } from "@/stores/settings";
import { openSplitWords } from "@/stores/splitWords";
import type {
  ClipboardAction,
  ClipboardFragment,
  ClipboardGroupRecord,
  ClipboardItem,
  ClipboardItemQuery,
  ClipboardItemRef,
  ClipboardKind,
  ClipboardRange,
} from "@/types/clipboard";
import type { ItemAction } from "@/types/settings";
import { cn } from "@/utils/cn";
import { getMessageApi } from "@/utils/feedback";
import { isMac } from "@/utils/is";
import type { WindowVisibilityPayload } from "../hooks/previewController";
import {
  isSpaceKey,
  useClipboardPreviewController,
} from "../hooks/useClipboardPreviewController";
import { useListLayout } from "../hooks/useListLayout";
import ClipboardCard from "./cards/ClipboardCard";
import NoteModal from "./NoteModal";
import SelectionBar from "./SelectionBar";

/** 前 10 项的快捷键：index 0-8 对应 1-9，index 9 对应 0 */
const KEY_HINTS = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];

/** 多选时点到受保护记录的提示只保留一条，连点不堆叠。 */
const PROTECTED_HINT_KEY = "clipboard-selection-protected";

interface ClipboardUpdatedPayload {
  cleanup?: number;
  deduplicated?: boolean;
  id?: string;
  imported?: boolean;
  kind?: ClipboardKind;
}

interface ClipboardMenuActionPayload {
  action: ClipboardAction;
  groupId?: string;
  itemId: string;
}

interface PreviewSelectionPayload {
  indices: number[];
  itemId: string;
}

/**
 * 剪贴板历史列表：虚拟滚动 + 分类型卡片 + 可视范围分页加载，
 * 跟随关键词（Header 已防抖）检索。
 */
const List: FC = () => {
  const { t } = useTranslation("clipboard");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [listStartIndex, setListStartIndex] = useState(0);
  const [isModifierPressed, setIsModifierPressed] = useState(false);
  const [customGroups, setCustomGroups] = useState<ClipboardGroupRecord[]>([]);
  const [noteTarget, setNoteTarget] = useState<ClipboardItem | null>(null);
  const [checkedIds, setCheckedIds] = useState<ReadonlySet<string>>(
    () => new Set(),
  );
  const [allChecked, setAllChecked] = useState(false);
  const [selectionBusy, setSelectionBusy] = useState(false);
  // Shift 连选的起点：最近一次单独勾选 / 取消的条目。
  const selectionAnchorIdRef = useRef<string | null>(null);
  // 勾选集合每次清空都换代，异步拉回的全选 / 连选结果属于旧视图时丢弃。
  const selectionTokenRef = useRef(0);
  const virtuosoRef = useRef<VirtuosoHandle>(null);
  const isAtTopRef = useRef(true);
  const itemElementMapRef = useRef(new Map<string, HTMLDivElement>());
  const closePreviewRef = useRef<(reason: string) => void>(() => {});
  const displaySettingsMountedRef = useRef(false);
  const keywordRef = useRef("");
  const reloadCurrentRangeRef = useRef<() => void>(() => {});
  const deferredReloadRef = useRef(false);
  // 剪贴板窗口启动即隐藏，初值取 false；首个 `window://visibility` show 事件会翻正。
  // dormant（隐藏）期间到达的剪贴板更新一律延后，不 reload 隐藏窗口。
  const clipboardWindowVisibleRef = useRef(false);

  const snapshot = useSnapshot(clipboardViewState);
  const settings = useSnapshot(settingsState);
  const { active: selecting } = useSnapshot(clipboardSelectionState);
  const { category, keyword, groupId, range } = snapshot;
  const autoPaste = settings.clipboard.content.autoPaste;
  const middleClick = settings.clipboard.content.middleClick;
  const display = settings.clipboard.display;
  const sort = settings.clipboard.content.sort;
  const redactSecrets = settings.clipboard.sensitive.redactSecrets;
  const quickActions = settings.clipboard.content.itemActions;
  const deleteFavoriteItems = settings.clipboard.content.deleteFavoriteItems;
  const deletePinnedItems = settings.clipboard.content.deletePinnedItems;
  const deleteFavoriteItemsOnlyInFavoriteGroup =
    settings.clipboard.content.deleteFavoriteItemsOnlyInFavoriteGroup;
  const { fileMaxCount, quickSnippets } = display;
  const listLayout = useListLayout();
  const showOriginalPreview = settings.clipboard.content.showOriginalPreview;
  const quickActionLabels = useMemo(() => {
    return buildItemActionLabels(t);
  }, [t]);
  const currentGroupName = getCurrentGroupName(customGroups, groupId);
  const itemQuery: ClipboardItemQuery = {
    favorite: range === "favorite" ? true : void 0,
    groupId: groupId ?? void 0,
    keyword,
    kind: category ?? void 0,
    sort,
  };

  const {
    findItemById,
    getItem,
    getItemIndexById,
    loadRange,
    loadedInitial,
    loading,
    patchItemById,
    refreshAfterRemoval,
    reload,
    reloadCurrentRange,
    removeItemById,
    total,
  } = useClipboardItems(itemQuery);
  const pinnedCount = countLeadingPinnedItems(getItem);
  // 虚拟列表只承载置顶之后的条目，其余逻辑一律用全局下标。
  const firstVisibleIndex = listStartIndex + pinnedCount;
  const {
    closeHoverPreviewForScroll,
    closePreview,
    handleItemPointerEnter,
    handleItemPointerLeave,
    handleItemPointerMove,
    handleKeyboardPreviewMove,
    handlePreviewAreaPointerLeave,
    handlePreviewSpaceDown,
    previewSession,
  } = useClipboardPreviewController({
    getActiveItem,
    itemElementMapRef,
    onHoverSelect: setSelectedId,
  });
  closePreviewRef.current = closePreview;
  reloadCurrentRangeRef.current = reloadCurrentRange;

  // 把 Rust 返回的同过滤下总数同步给 Footer（共享 store），避免 Footer 单独 IPC 计数。
  useEffect(() => {
    if (loadedInitial) clipboardStatsState.total = total;
  }, [loadedInitial, total]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: snapshot 作触发器，不需在回调内读取
  useEffect(() => {
    setSelectedId(null);
    if (keywordRef.current !== keyword) keywordRef.current = keyword;
    deferredReloadRef.current = false;
    closePreview("filterChange");
    // 换了视图就不再勾着看不见的记录，多选状态本身保留。
    resetChecked();
  }, [snapshot]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: 只在退出多选时触发，resetChecked 只写 state 与 ref
  useEffect(() => {
    if (!selecting) resetChecked();
  }, [selecting]);

  // biome-ignore lint/correctness/useExhaustiveDependencies: 仅按影响列表 payload 的展示设置触发重拉，函数引用用 ref 读取最新值
  useEffect(() => {
    if (!displaySettingsMountedRef.current) {
      displaySettingsMountedRef.current = true;
      return;
    }

    closePreviewRef.current("displaySettingChange");
    reloadCurrentRangeRef.current();
  }, [fileMaxCount, quickSnippets, redactSecrets]);

  /**
   * 从 Rust 拉取自定义分组，用于空状态展示当前分组名称。
   */
  const loadGroups = async () => {
    const groups = await listClipboardGroups();

    setCustomGroups(groups);
  };

  /**
   * 首次挂载时拉取分组名称。
   */
  useMount(() => {
    void loadGroups();
  });

  /**
   * 自定义分组变化后同步刷新空状态文案可用的分组名。
   */
  const handleGroupsUpdated = () => {
    void loadGroups();
  };

  useTauriListen(TAURI_EVENT.CLIPBOARD_GROUPS_UPDATED, handleGroupsUpdated);

  /**
   * 收到剪贴板更新：仅在列表位于顶部时刷新；否则延后到用户回到顶部后再刷新，
   * 避免打断当前浏览位置。
   * 用 ref 读取最新滚动位置，规避闭包陷旧值（事件订阅只挂载一次）。
   */
  const handleClipboardUpdated = (payload: ClipboardUpdatedPayload) => {
    // 剪贴板窗口隐藏（冻结态）期间不立即 reload：只记 pending，避免隐藏期间频繁复制触发反复 IPC + 重渲染。
    if (!clipboardWindowVisibleRef.current) {
      deferredReloadRef.current = true;
      return;
    }

    if (payload.cleanup !== void 0) {
      closePreview("cleanup");
      setSelectedId(null);
      resetChecked();
      deferredReloadRef.current = false;
      if (clipboardStatsState.total !== null) {
        clipboardStatsState.total = Math.max(
          clipboardStatsState.total - payload.cleanup,
          0,
        );
      }
      requestReloadAtTop();
      return;
    }

    if (payload.imported) {
      closePreview("backupImport");
      setSelectedId(null);
      resetChecked();
      requestReloadAtTop();
      return;
    }

    if (payload.deduplicated) {
      if (
        !shouldRefreshCurrentGroup(
          clipboardViewState.range,
          clipboardViewState.category,
          clipboardViewState.groupId,
          payload.kind,
        )
      ) {
        return;
      }

      requestReloadAtTop();
      return;
    }

    if (
      !shouldRefreshCurrentGroup(
        clipboardViewState.range,
        clipboardViewState.category,
        clipboardViewState.groupId,
        payload.kind,
      )
    ) {
      return;
    }

    requestReloadAtTop();
  };

  useTauriListen<ClipboardUpdatedPayload>(
    TAURI_EVENT.CLIPBOARD_UPDATED,
    (event) => {
      handleClipboardUpdated(event.payload);
    },
  );

  /**
   * 剪贴板窗口显隐变化：更新可见性镜像；显示时按偏好重置分组与滚动位置。
   * 可见性 ref 供 `handleClipboardUpdated` 判断是否处于冻结态——隐藏期间只记 pending，不立即 reload。
   */
  const handleWindowVisibility = (event: {
    payload: WindowVisibilityPayload;
  }) => {
    const { label, visible } = event.payload;
    if (label !== WINDOW_LABEL.CLIPBOARD) return;

    clipboardWindowVisibleRef.current = visible;
    if (!visible) {
      exitClipboardSelection();
      return;
    }

    const {
      scrollToTopOnOpen,
      selectCategoryOnOpen,
      selectGroupOnOpen,
      selectRangeOnOpen,
    } = settings.clipboard.window;
    const shouldResetSelection =
      selectRangeOnOpen !== WINDOW_OPEN_SELECTION_PRESERVE ||
      selectCategoryOnOpen !== WINDOW_OPEN_SELECTION_PRESERVE ||
      selectGroupOnOpen !== WINDOW_OPEN_SELECTION_PRESERVE;
    if (!scrollToTopOnOpen && !shouldResetSelection) return;

    closePreview("windowOpenReset");

    if (selectRangeOnOpen !== WINDOW_OPEN_SELECTION_PRESERVE) {
      clipboardViewState.range = selectRangeOnOpen;
    }

    if (selectCategoryOnOpen === WINDOW_OPEN_SELECTION_ALL) {
      clipboardViewState.category = null;
    } else if (selectCategoryOnOpen !== WINDOW_OPEN_SELECTION_PRESERVE) {
      clipboardViewState.category = selectCategoryOnOpen;
    }

    const openGroupId = parseWindowOpenGroupId(selectGroupOnOpen);
    if (selectGroupOnOpen === WINDOW_OPEN_SELECTION_ALL) {
      clipboardViewState.groupId = null;
    } else if (openGroupId) {
      clipboardViewState.groupId = openGroupId;
    }

    if (!scrollToTopOnOpen) return;

    setSelectedId(null);
    virtuosoRef.current?.scrollToIndex({ behavior: "auto", index: 0 });
    consumeDeferredReloadAtTop();
  };

  useTauriListen<WindowVisibilityPayload>(
    TAURI_EVENT.WINDOW_VISIBILITY,
    handleWindowVisibility,
  );

  /**
   * 删除 / 收藏 / 备注命令均不广播 clipboard://updated，故就地改本地镜像，
   * 避免整页 reload 打断滚动与选中态。
   */
  const removeItem = (id: string) => {
    removeItemById(id);
  };

  const patchItem = (id: string, patch: Partial<ClipboardItem>) => {
    patchItemById(id, patch);
  };

  /**
   * 收藏切换后：favorite 分组下取消收藏的条目应即时移出列表，其余分组仅更新标记。
   */
  const handleFavoriteToggled = (id: string, isFavorite: boolean) => {
    if (range === "favorite" && !isFavorite) {
      removeItem(id);
      return;
    }

    patchItem(id, { isFavorite });
  };

  /**
   * 备注保存后同步本地镜像；后端可能因 autoFavorite 设置联动收藏，故一并回填。
   */
  const handleNoteSaved = (
    id: string,
    note: string | null,
    autoFavorited: boolean,
  ) => {
    patchItem(id, autoFavorited ? { isFavorite: true, note } : { note });
  };

  const handleCloseNote = () => {
    setNoteTarget(null);
  };

  /**
   * 打开条目备注编辑框；若该条正在预览，先关闭预览避免窗口层级互相遮挡。
   */
  const handleOpenNote = (item: ClipboardItem, reason: string) => {
    if (previewSession?.itemId === item.id) closePreview(reason);

    setNoteTarget(item);
  };

  /**
   * 右键菜单移动分组后同步本地镜像；当前分组视图下移出其它分组时直接移除。
   */
  const handleMoveToGroup = async (
    item: ClipboardItem,
    nextGroupId: string,
  ) => {
    if (previewSession?.itemId === item.id) closePreview("moveToGroup");

    await updateClipboardItemGroup(item.id, nextGroupId);

    if (groupId !== null && groupId !== nextGroupId) {
      removeItem(item.id);
      return;
    }

    patchItem(item.id, { groupId: nextGroupId });
  };

  /**
   * 读取当前选中项。无显式选中时，优先取当前可视范围第一项。
   */
  function getActiveItem() {
    if (total === 0) return null;

    if (selectedId === null) {
      return getItem(firstVisibleIndex) ?? getItem(0);
    }

    return findItemById(selectedId);
  }

  /**
   * 注册虚拟列表项对应的 DOM 节点，预览打开时用它采集 anchor rect。
   */
  const registerItemElement = useMemoizedFn(
    (id: string, node: HTMLDivElement | null) => {
      if (node) {
        itemElementMapRef.current.set(id, node);
        return;
      }

      itemElementMapRef.current.delete(id);
    },
  );

  /**
   * 快捷键触发的删除：复用 `deleteClipboardItem` 内置的二次确认弹窗，
   * 仅当用户确认且 Rust 删除成功时才同步本地镜像。
   */
  const handleShortcutDelete = async (id: string) => {
    const target = findItemById(id);

    if (!target || !canDeleteItem(target)) return;

    if (previewSession?.itemId === id) closePreview("delete");

    const deleted = await deleteClipboardItem(
      id,
      target.isFavorite,
      target.isPinned,
    );

    if (!deleted) return;

    setSelectedId(
      getSelectedIdAfterDelete(
        getItem,
        getItemIndexById,
        firstVisibleIndex,
        selectedId,
        id,
      ),
    );
    removeItem(id);
  };

  /**
   * 清空勾选并换代，还在路上的全选 / 连选结果回来后作废。
   */
  function resetChecked() {
    selectionTokenRef.current += 1;
    selectionAnchorIdRef.current = null;
    setCheckedIds(new Set());
    setAllChecked(false);
  }

  /**
   * 多选时点到受保护的收藏 / 置顶记录：勾不上，提示可以去设置里放开。
   */
  function showProtectedHint() {
    getMessageApi().info({
      content: t("selection.protected"),
      key: PROTECTED_HINT_KEY,
    });
  }

  /**
   * 切换单条勾选，并把它记为 Shift 连选的起点。
   */
  function toggleChecked(item: ClipboardItem) {
    if (!canDeleteItem(item)) {
      showProtectedHint();
      return;
    }

    selectionAnchorIdRef.current = item.id;
    setAllChecked(false);
    setCheckedIds((current) => {
      const next = new Set(current);
      if (!next.delete(item.id)) next.add(item.id);

      return next;
    });
  }

  /**
   * Shift 点击：勾上起点到目标之间（含两端）所有能删的记录，起点不变。
   * 中间的行都已加载时就地取，否则向 Rust 要当前视图的完整顺序再截取。
   */
  async function checkRange(target: ClipboardItem) {
    const anchorId = selectionAnchorIdRef.current;
    if (anchorId === null || anchorId === target.id) {
      toggleChecked(target);
      return;
    }

    const token = selectionTokenRef.current;
    const refs =
      getLoadedRangeRefs(anchorId, target.id) ??
      (await fetchRangeRefs(anchorId, target.id));
    if (token !== selectionTokenRef.current) return;

    if (!refs) {
      toggleChecked(target);
      return;
    }

    const ids = getDeletableIds(refs);
    if (ids.length === 0) {
      showProtectedHint();
      return;
    }

    setAllChecked(false);
    setCheckedIds((current) => {
      return new Set([...current, ...ids]);
    });
  }

  /**
   * 两条记录之间（含两端）的已加载条目；有一端或中间某行没加载时返回 null。
   */
  function getLoadedRangeRefs(fromId: string, toId: string) {
    const fromIndex = getItemIndexById(fromId);
    const toIndex = getItemIndexById(toId);
    if (fromIndex === null || toIndex === null) return null;

    const refs: ClipboardItemRef[] = [];
    const end = Math.max(fromIndex, toIndex);
    for (let index = Math.min(fromIndex, toIndex); index <= end; index += 1) {
      const item = getItem(index);
      if (!item) return null;

      refs.push(item);
    }

    return refs;
  }

  /**
   * 从当前视图的完整顺序里截出两条记录之间（含两端）的部分；有一端已不在视图里时返回 null。
   */
  async function fetchRangeRefs(fromId: string, toId: string) {
    const refs = await loadViewRefs();
    if (!refs) return null;

    const fromIndex = refs.findIndex((ref) => {
      return ref.id === fromId;
    });
    const toIndex = refs.findIndex((ref) => {
      return ref.id === toId;
    });
    if (fromIndex === -1 || toIndex === -1) return null;

    return refs.slice(
      Math.min(fromIndex, toIndex),
      Math.max(fromIndex, toIndex) + 1,
    );
  }

  /**
   * 拉取当前视图全部记录的 id 与保护标记；失败原因命令层已提示，这里返回 null。
   */
  async function loadViewRefs() {
    setSelectionBusy(true);

    try {
      return await listClipboardItemRefs(itemQuery);
    } catch {
      return null;
    } finally {
      setSelectionBusy(false);
    }
  }

  function getDeletableIds(refs: ClipboardItemRef[]) {
    return refs.filter(canDeleteItem).map((ref) => {
      return ref.id;
    });
  }

  /**
   * 全选当前视图里能删的记录，已全选时全部取消；还没进入多选时顺带进入。
   */
  async function toggleAllChecked() {
    enterClipboardSelection();

    if (allChecked) {
      resetChecked();
      return;
    }

    const token = selectionTokenRef.current;
    const refs = await loadViewRefs();
    if (!refs || token !== selectionTokenRef.current) return;

    const ids = getDeletableIds(refs);
    if (ids.length === 0) {
      if (refs.length > 0) showProtectedHint();
      return;
    }

    selectionAnchorIdRef.current = null;
    setCheckedIds(new Set(ids));
    setAllChecked(true);
  }

  /**
   * 删除已勾选的记录：确认并删除成功后退出多选，再按新数据刷新列表；取消或失败时保留勾选。
   */
  async function deleteChecked() {
    if (checkedIds.size === 0 || selectionBusy) return;

    if (previewSession && checkedIds.has(previewSession.itemId)) {
      closePreview("batchDelete");
    }

    setSelectionBusy(true);

    let removed: number | null = null;
    try {
      removed = await deleteClipboardItems([...checkedIds]);
    } catch {
      // 失败原因命令层已提示，勾选原样保留方便重试。
    }

    setSelectionBusy(false);

    if (removed === null) return;

    exitClipboardSelection();
    setSelectedId(null);
    void refreshAfterRemoval();
  }

  /**
   * 快捷键触发的收藏切换：读当前项的 isFavorite 计算下一态，
   * Rust 返回真实状态后走统一的 `handleFavoriteToggled`（favorite 分组内取消会移除）。
   */
  const handleShortcutToggleFavorite = async (id: string) => {
    const current = findItemById(id);

    if (!current) return;

    const next = await toggleClipboardItemFavorite(id, !current.isFavorite);

    handleFavoriteToggled(id, next);
  };

  /**
   * 切换条目置顶态；置顶影响排序，成功后刷新当前范围以继续信任后端顺序。
   */
  const handleTogglePinned = async (id: string) => {
    const current = findItemById(id);

    if (!current) return;

    const next = await toggleClipboardItemPinned(id, !current.isPinned);

    patchItem(id, { isPinned: next });
    reloadCurrentRange();
  };

  /**
   * 打开拆词面板；Rust 没给出拆词动作（非文本、脱敏展示的敏感内容）时不响应。
   */
  const openSplit = (item: ClipboardItem) => {
    if (!item.availableActions?.includes("splitWords")) return;

    closePreview("splitWords");
    setSelectedId(item.id);
    openSplitWords(item.id);
  };

  /**
   * 点击卡片上的快捷信息：左键设置为复制时只复制该片段，其余情况直接粘贴到目标应用。
   */
  const pickSnippet = async (item: ClipboardItem, text: string) => {
    const fragment: ClipboardFragment = { kind: "snippet", text };

    setSelectedId(item.id);

    if (autoPaste === "singleClickCopy" || autoPaste === "doubleClickCopy") {
      if (previewSession?.itemId === item.id) closePreview("snippetCopy");

      await copyClipboardFragment(item.id, fragment);
      return;
    }

    closePreview("snippetPaste");
    await pasteClipboardFragment(item.id, fragment);
  };

  /**
   * 按当前条目后端声明的可用动作执行“打开”：链接 / 邮箱 / 定位文件共用 Cmd/Ctrl+O。
   */
  const handleShortcutOpen = async (
    item: ClipboardItem,
    action: ClipboardAction,
  ) => {
    if (previewSession?.itemId === item.id) closePreview("shortcutOpen");

    switch (action) {
      case "openLink":
        await openClipboardItemLink(item.id, false);
        return;
      case "sendEmail":
        await openClipboardItemLink(item.id, true);
        return;
      case "revealInFinder":
      case "revealInExplorer":
        await revealClipboardItem(item.id);
        return;
      default:
        return;
    }
  };

  /**
   * Rust 右键菜单点击事件：携带 `{action, itemId}`。
   * 用 ref 持续指向「当前 render 的派发函数」，规避 `useTauriListen` 只在挂载时
   * 抓一次闭包导致的状态过期（同款做法见 `handleClipboardUpdated`）。
   */
  const handleMenuActionRef = useRef<
    (payload: ClipboardMenuActionPayload) => void
  >(() => {});
  handleMenuActionRef.current = (payload) => {
    const { action, groupId: targetGroupId, itemId } = payload;
    const target = findItemById(itemId);

    if (!target) return;

    switch (action) {
      case "paste":
        closePreview("paste");
        pasteClipboardItem(target.id, false);
        return;
      case "pasteAsPlainText":
      case "pasteAsPath":
        closePreview("pastePlain");
        pasteClipboardItem(target.id, true);
        return;
      case "copy":
        if (previewSession?.itemId === target.id) closePreview("copy");
        writeToClipboard(target.id, false);
        return;
      case "saveImage":
        if (previewSession?.itemId === target.id) closePreview("saveImage");
        saveClipboardImageToFile(target.id);
        return;
      case "splitWords":
        openSplit(target);
        return;
      case "openLink":
        if (previewSession?.itemId === target.id) closePreview("openLink");
        openClipboardItemLink(target.id, false);
        return;
      case "sendEmail":
        if (previewSession?.itemId === target.id) closePreview("sendEmail");
        openClipboardItemLink(target.id, true);
        return;
      case "revealInFinder":
      case "revealInExplorer":
        if (previewSession?.itemId === target.id) closePreview("reveal");
        revealClipboardItem(target.id);
        return;
      case "toggleFavorite":
        handleShortcutToggleFavorite(target.id);
        return;
      case "togglePinned":
        handleTogglePinned(target.id);
        return;
      case "moveToGroup":
        if (!targetGroupId) return;

        void handleMoveToGroup(target, targetGroupId);
        return;
      case "editNote":
        handleOpenNote(target, "editNote");
        return;
      case "select":
        enterClipboardSelection();
        setSelectedId(target.id);
        if (canDeleteItem(target) && !checkedIds.has(target.id)) {
          toggleChecked(target);
        }
        return;
      case "delete":
        if (!canDeleteItem(target)) return;

        handleShortcutDelete(target.id);
        return;
    }
  };

  const handleMenuActionEvent = (event: { payload: unknown }) => {
    handleMenuActionRef.current(event.payload as ClipboardMenuActionPayload);
  };

  useTauriListen(TAURI_EVENT.CLIPBOARD_MENU_ACTION, handleMenuActionEvent);

  /**
   * 预览面板里选中的词；只在它属于当前预览条目时才接管 Enter / Cmd+C。
   */
  const previewSelectionRef = useRef<PreviewSelectionPayload | null>(null);

  const handlePreviewSelection = (event: {
    payload: PreviewSelectionPayload;
  }) => {
    previewSelectionRef.current = event.payload;
  };

  useTauriListen(TAURI_EVENT.PREVIEW_SELECTION, handlePreviewSelection);

  /**
   * 当前预览面板上选中的词，作为写回剪贴板的片段；没有选中时返回 null。
   */
  function getPreviewWordsFragment(): ClipboardFragment | null {
    const selection = previewSelectionRef.current;

    if (!previewSession || !selection) return null;
    if (selection.itemId !== previewSession.itemId) return null;
    if (selection.indices.length === 0) return null;

    return { indices: selection.indices, kind: "words" };
  }

  const handleKeyDown = (event: KeyboardEvent) => {
    const eventModifierPressed = isMac ? event.metaKey : event.ctrlKey;

    setIsModifierPressed(eventModifierPressed);

    if (event.key === "Escape") {
      event.preventDefault();
      closeTopEscapeLayer();

      return;
    }

    if (total === 0) return;

    if (eventModifierPressed && event.key.toLowerCase() === "a") {
      event.preventDefault();
      void toggleAllChecked();

      return;
    }

    if (selecting && handleSelectionKeyDown(event, eventModifierPressed)) {
      return;
    }

    if (event.key === "Enter") {
      event.preventDefault();

      const previewWords = getPreviewWordsFragment();

      if (previewWords && previewSession) {
        const { itemId } = previewSession;

        closePreview("enterPastePreviewWords");
        pasteClipboardFragment(itemId, previewWords);
        return;
      }

      const activeItem = getActiveItem();

      if (!activeItem) return;

      closePreview("enterPaste");
      pasteClipboardItem(activeItem.id, eventModifierPressed);

      return;
    }

    if (isSpaceKey(event)) {
      handlePreviewSpaceDown(event);
      return;
    }

    if (
      eventModifierPressed &&
      (event.key === "Backspace" || event.key === "Delete")
    ) {
      event.preventDefault();

      const activeItem = getActiveItem();

      if (!activeItem) return;

      handleShortcutDelete(activeItem.id);

      return;
    }

    if (
      eventModifierPressed &&
      event.key.toLowerCase() === "c" &&
      !shouldUseNativeCopy(event)
    ) {
      event.preventDefault();

      const previewWords = getPreviewWordsFragment();

      if (previewWords && previewSession) {
        copyClipboardFragment(previewSession.itemId, previewWords);
        return;
      }

      const activeItem = getActiveItem();

      if (!activeItem) return;

      if (previewSession?.itemId === activeItem.id)
        closePreview("shortcutCopy");

      writeToClipboard(activeItem.id, false);

      return;
    }

    if (eventModifierPressed && event.key.toLowerCase() === "o") {
      const activeItem = getActiveItem();

      if (!activeItem) return;

      const openAction = getOpenClipboardAction(activeItem.availableActions);

      if (!openAction) return;

      event.preventDefault();
      void handleShortcutOpen(activeItem, openAction);

      return;
    }

    if (eventModifierPressed && event.key.toLowerCase() === "s") {
      event.preventDefault();

      const activeItem = getActiveItem();

      if (!activeItem) return;

      openSplit(activeItem);

      return;
    }

    if (eventModifierPressed && event.key.toLowerCase() === "d") {
      event.preventDefault();

      const activeItem = getActiveItem();

      if (!activeItem) return;

      handleShortcutToggleFavorite(activeItem.id);

      return;
    }

    if (eventModifierPressed && event.key.toLowerCase() === "t") {
      event.preventDefault();

      const activeItem = getActiveItem();

      if (!activeItem) return;

      handleTogglePinned(activeItem.id);

      return;
    }

    if (eventModifierPressed && event.key.toLowerCase() === "m") {
      event.preventDefault();

      const activeItem = getActiveItem();

      if (!activeItem) return;

      handleOpenNote(activeItem, "shortcutNote");

      return;
    }

    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;

    event.preventDefault();

    const next = getNextKeyboardTarget(event);

    if (!next) return;

    setSelectedId(next.item.id);
    scrollItemIntoView(next.index);

    handleKeyboardPreviewMove(next.item);
  };

  useKeyboardEvent("keydown", handleKeyDown);

  /**
   * 多选时的按键：Enter 勾选 / 取消当前项，Cmd/Ctrl+Backspace / Delete 删除已勾选的记录，
   * 其余作用于单条记录的修饰键组合一律不响应。返回 false 的键（上下移动、空格预览）继续按列表规则处理。
   */
  function handleSelectionKeyDown(
    event: KeyboardEvent,
    modifierPressed: boolean,
  ) {
    if (event.key === "Enter") {
      event.preventDefault();

      const activeItem = getActiveItem();
      if (activeItem) toggleChecked(activeItem);

      return true;
    }

    if (!modifierPressed) return false;

    if (event.key === "Backspace" || event.key === "Delete") {
      event.preventDefault();
      void deleteChecked();
    }

    return true;
  }

  /**
   * 预览面板转交的按键：点过面板后键盘焦点可能停在预览窗口，Enter / Esc / Cmd+C / 上下键
   * 仍按列表这一套规则处理，预览里选了词时 Enter / Cmd+C 作用于选中的词。
   */
  const handlePreviewKeydown = (event: { payload: KeyboardEventInit }) => {
    handleKeyDown(
      new KeyboardEvent("keydown", { ...event.payload, cancelable: true }),
    );
  };

  useTauriListen<KeyboardEventInit>(
    TAURI_EVENT.PREVIEW_KEYDOWN,
    handlePreviewKeydown,
  );

  const handleKeyUp = (event: KeyboardEvent) => {
    const eventModifierPressed = isMac ? event.metaKey : event.ctrlKey;

    setIsModifierPressed(eventModifierPressed);
  };

  useKeyboardEvent("keyup", handleKeyUp);

  // 卡片回调对所有条目共用一份稳定引用（条目由卡片回传），hover / 选中变化时 memo 卡片不必全部重渲染。
  const handleCardPointerEnter = useMemoizedFn(
    (item: ClipboardItem, event: ReactPointerEvent<HTMLDivElement>) => {
      handleItemPointerEnter(item, event);
    },
  );

  const handleCardPointerLeave = useMemoizedFn(() => {
    handleItemPointerLeave();
  });

  const handleCardPointerMove = useMemoizedFn(
    (item: ClipboardItem, event: ReactPointerEvent<HTMLDivElement>) => {
      handleItemPointerMove(item, event);
    },
  );

  const quickPasteItem = useMemoizedFn((item: ClipboardItem) => {
    closePreview("quickPaste");
    pasteClipboardItem(item.id, false);
  });

  const openItemLink = useMemoizedFn((item: ClipboardItem) => {
    closePreview("openLink");
    openClipboardItemLink(item.id, item.subKind === "email");
  });

  const pickItemSnippet = useMemoizedFn((item: ClipboardItem, text: string) => {
    void pickSnippet(item, text);
  });

  const handleCardQuickAction = useMemoizedFn(
    async (item: ClipboardItem, action: ItemAction) => {
      if (action === "delete" && !canDeleteItem(item)) return;

      switch (action) {
        case "paste":
          closePreview("quickPaste");
          await pasteClipboardItem(item.id, false);
          return;
        case "pastePlain":
          closePreview("quickPastePlain");
          await pasteClipboardItem(item.id, true);
          return;
        case "pastePath":
          closePreview("quickPastePath");
          await pasteClipboardItem(item.id, true);
          return;
        case "copy":
          if (previewSession?.itemId === item.id) closePreview("quickCopy");
          await writeToClipboard(item.id, false);
          return;
        case "copyPlain":
          if (previewSession?.itemId === item.id) {
            closePreview("quickCopyPlain");
          }
          await writeToClipboard(item.id, true);
          return;
        case "splitWords":
          openSplit(item);
          return;
        case "openLink":
          if (previewSession?.itemId === item.id) {
            closePreview("quickOpenLink");
          }
          await openClipboardItemLink(item.id, false);
          return;
        case "sendEmail":
          if (previewSession?.itemId === item.id) {
            closePreview("quickSendEmail");
          }
          await openClipboardItemLink(item.id, true);
          return;
        case "reveal":
          if (previewSession?.itemId === item.id) closePreview("quickReveal");
          await revealClipboardItem(item.id);
          return;
        case "note":
          handleOpenNote(item, "editNote");
          return;
        case "pinItem":
          await handleTogglePinned(item.id);
          return;
        case "star":
          await handleShortcutToggleFavorite(item.id);
          return;
        case "delete":
          await handleShortcutDelete(item.id);
          return;
      }
    },
  );

  const handleCardMouseDown = useMemoizedFn(
    (item: ClipboardItem, event: ReactMouseEvent<HTMLDivElement>) => {
      if (selecting) {
        selectCardByMouse(item, event);
        return;
      }

      if (event.button !== 0) {
        if (event.button !== 1) return;

        event.preventDefault();

        if (middleClick === "singleClickPaste") {
          setSelectedId(item.id);
          closePreview("middleClickPaste");
          pasteClipboardItem(item.id, false);
          return;
        }

        if (middleClick === "singleClickPastePlain") {
          setSelectedId(item.id);
          closePreview("middleClickPastePlain");
          pasteClipboardItem(item.id, true);
          return;
        }

        if (middleClick === "singleClickCopy") {
          setSelectedId(item.id);
          closePreview("middleClickCopy");
          writeToClipboard(item.id, false);
          return;
        }

        if (middleClick === "singleClickCopyPlain") {
          setSelectedId(item.id);
          closePreview("middleClickCopyPlain");
          writeToClipboard(item.id, true);
        }

        return;
      }

      setSelectedId(item.id);

      if (autoPaste === "singleClickPaste") {
        closePreview("singleClickPaste");
        pasteClipboardItem(item.id, false);
        return;
      }

      if (autoPaste === "singleClickCopy") {
        closePreview("singleClickCopy");
        writeToClipboard(item.id, false);
      }
    },
  );

  /**
   * 多选时按下卡片：左键切换勾选，按住 Shift 连选（卡片此时不可选中文字，Shift 点击不会拉出选区）。
   * 中键不执行动作，但照样拦掉它的自动滚动。
   */
  function selectCardByMouse(
    item: ClipboardItem,
    event: ReactMouseEvent<HTMLDivElement>,
  ) {
    if (event.button === 1) event.preventDefault();
    if (event.button !== 0) return;

    setSelectedId(item.id);

    if (event.shiftKey) {
      void checkRange(item);
      return;
    }

    toggleChecked(item);
  }

  const handleCardDoubleClick = useMemoizedFn((item: ClipboardItem) => {
    if (selecting) return;

    if (autoPaste === "doubleClickPaste") {
      closePreview("doubleClickPaste");
      pasteClipboardItem(item.id, false);
      return;
    }

    if (autoPaste === "doubleClickCopy") {
      closePreview("doubleClickCopy");
      writeToClipboard(item.id, false);
    }
  });

  const handleRangeChanged = ({
    endIndex,
    startIndex,
  }: {
    startIndex: number;
    endIndex: number;
  }) => {
    setListStartIndex(startIndex);

    closeHoverPreviewForScroll();
    loadRange(startIndex + pinnedCount, endIndex + pinnedCount);
  };

  const handleAtTopStateChange = (atTop: boolean) => {
    isAtTopRef.current = atTop;

    if (!atTop) return;

    consumeDeferredReloadAtTop();
  };

  /**
   * 自动刷新请求只在顶部执行；离开顶部时保留 pending，等待回顶后消费。
   */
  function requestReloadAtTop() {
    if (!isAtTopRef.current) {
      deferredReloadRef.current = true;
      return;
    }

    deferredReloadRef.current = false;
    reload();
  }

  /**
   * 消费已有 pending；用于窗口回顶偏好或用户手动回到顶部后的补刷。
   */
  function consumeDeferredReloadAtTop() {
    if (!deferredReloadRef.current) return;

    requestReloadAtTop();
  }

  // 多选栏接替底部栏（Footer 多选时不渲染），列表加载中或为空时也要留着，才能退出多选。
  const selectionBar = selecting ? (
    <SelectionBar
      allChecked={allChecked}
      busy={selectionBusy}
      count={checkedIds.size}
      onDelete={deleteChecked}
      onExit={exitClipboardSelection}
      onToggleAll={toggleAllChecked}
    />
  ) : null;

  if (loading && !loadedInitial) {
    return (
      <>
        <div className="flex flex-1 items-center justify-center">
          <Spin />
        </div>

        {selectionBar}
      </>
    );
  }

  if (loadedInitial && total === 0) {
    const description = getEmptyDescription(
      t,
      keyword,
      range,
      category,
      groupId,
      currentGroupName,
    );

    return (
      <>
        <div
          className="flex flex-1 flex-col items-center justify-center"
          data-tauri-drag-region
        >
          <Empty
            description={description}
            image={Empty.PRESENTED_IMAGE_SIMPLE}
          />
        </div>

        {selectionBar}
      </>
    );
  }

  return (
    <>
      <div
        aria-multiselectable={selecting}
        className="relative flex flex-1 flex-col overflow-hidden"
        onPointerLeave={handlePreviewAreaPointerLeave}
        role="listbox"
      >
        {renderPinnedItems()}

        <VirtuosoScroller className="min-h-0 flex-1">
          {renderVirtuoso}
        </VirtuosoScroller>

        <NoteModal
          item={noteTarget}
          onClose={handleCloseNote}
          onSaved={handleNoteSaved}
        />
      </div>

      {selectionBar}
    </>
  );

  function renderVirtuoso(props: VirtuosoScrollerChildrenProps) {
    const { scrollerRef } = props;

    return (
      <Virtuoso
        atTopStateChange={handleAtTopStateChange}
        computeItemKey={computeListItemKey}
        itemContent={renderListItemContent}
        rangeChanged={handleRangeChanged}
        ref={virtuosoRef}
        scrollerRef={scrollerRef}
        totalCount={total - pinnedCount}
      />
    );
  }

  /**
   * 置顶项固定在滚动区上方而不是 sticky 盖在列表上：云母 / 亚克力下任何遮挡底色
   * 都会成为一块不透明色块，backdrop-filter 在透明窗口里也遮不住下方条目。
   */
  function renderPinnedItems() {
    if (pinnedCount === 0) return null;

    return (
      <div className="relative z-1 shrink-0">
        {Array.from({ length: pinnedCount }, (_, index) => {
          return (
            <Fragment key={computeItemKey(index)}>
              {renderItemContent(index)}
            </Fragment>
          );
        })}
      </div>
    );
  }

  function computeItemKey(index: number) {
    return getItem(index)?.id ?? `placeholder-${index}`;
  }

  function computeListItemKey(index: number) {
    return computeItemKey(index + pinnedCount);
  }

  function renderListItemContent(index: number) {
    return renderItemContent(index + pinnedCount);
  }

  /**
   * 置顶项常驻滚动区上方，只有普通条目需要换算成虚拟列表下标再滚动。
   */
  function scrollItemIntoView(index: number) {
    if (index < pinnedCount) return;

    virtuosoRef.current?.scrollIntoView({
      behavior: "smooth",
      index: index - pinnedCount,
    });
  }

  function renderItemContent(index: number) {
    const item = getItem(index);
    if (!item) return renderPlaceholderItem();

    const relativeIndex = index - firstVisibleIndex;
    // 多选时 Cmd/Ctrl+数字不粘贴，也就不显示数字提示。
    const hintKey =
      !selecting && relativeIndex >= 0 && relativeIndex < 10
        ? KEY_HINTS[relativeIndex]
        : void 0;

    // 首项同样留出上边距：选中态外环画在边框外侧，贴着视口顶边会被裁掉一截。
    return (
      <div className={listLayout.itemClassName}>
        <ClipboardCard
          canDelete={canDeleteItem(item)}
          checked={checkedIds.has(item.id)}
          hintKey={hintKey}
          isLinkActive={isModifierPressed && !selecting}
          isSelected={
            selectedId === null
              ? index === firstVisibleIndex
              : item.id === selectedId
          }
          item={item}
          onAuxClick={preventMiddleClickDefault}
          onDoubleClick={handleCardDoubleClick}
          onMouseDown={handleCardMouseDown}
          onOpenLink={openItemLink}
          onPickSnippet={pickItemSnippet}
          onPointerEnter={handleCardPointerEnter}
          onPointerLeave={handleCardPointerLeave}
          onPointerMove={handleCardPointerMove}
          onQuickAction={handleCardQuickAction}
          onQuickPaste={quickPasteItem}
          onRootElement={registerItemElement}
          quickActionLabels={quickActionLabels}
          quickActions={quickActions}
          selecting={selecting}
          showOriginalOnHover={showOriginalPreview}
        />
      </div>
    );
  }

  /**
   * 未加载条目的骨架：按当前密度画成两行文本卡片的样子，数据到达后高度变化尽量小。
   */
  function renderPlaceholderItem() {
    const { cardClassName, headerClassName, itemClassName } = listLayout;

    return (
      <div aria-hidden="true" className={itemClassName}>
        <div
          className={cn(
            "flex border-ant-border-secondary bg-ant-fill-quaternary",
            cardClassName,
          )}
        >
          {headerClassName ? (
            <div className={cn("flex items-center gap-1", headerClassName)}>
              <span className="size-4 rounded-1 bg-ant-fill-secondary" />
              <span className="h-3 w-16 rounded-1 bg-ant-fill-secondary" />
            </div>
          ) : (
            <span className="flex h-5 shrink-0 items-center">
              <span className="size-4 rounded-1 bg-ant-fill-secondary" />
            </span>
          )}

          <div className="flex min-w-0 flex-1 flex-col">
            <span className="flex h-5 items-center">
              <span className="h-3 w-9/12 rounded-1 bg-ant-fill-secondary" />
            </span>
            <span className="flex h-5 items-center">
              <span className="h-3 w-6/12 rounded-1 bg-ant-fill-secondary" />
            </span>
          </div>
        </div>
      </div>
    );
  }

  /**
   * 根据方向键计算下一项。
   */
  function getNextKeyboardTarget(event: KeyboardEvent) {
    const nextIndex = getNextKeyboardIndex(
      getItemIndexById,
      firstVisibleIndex,
      selectedId,
      total,
      event.key,
    );
    const item = getItem(nextIndex);
    if (!item) {
      loadRange(nextIndex, nextIndex);
      return null;
    }

    return { index: nextIndex, item };
  }

  /**
   * 判断当前条目是否允许删除：收藏 / 置顶条目分别受各自保护开关约束。
   */
  function canDeleteItem(item: Pick<ClipboardItem, "isFavorite" | "isPinned">) {
    if (item.isPinned && !deletePinnedItems) return false;

    if (!item.isFavorite) return true;

    if (!deleteFavoriteItems) return false;

    if (!deleteFavoriteItemsOnlyInFavoriteGroup) return true;

    return range === "favorite";
  }

  /**
   * ESC 按预览、多选、分组、分类、窗口的顺序逐层退出。
   */
  function closeTopEscapeLayer() {
    if (previewSession !== null) {
      closePreview("escape");
      return;
    }

    if (clipboardSelectionState.active) {
      exitClipboardSelection();
      return;
    }

    if (clipboardViewState.groupId !== null) {
      clipboardViewState.groupId = null;
      return;
    }

    if (clipboardViewState.category !== null) {
      clipboardViewState.category = null;
      return;
    }

    void hideWindow(WINDOW_LABEL.CLIPBOARD);
  }
};

/**
 * 生成空列表文案，按搜索词、范围、分类和自定义分组组合出具体提示。
 */
function getEmptyDescription(
  t: TFunction<"clipboard">,
  keyword: string,
  range: ClipboardRange,
  category: ClipboardKind | null,
  groupId: string | null,
  groupName: string | null,
) {
  const isSearching = keyword.length > 0;
  const isFavorite = range === "favorite";
  const hasGroup = groupId !== null;
  const categoryLabel = category ? t(`empty.categories.${category}`) : "";
  const groupLabel = groupName ?? t("empty.groupFallback");

  if (isSearching) {
    return getSearchingEmptyDescription(
      t,
      keyword,
      isFavorite,
      hasGroup,
      groupLabel,
      categoryLabel,
    );
  }

  if (hasGroup) {
    if (isFavorite && category) {
      return t("empty.groupFavoriteCategory", {
        category: categoryLabel,
        group: groupLabel,
      });
    }

    if (isFavorite) {
      return t("empty.groupFavorites", { group: groupLabel });
    }

    if (category) {
      return t("empty.groupCategory", {
        category: categoryLabel,
        group: groupLabel,
      });
    }

    return t("empty.group", { group: groupLabel });
  }

  if (isFavorite && category) {
    return t("empty.favoriteCategory", { category: categoryLabel });
  }

  if (category) {
    return t("empty.category", { category: categoryLabel });
  }

  return t(isFavorite ? "empty.favorites" : "empty.history");
}

/**
 * 生成搜索空状态文案，覆盖范围 / 分类 / 分组三种过滤维度。
 */
function getSearchingEmptyDescription(
  t: TFunction<"clipboard">,
  keyword: string,
  isFavorite: boolean,
  hasGroup: boolean,
  groupLabel: string,
  categoryLabel: string,
) {
  const hasCategory = categoryLabel.length > 0;

  if (hasGroup) {
    if (isFavorite && hasCategory) {
      return t("empty.searchGroupFavoriteCategory", {
        category: categoryLabel,
        group: groupLabel,
        keyword,
      });
    }

    if (isFavorite) {
      return t("empty.searchGroupFavorites", {
        group: groupLabel,
        keyword,
      });
    }

    if (hasCategory) {
      return t("empty.searchGroupCategory", {
        category: categoryLabel,
        group: groupLabel,
        keyword,
      });
    }

    return t("empty.searchGroup", { group: groupLabel, keyword });
  }

  if (isFavorite && hasCategory) {
    return t("empty.searchFavoriteCategory", {
      category: categoryLabel,
      keyword,
    });
  }

  if (isFavorite) {
    return t("empty.searchFavorites", { keyword });
  }

  if (hasCategory) {
    return t("empty.searchCategory", { category: categoryLabel, keyword });
  }

  return t("empty.searchHistory", { keyword });
}

/**
 * 从当前已加载分组列表中取出选中分组名称；找不到时交给文案层兜底。
 */
function getCurrentGroupName(
  groups: ClipboardGroupRecord[],
  groupId: string | null,
) {
  if (!groupId) return null;

  const current = groups.find((record) => {
    return record.id === groupId;
  });

  return current?.name ?? null;
}

/**
 * 根据方向键和当前选中 id 计算下一项索引。
 */
function getNextKeyboardIndex(
  getItemIndexById: (id: string) => number | null,
  firstVisibleIndex: number,
  selectedId: string | null,
  total: number,
  key: string,
) {
  const selectedIndex =
    selectedId === null ? null : getItemIndexById(selectedId);
  const currentIndex = selectedIndex ?? firstVisibleIndex;

  if (key === "ArrowUp") {
    return Math.max(0, currentIndex - 1);
  }

  return Math.min(total - 1, currentIndex + 1);
}

/**
 * 从后端声明的右键动作中取出可由 Cmd/Ctrl+O 触发的“打开”动作。
 */
function getOpenClipboardAction(actions: ClipboardAction[] | undefined) {
  const openActions: ClipboardAction[] = [
    "openLink",
    "sendEmail",
    "revealInFinder",
    "revealInExplorer",
  ];

  return actions?.find((action) => {
    return openActions.includes(action);
  });
}

/**
 * 删除当前 active 项后优先选后一项；删除末尾时回退到前一项。
 * 右键删除非 active 项时保留当前显式选中，避免意外跳选。
 */
function getSelectedIdAfterDelete(
  getItem: (index: number) => ClipboardItem | null,
  getItemIndexById: (id: string) => number | null,
  activeFallbackIndex: number,
  selectedId: string | null,
  deletedId: string,
) {
  const deletedIndex = getItemIndexById(deletedId);
  if (deletedIndex === null) return selectedId;

  const activeId =
    selectedId ?? getItem(activeFallbackIndex)?.id ?? getItem(0)?.id ?? null;

  if (activeId !== deletedId) return selectedId;

  const nextItem = getItem(deletedIndex + 1) ?? getItem(deletedIndex - 1);

  return nextItem?.id ?? null;
}

/**
 * 判断 Cmd/Ctrl+C 是否应交给浏览器原生复制，避免覆盖输入框或文本选区复制。
 */
function shouldUseNativeCopy(event: KeyboardEvent) {
  const target = event.target;
  if (target instanceof HTMLElement) {
    const tagName = target.tagName.toLowerCase();
    if (target.isContentEditable) return true;
    if (tagName === "input" || tagName === "textarea") return true;
  }

  const selection = window.getSelection();

  return Boolean(selection && !selection.isCollapsed);
}

/**
 * 中键动作已在 mousedown 里处理，这里只拦掉中键 auxclick 的默认行为。
 */
const preventMiddleClickDefault = (event: ReactMouseEvent<HTMLDivElement>) => {
  if (event.button !== 1) return;

  event.preventDefault();
};

/**
 * 统计当前已加载页开头连续置顶条目数，这些条目固定渲染在虚拟列表上方。
 */
function countLeadingPinnedItems(
  getItem: (index: number) => ClipboardItem | null,
) {
  let count = 0;

  while (true) {
    const item = getItem(count);
    if (!item?.isPinned) break;

    count += 1;
  }

  return count;
}

/**
 * 判断普通剪贴板更新是否会出现在当前分组列表中。
 */
function shouldRefreshCurrentGroup(
  range: ClipboardRange,
  category: ClipboardKind | null,
  groupId: string | null,
  kind?: ClipboardKind,
) {
  if (groupId) return false;
  if (range === "favorite") return false;
  if (!category) return true;
  if (kind === void 0) return false;

  return category === kind;
}

export default List;
