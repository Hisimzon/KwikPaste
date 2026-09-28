import type { FC } from "react";
import { useKeyboardLayer } from "@/hooks/useKeyboardEvent";

const MODAL_KEYBOARD_LAYER = "modal";

/**
 * 弹窗独占键盘：放在 `destroyOnHidden` 弹窗的内容里，随内容挂载压栈、销毁出栈。期间列表、分组栏和
 * 快捷键提示不再响应按键，Enter / 方向键 / Tab / 快捷键只作用于弹窗；弹窗自己的 Esc 关闭走 rc-portal
 * 的 window 监听，不经过浮层栈。弹窗关闭后内容若不销毁，底层会一直收不到按键。
 */
const ModalKeyboardLayer: FC = () => {
  useKeyboardLayer(MODAL_KEYBOARD_LAYER);

  return null;
};

export default ModalKeyboardLayer;
