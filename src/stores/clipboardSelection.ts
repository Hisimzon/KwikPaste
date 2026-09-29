import { proxy } from "valtio";

interface ClipboardSelectionState {
  /** 列表是否处于多选状态；选中了哪些记录由列表自己维护。 */
  active: boolean;
}

/**
 * 剪贴板窗口的多选状态：底部栏的多选按钮写入，列表据此切换点击行为并接管底部栏。
 * 单独成 store：`clipboardViewState` 的任何变化都会触发列表重置选中与重新查询。
 */
export const clipboardSelectionState = proxy<ClipboardSelectionState>({
  active: false,
});

/**
 * 进入多选。
 */
export const enterClipboardSelection = () => {
  clipboardSelectionState.active = true;
};

/**
 * 退出多选。
 */
export const exitClipboardSelection = () => {
  clipboardSelectionState.active = false;
};
