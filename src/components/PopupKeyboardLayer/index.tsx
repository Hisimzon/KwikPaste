import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import type { FC } from "react";
import { TAURI_EVENT } from "@/constants/events";
import { useKeyboardEvent, useKeyboardLayer } from "@/hooks/useKeyboardEvent";
import { useTauriListen } from "@/hooks/useTauriListen";

interface PopupKeyboardLayerProps {
  /**
   * 浮层独占的键盘层名，见 `useKeyboardLayer`。
   */
  layer: string;
  /**
   * 关闭浮层。
   */
  onClose: () => void;
}

interface WindowVisibilityPayload {
  label: string;
  visible: boolean;
}

/**
 * 下拉菜单 / 气泡卡片打开期间独占键盘：随浮层打开挂载、关闭卸载。期间列表、分组栏和快捷键提示收不到按键，
 * 关浮层的 Esc 不会再去清筛选或隐藏窗口。浏览器按键的 Esc 由 rc-portal 关闭浮层；Windows 剪贴板窗口
 * 不可聚焦时按键来自 Rust 钩子，浮层收不到，由这里关闭。所在窗口隐藏时一并收起，免得下次打开还挡着键盘。
 */
const PopupKeyboardLayer: FC<PopupKeyboardLayerProps> = (props) => {
  const { layer, onClose } = props;

  useKeyboardLayer(layer);

  const handleKeyDown = (event: KeyboardEvent) => {
    if (event.key !== "Escape") return;

    event.preventDefault();
    onClose();
  };

  useKeyboardEvent("keydown", handleKeyDown, layer);

  const handleWindowVisibility = (event: {
    payload: WindowVisibilityPayload;
  }) => {
    const { label, visible } = event.payload;
    if (visible || label !== getCurrentWebviewWindow().label) return;

    onClose();
  };

  useTauriListen<WindowVisibilityPayload>(
    TAURI_EVENT.WINDOW_VISIBILITY,
    handleWindowVisibility,
  );

  return null;
};

export default PopupKeyboardLayer;
