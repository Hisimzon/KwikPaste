import type {
  ClipboardCategory,
  ClipboardItemSort,
  ClipboardRange,
  ContentCategory,
} from "./clipboard";

/**
 * 设置数据契约，镜像 `src-tauri/src/settings/model.rs::Settings`。
 *
 * 字段命名、枚举字面量必须与 Rust 的 `serde` 序列化输出严格一致：
 * - struct 默认走 `rename_all = "camelCase"`；
 * - enum 默认走 `rename_all = "camelCase"`，特例已在每个类型上方注明。
 *
 * Rust 端新增/重命名字段时，**本文件必须同步修改**，否则前端读到的是空字段，组件渲染会失真而无报错。
 */

/** Rust enum `Theme`（`rename_all = "lowercase"`）。 */
export type Theme = "auto" | "light" | "dark";

/** Rust enum `Material`（`rename_all = "lowercase"`）。 */
export type Material = "default" | "mica" | "acrylic";

/** Rust `window::MaterialSupport`：当前系统能真正渲染的原生材质。 */
export interface WindowMaterialSupport {
  mica: boolean;
  acrylic: boolean;
}

/** Rust enum `Language`（手动 `serde(rename)`）。 */
export type Language = "zh-CN" | "en-US";

export type AutoPaste =
  | "disabled"
  | "singleClickPaste"
  | "doubleClickPaste"
  | "singleClickCopy"
  | "doubleClickCopy";

export type MiddleClickAction =
  | "disabled"
  | "singleClickPaste"
  | "singleClickPastePlain"
  | "singleClickCopy"
  | "singleClickCopyPlain";

export type ItemAction =
  | "paste"
  | "pastePlain"
  | "pastePath"
  | "copy"
  | "copyPlain"
  | "splitWords"
  | "openLink"
  | "sendEmail"
  | "reveal"
  | "note"
  | "star"
  | "pinItem"
  | "delete";

export type CaptureKind = "files" | "image" | "html" | "rtf" | "text";

/** `minutes` 只用于自定义清理规则，默认保留时长不写这个单位。 */
export type RetentionUnit =
  | "minutes"
  | "hours"
  | "days"
  | "weeks"
  | "months"
  | "forever";

export type StorageLimitAction = "remind" | "cleanup";

export type WindowPosition = "remember" | "followCursor" | "center";

export type WindowOpenRangeSelection = "preserve" | ClipboardRange;

export type WindowOpenCategorySelection =
  | "preserve"
  | "all"
  | ClipboardCategory;

export type WindowOpenGroupSelection = "preserve" | "all" | `group:${string}`;

export type PreviewHoverDelayMs = "ms300" | "ms500" | "ms1000";

/** 文本预览的展示方式：原文，或拆成词块逐个点选。 */
export type PreviewTextView = "plain" | "words";

export type UpdateFrequency = "daily" | "weekly" | "monthly";

/** Rust enum `TrayClick`：Windows 左键单击托盘图标打开的窗口。 */
export type TrayClick = "clipboard" | "preference";

export interface General {
  autoStart: boolean;
  runAsAdmin: boolean;
  trayIcon: boolean;
  trayClick: TrayClick;
  dockIcon: boolean;
}

export interface Appearance {
  theme: Theme;
  material: Material;
  language: Language;
}

/** Rust enum `QuickPasteModifiers`：快速粘贴按住的修饰键组合。 */
export type QuickPasteModifiers =
  | "controlShift"
  | "controlAlt"
  | "altShift"
  | "alt"
  | "control";

export interface QuickPaste {
  enabled: boolean;
  modifiers: QuickPasteModifiers;
}

/** Rust enum `MouseTrigger`：唤起剪贴板窗口的鼠标按键，侧键 back / forward 对应 XBUTTON1 / XBUTTON2。 */
export type MouseTrigger = "disabled" | "middle" | "back" | "forward";

export interface Shortcuts {
  openClipboard: string;
  openPreference: string;
  winV: boolean;
  mouseTrigger: MouseTrigger;
  quickPaste: QuickPaste;
}

export interface Content {
  autoPaste: AutoPaste;
  middleClick: MiddleClickAction;
  copyPlain: boolean;
  copyThenHideWindow: boolean;
  pastePlain: boolean;
  pasteFilesAsPath: boolean;
  showOriginalPreview: boolean;
  deleteConfirm: boolean;
  deleteFavoriteItems: boolean;
  deleteFavoriteConfirm: boolean;
  deletePinnedItems: boolean;
  deletePinnedConfirm: boolean;
  deleteFavoriteItemsOnlyInFavoriteGroup: boolean;
  autoFavorite: boolean;
  updateOnReuse: boolean;
  sort: ClipboardItemSort;
  itemActions: ItemAction[];
  itemActionOrder: ItemAction[];
}

