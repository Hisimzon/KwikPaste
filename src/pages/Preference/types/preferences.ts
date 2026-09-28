import type { RetentionUnit, Settings } from "@/types/settings";

export type PreferenceTabId =
  | "general"
  | "shortcuts"
  | "appearance"
  | "capture"
  | "window"
  | "paste"
  | "items"
  | "data"
  | "about";

export interface RetentionSettingValue {
  unit: RetentionUnit;
  value: number;
}

export interface SortableCheckboxTreeSettingValue {
  order: string[];
  selected: string[];
}

export type SettingValue =
  | boolean
  | number
  | string
  | string[]
  | SortableCheckboxTreeSettingValue
  | RetentionSettingValue;

export type PreferenceStorageState = "loading" | "ready" | "error";

export interface PreferenceOption {
  value: string | number;
  /**
   * 选项本身就是一组按键时填写，标签直接按平台格式展示按键，不走 i18n。
   */
  shortcut?: string;
}

export interface PreferenceShortcutTag {
  keys: string[];
}

export type PreferenceControl =
  | { type: "switch" }
  | {
      type: "permission";
      kind: "accessibility" | "fullDiskAccess" | "runAsAdministrator";
    }
  | { type: "segmented"; options: PreferenceOption[] }
  | { type: "tiles"; kind: "material" | "theme" }
  | { type: "select"; options: PreferenceOption[]; mode?: "multiple" }
  | { type: "clipboardGroupSelect" }
  | {
      type: "sortableTree";
      options: PreferenceOption[];
    }
  | {
      type: "sortableCheckboxTree";
      options: PreferenceOption[];
      orderPath: readonly string[];
    }
  | {
      type: "number";
      max?: number;
      min?: number;
      suffixKey?: string;
    }
  | { type: "retention" }
  | { type: "text" }
  | { type: "shortcutRecorder" }
  | { type: "textarea" }
  | { type: "appExclusion" }
  | { type: "action"; danger?: boolean }
  | { type: "status" }
  | { type: "shortcutTags"; shortcuts: PreferenceShortcutTag[] }
  | { type: "storageOverview" };

export interface PreferenceSetting {
  control: PreferenceControl;
  disabled?: boolean;
  disabledWhen?: (settings: Settings) => boolean;
  id: string;
  keywords?: string[];
  parentId?: string;
  path?: readonly string[];
  status?: "comingSoon" | "alwaysOn" | "requiresBackend" | "experimental";
  value?: (settings: Settings) => SettingValue;
}

export type PreferenceSettingChangeHandler = (
  setting: PreferenceSetting,
  value: SettingValue,
) => Promise<void>;

export interface PreferenceSection {
  id: string;
  settings: PreferenceSetting[];
}

/** 侧栏按组排列一级分类，组与组之间画分隔线。 */
export type PreferenceTabGroup = "app" | "clipboard" | "data";

export interface PreferenceTab {
  group: PreferenceTabGroup;
  icon: string;
  id: PreferenceTabId;
  sections: PreferenceSection[];
}
