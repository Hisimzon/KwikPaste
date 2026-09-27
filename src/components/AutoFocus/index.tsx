import { useMount } from "ahooks";
import type { InputRef } from "antd";
import type { FC, RefObject } from "react";
import { prepareClipboardWindowEditableFocus } from "@/hooks/useClipboardWindowEditableFocus";

interface AutoFocusProps {
  /**
   * 要聚焦的输入控件，取 antd `Input` / `Input.TextArea` 的 ref。
   */
  target: RefObject<Pick<InputRef, "focus"> | null>;
}

/**
 * 挂载即聚焦 `target` 并把光标放到末尾；放在 `destroyOnHidden` 弹窗的内容里，每次打开都随内容重新挂载。
 * 输入框的 `autoFocus` 执行时 Portal 容器还没挂进文档，会落空；等 `afterOpenChange` 又太晚：弹窗焦点锁
 * 先把焦点挪到关闭按钮，Windows 剪贴板窗口随之退出编辑态，开场动画期间敲的字会落到原来的前台应用。
 * 该窗口默认不可聚焦，单纯 `focus()` 只移动 DOM 焦点，所以先切到编辑模式让窗口拿到系统焦点。
 */
const AutoFocus: FC<AutoFocusProps> = (props) => {
  const { target } = props;

  useMount(async () => {
    await prepareClipboardWindowEditableFocus();

    target.current?.focus({ cursor: "end" });
  });

  return null;
};

export default AutoFocus;