/** Rust enum `ListStyle`：条目画成独立卡片，或贴边排列用分隔线隔开。 */
export type ListStyle = "card" | "seamless";

/** Rust enum `ListDensity`：`custom` 时按 `customLayout` 的各项尺寸排布。 */
export type ListDensity = "comfortable" | "standard" | "compact" | "custom";

/** Rust `CustomListLayout`：自定义密度的尺寸，单位 px。 */
export interface CustomListLayout {
  headerRow: boolean;
  itemGap: number;
  paddingY: number;
}

export interface Display {
  textMaxLines: number;
  imageMaxHeight: number;
  fileMaxCount: number;
  quickSnippets: boolean;
  listStyle: ListStyle;
  density: ListDensity;
  customLayout: CustomListLayout;
}

export interface Capture {
  text: boolean;
  html: boolean;
  rtf: boolean;
  image: boolean;
  files: boolean;
  maxTextMb: number;
  maxImageMb: number;
  order: CaptureKind[];
}

export interface Sensitive {
  collectSecrets: boolean;
  redactSecrets: boolean;
}

export interface Retention {
  value: number;
  unit: RetentionUnit;
}

/** 自定义清理规则；所有条件同时满足才算命中，空数组 / 0 / false 表示不限。 */
export interface RetentionRule {
  id: string;
  enabled: boolean;
  categories: ContentCategory[];
  minSizeKb: number;
  sourceAppIds: string[];
  sensitiveOnly: boolean;
  unusedOnly: boolean;
  keep: Retention;
}

export interface History {
  retention: Retention;
  /** 自上而下匹配，记录按第一条命中的规则清理；都没命中时按 `retention`。 */
  rules: RetentionRule[];
  maxCount: number;
  storageLimitMb: number;
  storageLimitAction: StorageLimitAction;
}

export interface Search {
  defaultFocus: boolean;
  clearOnHide: boolean;
}

export interface Window {
  position: WindowPosition;
  scrollToTopOnOpen: boolean;
  selectRangeOnOpen: WindowOpenRangeSelection;
  selectCategoryOnOpen: WindowOpenCategorySelection;
  selectGroupOnOpen: WindowOpenGroupSelection;
  lightweightMode: boolean;
  idleDestroySeconds: number;
}

export interface Preview {
  hoverEnabled: boolean;
  hoverDelayMs: PreviewHoverDelayMs;
  spaceEnabled: boolean;
  textView: PreviewTextView;
}

export interface Feedback {
  copySound: boolean;
}

export interface Filters {
  excludedAppIds: string[];
}

export interface Onboarding {
  completed: boolean;
  lastStep: number;
}

export interface Clipboard {
  capture: Capture;
  content: Content;
  display: Display;
  sensitive: Sensitive;
  history: History;
  search: Search;
  window: Window;
  preview: Preview;
  feedback: Feedback;
  filters: Filters;
}

export interface Update {
  autoCheck: boolean;
  frequency: UpdateFrequency;
  includeBeta: boolean;
  includeNightly: boolean;
  lastCheckedAt: string | null;
  skippedVersion: string | null;
}

/** 局域网同步；设备身份和已配对设备不在设置里（不进备份包）。 */
export interface LanSync {
  enabled: boolean;
  /** 在其他设备上显示的名称；空串表示用系统电脑名。 */
  deviceName: string;
  /** 收到其他设备的复制后同时写入本机剪贴板。 */
  writeClipboard: boolean;
  text: boolean;
  image: boolean;
  maxImageMb: number;
}

export interface SyncSettings {
  lan: LanSync;
}

export interface Settings {
  general: General;
  appearance: Appearance;
  shortcuts: Shortcuts;
  clipboard: Clipboard;
  sync: SyncSettings;
  onboarding: Onboarding;
  update: Update;
}

/**
 * 任意层级可选的设置补丁，与 Rust 端 `update_settings` 的 `serde_json::Value` 深度合并语义对齐。
 * 数组字段按整体替换处理（与 Rust 的 deep_merge 行为一致），调用方需要传完整数组。
 */
export type SettingsPatch = DeepPartial<Settings>;

type DeepPartial<T> =
  T extends ReadonlyArray<infer U>
    ? ReadonlyArray<U>
    : T extends object
      ? { [K in keyof T]?: DeepPartial<T[K]> }
      : T;
