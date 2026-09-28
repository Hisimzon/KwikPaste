import { useEventListener, useLatest } from "ahooks";
import { useEffect } from "react";
import { TAURI_EVENT } from "@/constants/events";
import { isMac, isWinClipboardWindow } from "@/utils/is";
import { useTauriListen } from "./useTauriListen";

type KeyboardEventType = "keydown" | "keyup";

const EDITABLE_GLOBAL_KEYBOARD_ATTRIBUTE = "data-allow-global-keyboard";
const EDITABLE_GLOBAL_KEYBOARD_SELECTOR = `[${EDITABLE_GLOBAL_KEYBOARD_ATTRIBUTE}="true"]`;

/**
 * 搜索框交给列表的按键：上下选条目、Enter 粘贴、Esc 逐层退出、Tab 切自定义分组，以及平台修饰键本身
 * （显示快捷键提示）。左右键留给输入框移动光标，Windows 上按 ↓ 离开搜索框后左右键再切分类。
 */
const EDITABLE_GLOBAL_HANDOFF_KEYS = new Set([
  "ArrowDown",
  "ArrowUp",
  "Enter",
  "Escape",
  "Tab",
  isMac ? "Meta" : "Control",
]);

/**
 * macOS 输入框自己的 ⌘ 编辑组合，不交给列表：全选、复制、粘贴、剪切、撤销 / 重做，⌘⌫ / ⌘⌦ 删到行首 / 行尾。
 */
const MAC_NATIVE_EDITING_SHORTCUT_KEYS = new Set([
  "a",
  "backspace",
  "c",
  "delete",
  "v",
  "x",
  "z",
]);

const VK_RETURN = 0x0d;
const VK_OEM_COMMA = 0xbc;
const VK_C = 0x43;

/**
 * Windows 搜索框在按住 Ctrl 时交给列表的应用快捷键：虚拟键码 → Rust 键盘钩子发来的按键名。
 * 与钩子 `ctrl_shortcut_key` 放行的组合一致，只去掉输入框自己的编辑组合（A、C、Backspace、Delete）；
 * 表外的 Ctrl 组合（粘贴、剪切、撤销、按词移动 / 删除等）都留给输入框。按键码而不是 `key` 对照，
 * 法语键盘的数字行、俄语键盘的字母键才和钩子一样命中。
 */
const WIN_CTRL_SHORTCUT_KEYS = new Map<number, string>([
  [VK_RETURN, "Enter"],
  [VK_OEM_COMMA, ","],
  ...[..."0123456789DFKMNOPQST"].map((char): [number, string] => {
    return [char.charCodeAt(0), char.toLowerCase()];
  }),
]);

/**
 * 修饰键。它们的 keyup 在浮层打开期间仍放行给底层、在输入控件里也一律交出，底层靠它复位修饰键状态；
 * 其它键的 keyup 照样拦：底层的空格 keyup 会 `preventDefault`，放过去会吞掉弹窗按钮靠空格 keyup 触发的点击。
 */
const MODIFIER_KEYS = new Set(["Alt", "Control", "Meta", "Shift"]);

/**
 * 输入法正在处理的 keydown 的 keyCode。开始组字的那个键 `isComposing` 还是 false；WebKit 把确认组字的
 * Enter 放在 compositionend 之后派发，那时 `isComposing` 已是 false，这两种都只能靠它认出来。
 */
const IME_PROCESS_KEY_CODE = 229;

interface NavEventPayload {
  code?: string;
  type: KeyboardEventType;
  key: string;
  ctrlKey?: boolean;
  shiftKey?: boolean;
}

interface KeyboardLayerEntry {
  layer: string;
}

/**
 * 独占键盘的浮层栈（每个 webview 一份）。栈顶浮层打开期间，按键只交给登记在该浮层上的处理器，
 * 被遮住的列表、分组栏和快捷键提示一律跳过；修饰键的 keyup 不拦，底层靠它复位修饰键状态。
 */
const keyboardLayers: KeyboardLayerEntry[] = [];

/**
 * 判断某个处理器所在的层当前能否收到按键：没有浮层时只有底层（`layer` 为空）生效。
 */
const isKeyboardLayerActive = (layer?: string) => {
  return keyboardLayers[keyboardLayers.length - 1]?.layer === layer;
};

/**
 * 声明一个独占键盘的浮层：挂载期间压栈，卸载后出栈。
 *
 * 出栈推迟到下一个任务：关闭浮层的那次按键（如 Escape）还会继续派发给其它监听器，
 * 此时浮层必须仍在栈顶，否则底层会接着把同一个按键再处理一遍（例如直接隐藏窗口）。
 */
export const useKeyboardLayer = (layer: string) => {
  useEffect(() => {
    const entry: KeyboardLayerEntry = { layer };

    keyboardLayers.push(entry);

    return () => {
      window.setTimeout(() => {
        const index = keyboardLayers.indexOf(entry);
        if (index !== -1) keyboardLayers.splice(index, 1);
      });
    };
  }, [layer]);
};

/**
 * 跨平台键盘事件监听 hook。
 *
 * macOS 与可聚焦窗口直接监听浏览器键盘事件；Windows 剪贴板窗口默认不可聚焦，
 * 导航键通常来自 Rust 低级钩子。输入控件里的按键默认留给输入本身，
 * 只有搜索框交出导航键和快捷键（见 {@link getEditableHandoffEvent}）。
 * `layer` 表示处理器所属的独占浮层（见 {@link useKeyboardLayer}），不传即底层界面。
 */
export const useKeyboardEvent = (
  type: KeyboardEventType,
  handler: (event: KeyboardEvent) => void,
  layer?: string,
) => {
  const isWindowsClipboardWindow = isWinClipboardWindow();
  const handlerRef = useLatest(handler);

  const shouldHandle = (key: string) => {
    if (type === "keyup" && MODIFIER_KEYS.has(key)) return true;

    return isKeyboardLayerActive(layer);
  };

  const handleBrowserEvent = (event: KeyboardEvent) => {
    if (!shouldHandle(event.key)) return;

    const editableTarget = findEditableElement(event.target);
    if (editableTarget) {
      const handoffEvent = getEditableHandoffEvent(editableTarget, event);
      if (!handoffEvent) return;

      // Windows 剪贴板窗口编辑态下可聚焦、钩子停用：交出按键时让输入框失焦，窗口退回不可聚焦，后续按键重新走钩子。
      // 只按下 Ctrl 时不失焦，接着按的编辑组合才留得在输入框里。
      if (
        isWindowsClipboardWindow &&
        event.type === "keydown" &&
        event.key !== "Control"
      ) {
        editableTarget.blur();
      }

      // 换成钩子同款事件后，处理器的 preventDefault 落不到原事件上，这里替它拦下。
      if (handoffEvent !== event) event.preventDefault();

      handlerRef.current(handoffEvent);
      return;
    }

    handlerRef.current(event);
  };

  useEventListener(type, handleBrowserEvent);

  useTauriListen<NavEventPayload>(TAURI_EVENT.KEYBOARD_NAV, (event) => {
    if (!isWindowsClipboardWindow) return;
    if (shouldUseNativeEditableKeyboard(document.activeElement)) return;

    const { type: payloadType, ...rest } = event.payload;

    if (payloadType !== type || !rest.key) return;
    if (!shouldHandle(rest.key)) return;

    handlerRef.current(
      new KeyboardEvent(payloadType, { cancelable: true, ...rest }),
    );
  });
};

function findEditableElement(target: EventTarget | null): HTMLElement | null {
  if (!(target instanceof Element)) return null;

  let element: Element | null = target;
  while (element) {
    if (element instanceof HTMLElement) {
      if (element.isContentEditable) return element;

      const tagName = element.tagName.toLowerCase();
      if (tagName === "input" || tagName === "textarea") return element;
    }

    element = element.parentElement;
  }

  return null;
}

function shouldUseNativeEditableKeyboard(target: EventTarget | null) {
  return findEditableElement(target) !== null;
}

/**
 * 输入控件里的按键交给全局处理器时用的事件，留给输入框自己时返回 null。
 *
 * 修饰键松开一律交出，底层靠它复位修饰键状态；按下只有搜索框交出，而且输入法组字中的按键不交。
 * macOS 交出时不失焦，⌘ 组合逐个放行，输入框自己的编辑组合除外。Windows 按住 Ctrl 时只交出应用快捷键
 * （见 {@link getWinCtrlShortcutKey}），并换成钩子发来的同款事件；AltGr 在 Windows 上等于 Ctrl+Alt，
 * 它打出的字符（如波兰语的 ś、ń）照常输入。
 */
function getEditableHandoffEvent(target: HTMLElement, event: KeyboardEvent) {
  if (event.type === "keyup") {
    return MODIFIER_KEYS.has(event.key) ? event : null;
  }
  if (!target.closest(EDITABLE_GLOBAL_KEYBOARD_SELECTOR)) return null;
  if (event.isComposing || event.keyCode === IME_PROCESS_KEY_CODE) return null;

  if (isMac) {
    if (EDITABLE_GLOBAL_HANDOFF_KEYS.has(event.key)) return event;
    if (!event.metaKey) return null;

    return MAC_NATIVE_EDITING_SHORTCUT_KEYS.has(event.key.toLowerCase())
      ? null
      : event;
  }

  if (event.key === "Control") return event;
  if (!event.ctrlKey) {
    return EDITABLE_GLOBAL_HANDOFF_KEYS.has(event.key) ? event : null;
  }
  if (event.altKey) return null;

  const key = getWinCtrlShortcutKey(target, event);
  if (!key) return null;

  return new KeyboardEvent("keydown", { cancelable: true, ctrlKey: true, key });
}

/**
 * Windows 搜索框里按住 Ctrl 时交给列表的按键名，留给输入框时返回 null。查 {@link WIN_CTRL_SHORTCUT_KEYS}；
 * Ctrl+C 例外：输入框里没选中文字时也交出，复制选中条目，选中了文字才留给输入框复制。
 */
function getWinCtrlShortcutKey(target: HTMLElement, event: KeyboardEvent) {
  const key = WIN_CTRL_SHORTCUT_KEYS.get(event.keyCode);
  if (key) return key;
  if (event.keyCode !== VK_C) return null;

  const hasSelectedText =
    target instanceof HTMLInputElement &&
    target.selectionStart !== target.selectionEnd;

  return hasSelectedText ? null : "c";
}
