/* @unocss-include */
import {
  CAPTURE_KIND_DISPLAY_ORDER,
  CAPTURE_KIND_OPTIONS,
} from "@/constants/captureKinds";
import { ITEM_ACTION_OPTIONS } from "@/constants/itemActions";
import { LANGUAGE_OPTIONS } from "@/constants/languages";
import {
  LIST_DENSITY_OPTIONS,
  LIST_ITEM_GAP_VALUES,
  LIST_PADDING_Y_VALUES,
  LIST_STYLE_OPTIONS,
} from "@/constants/listLayout";
import { QUICK_PASTE_MODIFIER_OPTIONS } from "@/constants/quickPaste";
import {
  WINDOW_OPEN_CATEGORY_OPTIONS,
  WINDOW_OPEN_RANGE_OPTIONS,
} from "@/constants/windowOpenSelection";
import type { Settings } from "@/types/settings";
import { isMac, isPortable, isWin } from "@/utils/is";
import {
  LAN_SYNC_AVAILABLE,
  LAN_SYNC_MAX_IMAGE_MB_MAX,
  LAN_SYNC_MAX_IMAGE_MB_MIN,
  STORAGE_LIMIT_MAX_MB,
  STORAGE_LIMIT_MIN_MB,
} from "../constants";
import type { PreferenceSetting, PreferenceTab } from "../types/preferences";

const CLICK_ACTION_OPTIONS = [
  { value: "disabled" },
  { value: "singleClickPaste" },
  { value: "doubleClickPaste" },
  { value: "singleClickCopy" },
  { value: "doubleClickCopy" },
];
const MIDDLE_CLICK_ACTION_OPTIONS = [
  { value: "disabled" },
  { value: "singleClickPaste" },
  { value: "singleClickPastePlain" },
  { value: "singleClickCopy" },
  { value: "singleClickCopyPlain" },
];
const TRAY_CLICK_OPTIONS = [{ value: "clipboard" }, { value: "preference" }];
const MOUSE_TRIGGER_OPTIONS = [
  { value: "disabled" },
  { value: "middle" },
  { value: "back" },
  { value: "forward" },
];
const CLIPBOARD_SORT_OPTIONS = [
  { value: "createdAtDesc" },
  { value: "updatedAtDesc" },
  { value: "useCountDesc" },
];
const STORAGE_LIMIT_ACTION_OPTIONS = [
  { value: "remind" },
  { value: "cleanup" },
];
const LIST_ITEM_GAP_OPTIONS = LIST_ITEM_GAP_VALUES.map((value) => {
  return { value };
});
const LIST_PADDING_Y_OPTIONS = LIST_PADDING_Y_VALUES.map((value) => {
  return { value };
});
// 便携版不自动提权：计划任务记着 exe 路径，换电脑、挪文件夹后就失效。
const SHOW_RUN_AS_ADMINISTRATOR = isWin && !isPortable;

/**
 * 自定义尺寸只在密度选「自定义」时展开，其它档位下收起，相当于默认收起的高级项。
 */
function isNotCustomDensity(settings: Settings) {
  return settings.clipboard.display.density !== "custom";
}

/**
 * 偏好分类按用户找设置的心智组织；setting id 是 i18n key 与跨窗口定位目标，挪动分类时保持不变。
 */
export const preferenceTabs: PreferenceTab[] = [
  {
    group: "app",
    icon: "i-lucide:settings",
    id: "general",
    sections: [
      {
        id: "startup",
        settings: [
          {
            control: { type: "switch" },
            id: "control.autoStart",
            keywords: ["startup", "login", "autostart"],
            path: ["general", "autoStart"],
            value: (settings) => {
              return settings.general.autoStart;
            },
          },
          {
            control: { type: "switch" },
            id: "control.trayIcon",
            keywords: ["tray", "menu bar", "system"],
            path: ["general", "trayIcon"],
            value: (settings) => {
              return settings.general.trayIcon;
            },
          },
          // macOS 单击菜单栏图标弹出菜单，没有可选的单击行为。
          ...(isWin
            ? [
                {
                  control: {
                    options: TRAY_CLICK_OPTIONS,
                    type: "select",
                  } as const,
                  disabledWhen: (settings: Settings) => {
                    return !settings.general.trayIcon;
                  },
                  id: "control.trayClick",
                  keywords: ["tray", "click", "preferences", "clipboard"],
                  parentId: "control.trayIcon",
                  path: ["general", "trayClick"] as const,
                  value: (settings: Settings) => {
                    return settings.general.trayClick;
                  },
                },
              ]
            : []),
          {
            control: { type: "switch" },
            id: "control.dockIcon",
            keywords: ["dock", "taskbar", "icon"],
            path: ["general", "dockIcon"],
            value: (settings) => {
              return settings.general.dockIcon;
            },
          },
          {
            control: { type: "action" },
            id: "control.reopenOnboarding",
            keywords: ["onboarding", "guide", "welcome", "help"],
          },
        ],
      },
      ...(isMac || SHOW_RUN_AS_ADMINISTRATOR
        ? [
            {
              id: "permissions",
              settings: [
                ...(isMac
                  ? [
                      {
                        control: {
                          kind: "accessibility",
                          type: "permission",
                        } as const,
                        id: "permissions.accessibility",
                        keywords: [
                          "accessibility",
                          "permission",
                          "paste",
                          "macos",
                        ],
                      },
                      {
                        control: {
                          kind: "fullDiskAccess",
                          type: "permission",
                        } as const,
                        id: "permissions.fullDiskAccess",
                        keywords: [
                          "full disk",
                          "permission",
                          "privacy",
                          "macos",
                        ],
                      },
                    ]
                  : []),
                ...(SHOW_RUN_AS_ADMINISTRATOR
                  ? [
                      {
                        control: {
                          kind: "runAsAdministrator",
                          type: "permission",
                        } as const,
                        id: "permissions.runAsAdministrator",
                        keywords: [
                          "administrator",
                          "admin",
                          "permission",
                          "windows",
                          "uac",
                        ],
                      },
                    ]
                  : []),
              ],
            },
          ]
        : []),
      {
        id: "performance",
        settings: [
          {
            control: { type: "switch" },
            id: "window.lightweightMode",
            keywords: ["system", "performance", "memory", "idle"],
            path: ["clipboard", "window", "lightweightMode"],
            value: (settings) => {
              return settings.clipboard.window.lightweightMode;
            },
          },
          {
            control: {
              max: 86400,
              min: 5,
              suffixKey: "seconds",
              type: "number",
            },
            disabledWhen: (settings) => {
              return !settings.clipboard.window.lightweightMode;
            },
            id: "window.idleDestroySeconds",
            keywords: ["system", "idle", "destroy", "seconds"],
            parentId: "window.lightweightMode",
            path: ["clipboard", "window", "idleDestroySeconds"],
            value: (settings) => {
              return settings.clipboard.window.idleDestroySeconds;
            },
          },
        ],
      },
    ],
  },
  {
    group: "app",
    icon: "i-lucide:keyboard",
    id: "shortcuts",
    sections: [
      {
        id: "globalShortcuts",
        settings: [
          {
            control: { type: "shortcutRecorder" },
            id: "shortcuts.openClipboard",
            keywords: ["shortcut", "hotkey", "open"],
            path: ["shortcuts", "openClipboard"],
            value: (settings) => {
              return settings.shortcuts.openClipboard;
            },
          },
          {
            control: { type: "shortcutRecorder" },
            id: "shortcuts.openPreference",
            keywords: ["shortcut", "hotkey", "preference"],
            path: ["shortcuts", "openPreference"],
            value: (settings) => {
              return settings.shortcuts.openPreference;
            },
          },
          ...(isWin
            ? [
                {
                  control: { type: "switch" } as const,
                  id: "shortcuts.winV",
                  keywords: [
                    "win",
                    "winv",
                    "windows",
                    "clipboard history",
                    "super",
                  ],
                  path: ["shortcuts", "winV"] as const,
                  value: (settings: Settings) => {
                    return settings.shortcuts.winV;
                  },
                },
                {
                  control: {
                    options: MOUSE_TRIGGER_OPTIONS,
                    type: "select",
                  } as const,
                  id: "shortcuts.mouseTrigger",
                  keywords: [
                    "mouse",
                    "middle click",
                    "middle button",
                    "side button",
                    "back",
                    "forward",
                    "xbutton",
                    "wheel",
                  ],
                  path: ["shortcuts", "mouseTrigger"] as const,
                  value: (settings: Settings) => {
                    return settings.shortcuts.mouseTrigger;
                  },
                },
              ]
            : []),
          {
            control: { type: "switch" },
            id: "shortcuts.quickPaste",
            keywords: ["quick paste", "number", "digit", "paste", "hotkey"],
            path: ["shortcuts", "quickPaste", "enabled"],
            value: (settings) => {
              return settings.shortcuts.quickPaste.enabled;
            },
          },
          {
            control: {
              options: QUICK_PASTE_MODIFIER_OPTIONS,
              type: "select",
            },
            disabledWhen: (settings) => {
              return !settings.shortcuts.quickPaste.enabled;
            },
            id: "shortcuts.quickPasteModifiers",
            keywords: ["quick paste", "modifier", "ctrl", "alt", "shift"],
            parentId: "shortcuts.quickPaste",
            path: ["shortcuts", "quickPaste", "modifiers"],
            value: (settings) => {
              return settings.shortcuts.quickPaste.modifiers;
            },
          },
        ],
      },
    ],
  },
  {
    group: "app",
    icon: "i-lucide:palette",
    id: "appearance",
    sections: [
      {
        id: "appearance",
        settings: [
          {
            control: { kind: "theme", type: "tiles" },
            id: "appearance.theme",
            keywords: ["theme", "dark", "light"],
            path: ["appearance", "theme"],
            value: (settings) => {
              return settings.appearance.theme;
            },
          },
          {
            control: { kind: "material", type: "tiles" },
            id: "appearance.material",
            keywords: ["material", "mica", "acrylic", "transparency"],
            path: ["appearance", "material"],
            value: (settings) => {
              return settings.appearance.material;
            },
          },
          {
            control: {
              options: LANGUAGE_OPTIONS,
              type: "segmented",
            },
            id: "appearance.language",
            keywords: ["language", "locale", "english"],
            path: ["appearance", "language"],
            value: (settings) => {
              return settings.appearance.language;
            },
          },
        ],
      },
      {
        id: "cards",
        settings: [
          {
            control: { options: LIST_STYLE_OPTIONS, type: "segmented" },
            id: "appearance.listStyle",
            keywords: ["list", "card", "seamless", "divider", "style"],
            path: ["clipboard", "display", "listStyle"],
            value: (settings) => {
              return settings.clipboard.display.listStyle;
            },
          },
          {
            control: { options: LIST_DENSITY_OPTIONS, type: "segmented" },
            id: "appearance.listDensity",
            keywords: ["density", "compact", "spacing", "height", "custom"],
            path: ["clipboard", "display", "density"],
            value: (settings) => {
              return settings.clipboard.display.density;
            },
          },
          {
            control: { type: "switch" },
            disabledWhen: isNotCustomDensity,
            id: "appearance.headerRow",
            keywords: ["density", "header", "time", "type", "icon"],
            parentId: "appearance.listDensity",
            path: ["clipboard", "display", "customLayout", "headerRow"],
            value: (settings) => {
              return settings.clipboard.display.customLayout.headerRow;
            },
          },
          {
            control: { options: LIST_ITEM_GAP_OPTIONS, type: "select" },
            // 无间风格条目贴边排列，间距不生效，一并收起。
            disabledWhen: (settings) => {
              return (
                isNotCustomDensity(settings) ||
                settings.clipboard.display.listStyle === "seamless"
              );
            },
            id: "appearance.itemGap",
            keywords: ["density", "gap", "spacing", "margin"],
            parentId: "appearance.listDensity",
            path: ["clipboard", "display", "customLayout", "itemGap"],
            value: (settings) => {
              return settings.clipboard.display.customLayout.itemGap;
            },
          },
          {
            control: { options: LIST_PADDING_Y_OPTIONS, type: "select" },
            disabledWhen: isNotCustomDensity,
            id: "appearance.itemPadding",
            keywords: ["density", "padding", "height"],
            parentId: "appearance.listDensity",
            path: ["clipboard", "display", "customLayout", "paddingY"],
            value: (settings) => {
              return settings.clipboard.display.customLayout.paddingY;
            },
          },
          {
            control: { max: 5, min: 1, suffixKey: "lines", type: "number" },
            id: "appearance.textMaxLines",
            keywords: ["density", "text", "line", "compact"],
            path: ["clipboard", "display", "textMaxLines"],
            value: (settings) => {
              return settings.clipboard.display.textMaxLines;
            },
          },
          {
            control: { max: 100, min: 20, suffixKey: "px", type: "number" },
            id: "appearance.imageMaxHeight",
            keywords: ["density", "image", "height", "thumbnail"],
            path: ["clipboard", "display", "imageMaxHeight"],
            value: (settings) => {
              return settings.clipboard.display.imageMaxHeight;
            },
          },
          {
            control: { max: 5, min: 1, suffixKey: "files", type: "number" },
            id: "appearance.fileMaxCount",
            keywords: ["density", "file", "count", "array"],
            path: ["clipboard", "display", "fileMaxCount"],
            value: (settings) => {
              return settings.clipboard.display.fileMaxCount;
            },
          },
          {
            control: { type: "switch" },
            id: "paste.quickSnippets",
            keywords: ["quick", "snippet", "extract", "number", "code"],
            path: ["clipboard", "display", "quickSnippets"],
            value: (settings) => {
              return settings.clipboard.display.quickSnippets;
            },
          },
          {
            control: { type: "switch" },
            id: "appearance.showOriginalPreview",
            keywords: ["note", "hover", "original", "preview"],
            path: ["clipboard", "content", "showOriginalPreview"],
            value: (settings) => {
              return settings.clipboard.content.showOriginalPreview;
            },
          },
        ],
      },
    ],
  },
  {
    group: "clipboard",
    icon: "i-lucide:clipboard-plus",
    id: "capture",
    sections: [
      {
        id: "capture",
        settings: [
          {
            control: { type: "captureKinds" },
            id: "capture.kinds",
            keywords: [
              "text",
              "plain",
              "html",
              "rtf",
              "rich text",
              "image",
              "picture",
              "file",
              "folder",
              "record",
            ],
            path: ["clipboard", "capture"],
            value: (settings) => {
              return CAPTURE_KIND_DISPLAY_ORDER.filter((kind) => {
                return settings.clipboard.capture[kind];
              });
            },
          },
          {
            control: {
              options: CAPTURE_KIND_OPTIONS,
              type: "sortableTree",
            },
            id: "capture.order",
            keywords: ["priority", "order", "format", "rich text"],
            path: ["clipboard", "capture", "order"],
            value: (settings) => {
              return settings.clipboard.capture.order;
            },
          },
        ],
      },
      {
        id: "captureRules",
        settings: [
          {
            control: { min: 0, suffixKey: "mb", type: "number" },
            id: "capture.maxTextMb",
            keywords: ["text", "size", "limit", "mb"],
            path: ["clipboard", "capture", "maxTextMb"],
            value: (settings) => {
              return settings.clipboard.capture.maxTextMb;
            },
          },
          {
            control: { min: 0, suffixKey: "mb", type: "number" },
            id: "capture.maxImageMb",
            keywords: ["image", "picture", "size", "limit", "mb"],
            path: ["clipboard", "capture", "maxImageMb"],
            value: (settings) => {
              return settings.clipboard.capture.maxImageMb;
            },
          },
          {
            control: { type: "switch" },
            id: "copy.sound",
            keywords: ["sound", "feedback", "copy"],
            path: ["clipboard", "feedback", "copySound"],
            value: (settings) => {
              return settings.clipboard.feedback.copySound;
            },
          },
        ],
      },
      {
        id: "sensitive",
        settings: [
          {
            control: { type: "switch" },
            id: "sensitive.collectSecrets",
            keywords: ["token", "key", "secret", "code"],
            path: ["clipboard", "sensitive", "collectSecrets"],
            value: (settings) => {
              return settings.clipboard.sensitive.collectSecrets;
            },
          },
          {
            control: { type: "switch" },
            id: "sensitive.redactSecrets",
            keywords: ["token", "key", "secret", "redact", "mask"],
            path: ["clipboard", "sensitive", "redactSecrets"],
            value: (settings) => {
              return settings.clipboard.sensitive.redactSecrets;
            },
          },
          {
            control: { type: "appExclusion" },
            id: "source.excludedApps",
            keywords: ["exclude", "ignore", "app", "source"],
            path: ["clipboard", "filters", "excludedAppIds"],
            value: (settings) => {
              return settings.clipboard.filters.excludedAppIds;
            },
          },
        ],
      },
    ],
  },
  {
    group: "clipboard",
    icon: "i-lucide:panel-top",
    id: "window",
    sections: [
      {
        id: "window",
        settings: [
          {
            control: {
              options: [
                { value: "followCursor" },
                { value: "center" },
                { value: "remember" },
              ],
              type: "segmented",
            },
            id: "window.position",
            keywords: ["window", "position", "cursor"],
            path: ["clipboard", "window", "position"],
            value: (settings) => {
              return settings.clipboard.window.position;
            },
          },
          {
            control: { type: "switch" },
            id: "window.scrollToTopOnOpen",
            keywords: ["window", "scroll", "top", "open"],
            path: ["clipboard", "window", "scrollToTopOnOpen"],
            value: (settings) => {
              return settings.clipboard.window.scrollToTopOnOpen;
            },
          },
          {
            control: { options: WINDOW_OPEN_RANGE_OPTIONS, type: "select" },
            id: "window.selectRangeOnOpen",
            keywords: ["window", "range", "all", "favorite", "open"],
            path: ["clipboard", "window", "selectRangeOnOpen"],
            value: (settings) => {
              return settings.clipboard.window.selectRangeOnOpen;
            },
          },
          {
            control: { options: WINDOW_OPEN_CATEGORY_OPTIONS, type: "select" },
            id: "window.selectCategoryOnOpen",
            keywords: ["window", "category", "kind", "all", "open"],
            path: ["clipboard", "window", "selectCategoryOnOpen"],
            value: (settings) => {
              return settings.clipboard.window.selectCategoryOnOpen;
            },
          },
          {
            control: { type: "clipboardGroupSelect" },
            id: "window.selectGroupOnOpen",
            keywords: ["window", "group", "folder", "all", "open"],
            path: ["clipboard", "window", "selectGroupOnOpen"],
            value: (settings) => {
              return settings.clipboard.window.selectGroupOnOpen;
            },
          },
          {
            control: { type: "switch" },
            id: "search.defaultFocus",
            keywords: ["search", "focus", "open"],
            path: ["clipboard", "search", "defaultFocus"],
            value: (settings) => {
              return settings.clipboard.search.defaultFocus;
            },
          },
          {
            control: { type: "switch" },
            id: "search.clearOnHide",
            keywords: ["search", "clear", "hide"],
            path: ["clipboard", "search", "clearOnHide"],
            value: (settings) => {
              return settings.clipboard.search.clearOnHide;
            },
          },
        ],
      },
      {
        id: "sort",
        settings: [
          {
            control: { options: CLIPBOARD_SORT_OPTIONS, type: "select" },
            id: "search.sort",
            keywords: ["sort", "frequency", "usage", "created", "updated"],
            path: ["clipboard", "content", "sort"],
            value: (settings) => {
              return settings.clipboard.content.sort;
            },
          },
          {
            control: { type: "switch" },
            id: "copy.updateOnReuse",
            keywords: ["copy", "paste", "reuse", "sort", "frequency"],
            path: ["clipboard", "content", "updateOnReuse"],
            value: (settings) => {
              return settings.clipboard.content.updateOnReuse;
            },
          },
        ],
      },
      {
        id: "preview",
        settings: [
          {
            control: { type: "switch" },
            id: "preview.hover",
            keywords: ["preview", "hover"],
            path: ["clipboard", "preview", "hoverEnabled"],
            value: (settings) => {
              return settings.clipboard.preview.hoverEnabled;
            },
          },
          {
            control: {
              options: [
                { value: "ms300" },
                { value: "ms500" },
                { value: "ms1000" },
              ],
              type: "segmented",
            },
            disabledWhen: (settings) => {
              return !settings.clipboard.preview.hoverEnabled;
            },
            id: "preview.delay",
            keywords: ["preview", "delay", "hover"],
            parentId: "preview.hover",
            path: ["clipboard", "preview", "hoverDelayMs"],
            value: (settings) => {
              return settings.clipboard.preview.hoverDelayMs;
            },
          },
          {
            control: { type: "switch" },
            id: "preview.space",
            keywords: ["space", "preview", "keyboard"],
            path: ["clipboard", "preview", "spaceEnabled"],
            value: (settings) => {
              return settings.clipboard.preview.spaceEnabled;
            },
          },
          {
            control: {
              options: [{ value: "plain" }, { value: "words" }],
              type: "segmented",
            },
            id: "preview.textView",
            keywords: ["preview", "text", "words", "split"],
            path: ["clipboard", "preview", "textView"],
            value: (settings) => {
              return settings.clipboard.preview.textView;
            },
          },
        ],
      },
    ],
  },
  {
    group: "clipboard",
    icon: "i-lucide:clipboard-paste",
    id: "paste",
    sections: [
      {
        id: "click",
        settings: [
          {
            control: {
              options: CLICK_ACTION_OPTIONS,
              type: "segmented",
            },
            id: "paste.autoPaste",
            keywords: ["paste", "click", "auto"],
            path: ["clipboard", "content", "autoPaste"],
            value: (settings) => {
              return settings.clipboard.content.autoPaste;
            },
          },
          {
            control: {
              options: MIDDLE_CLICK_ACTION_OPTIONS,
              type: "segmented",
            },
            id: "paste.middleClick",
            keywords: ["paste", "middle click", "mouse"],
            path: ["clipboard", "content", "middleClick"],
            value: (settings) => {
              return settings.clipboard.content.middleClick;
            },
          },
          {
            control: { type: "switch" },
            id: "copy.hideWindow",
            keywords: ["copy", "hide", "window"],
            path: ["clipboard", "content", "copyThenHideWindow"],
            value: (settings) => {
              return settings.clipboard.content.copyThenHideWindow;
            },
          },
        ],
      },
      {
        id: "format",
        settings: [
          {
            control: { type: "switch" },
            id: "paste.plainDefault",
            keywords: ["plain", "paste", "format"],
            path: ["clipboard", "content", "pastePlain"],
            value: (settings) => {
              return settings.clipboard.content.pastePlain;
            },
          },
          {
            control: { type: "switch" },
            id: "paste.fileMode",
            keywords: ["file", "path", "paste"],
            path: ["clipboard", "content", "pasteFilesAsPath"],
            value: (settings) => {
              return settings.clipboard.content.pasteFilesAsPath;
            },
          },
          {
            control: { type: "switch" },
            id: "copy.plainDefault",
            keywords: ["copy", "plain", "format"],
            path: ["clipboard", "content", "copyPlain"],
            value: (settings) => {
              return settings.clipboard.content.copyPlain;
            },
          },
        ],
      },
    ],
  },
  {
    group: "clipboard",
    icon: "i-lucide:layers",
    id: "items",
    sections: [
      {
        id: "actions",
        settings: [
          {
            control: {
              options: ITEM_ACTION_OPTIONS,
              orderPath: ["clipboard", "content", "itemActionOrder"],
              type: "sortableCheckboxTree",
            },
            id: "actions.visible",
            keywords: ["action", "hover", "buttons"],
            path: ["clipboard", "content", "itemActions"],
            value: (settings) => {
              return {
                order: settings.clipboard.content.itemActionOrder,
                selected: settings.clipboard.content.itemActions,
              };
            },
          },
        ],
      },
      {
        id: "deleteProtection",
        settings: [
          {
            control: { type: "switch" },
            id: "actions.deleteConfirm",
            keywords: ["delete", "confirm"],
            path: ["clipboard", "content", "deleteConfirm"],
            value: (settings) => {
              return settings.clipboard.content.deleteConfirm;
            },
          },
          {
            control: { type: "switch" },
            id: "actions.deleteFavoriteItems",
            keywords: ["delete", "favorite", "allow"],
            path: ["clipboard", "content", "deleteFavoriteItems"],
            value: (settings) => {
              return settings.clipboard.content.deleteFavoriteItems;
            },
          },
          {
            control: { type: "switch" },
            disabledWhen: (settings) => {
              return !settings.clipboard.content.deleteFavoriteItems;
            },
            id: "actions.deleteFavoriteConfirm",
            keywords: ["delete", "favorite", "confirm"],
            parentId: "actions.deleteFavoriteItems",
            path: ["clipboard", "content", "deleteFavoriteConfirm"],
            value: (settings) => {
              return settings.clipboard.content.deleteFavoriteConfirm;
            },
          },
          {
            control: { type: "switch" },
            disabledWhen: (settings) => {
              return !settings.clipboard.content.deleteFavoriteItems;
            },
            id: "actions.deleteFavoriteItemsOnlyInFavoriteGroup",
            keywords: ["delete", "favorite", "group"],
            parentId: "actions.deleteFavoriteItems",
            path: [
              "clipboard",
              "content",
              "deleteFavoriteItemsOnlyInFavoriteGroup",
            ],
            value: (settings) => {
              return settings.clipboard.content
                .deleteFavoriteItemsOnlyInFavoriteGroup;
            },
          },
          {
            control: { type: "switch" },
            id: "actions.deletePinnedItems",
            keywords: ["delete", "pinned", "pin", "allow"],
            path: ["clipboard", "content", "deletePinnedItems"],
            value: (settings) => {
              return settings.clipboard.content.deletePinnedItems;
            },
          },
          {
            control: { type: "switch" },
            disabledWhen: (settings) => {
              return !settings.clipboard.content.deletePinnedItems;
            },
            id: "actions.deletePinnedConfirm",
            keywords: ["delete", "pinned", "pin", "confirm"],
            parentId: "actions.deletePinnedItems",
            path: ["clipboard", "content", "deletePinnedConfirm"],
            value: (settings) => {
              return settings.clipboard.content.deletePinnedConfirm;
            },
          },
        ],
      },
      {
        id: "organizing",
        settings: [
          {
            control: { type: "action" },
            id: "organizing.customGroups",
            keywords: ["group", "folder", "organize"],
          },
          {
            control: { type: "switch" },
            id: "organizing.autoFavorite",
            keywords: ["note", "favorite", "auto"],
            path: ["clipboard", "content", "autoFavorite"],
            value: (settings) => {
              return settings.clipboard.content.autoFavorite;
            },
          },
        ],
      },
    ],
  },
  ...(LAN_SYNC_AVAILABLE
    ? ([
        {
          group: "data",
          icon: "i-lucide:arrow-down-up",
          id: "sync",
          sections: [
            {
              id: "lanSync",
              settings: [
                {
                  control: { type: "switch" },
                  id: "sync.lan.enabled",
                  keywords: [
                    "lan",
                    "sync",
                    "network",
                    "wifi",
                    "device",
                    "局域网",
                  ],
                  path: ["sync", "lan", "enabled"],
                  value: (settings) => {
                    return settings.sync.lan.enabled;
                  },
                },
                {
                  control: { type: "text" },
                  id: "sync.lan.deviceName",
                  keywords: ["device", "name", "computer"],
                  path: ["sync", "lan", "deviceName"],
                  value: (settings) => {
                    return settings.sync.lan.deviceName;
                  },
                },
              ],
            },
            {
              id: "lanDevices",
              settings: [
                {
                  control: { type: "lanSync" },
                  id: "sync.lan.devices",
                  keywords: [
                    "pair",
                    "pairing",
                    "code",
                    "device",
                    "nearby",
                    "address",
                    "ip",
                  ],
                },
              ],
            },
            {
              id: "lanContent",
              settings: [
                {
                  control: { type: "switch" },
                  id: "sync.lan.text",
                  keywords: ["text", "sync"],
                  path: ["sync", "lan", "text"],
                  value: (settings) => {
                    return settings.sync.lan.text;
                  },
                },
                {
                  control: { type: "switch" },
                  id: "sync.lan.image",
                  keywords: ["image", "picture", "screenshot", "sync"],
                  path: ["sync", "lan", "image"],
                  value: (settings) => {
                    return settings.sync.lan.image;
                  },
                },
                {
                  control: {
                    max: LAN_SYNC_MAX_IMAGE_MB_MAX,
                    min: LAN_SYNC_MAX_IMAGE_MB_MIN,
                    suffixKey: "mb",
                    type: "number",
                  },
                  disabledWhen: (settings) => {
                    return !settings.sync.lan.image;
                  },
                  id: "sync.lan.maxImageMb",
                  keywords: ["image", "size", "limit"],
                  path: ["sync", "lan", "maxImageMb"],
                  value: (settings) => {
                    return settings.sync.lan.maxImageMb;
                  },
                },
                {
                  control: { type: "switch" },
                  id: "sync.lan.writeClipboard",
                  keywords: ["clipboard", "paste", "receive"],
                  path: ["sync", "lan", "writeClipboard"],
                  value: (settings) => {
                    return settings.sync.lan.writeClipboard;
                  },
                },
              ],
            },
          ],
        },
      ] satisfies PreferenceTab[])
    : []),
  {
    group: "data",
    icon: "i-lucide:chart-pie",
    id: "overview",
    sections: [
      {
        id: "overview",
        settings: [
          {
            control: { type: "storageOverview" },
            id: "overview.dashboard",
            keywords: [
              "overview",
              "statistics",
              "storage",
              "usage",
              "count",
              "category",
              "trend",
              "source",
            ],
          },
        ],
      },
    ],
  },
  {
    group: "data",
    icon: "i-lucide:database",
    id: "data",
    sections: [
      {
        id: "cleanup",
        settings: [
          {
            control: { type: "retention" },
            id: "history.retention",
            keywords: ["retention", "cleanup", "history", "expire"],
            path: ["clipboard", "history", "retention"],
            value: (settings) => {
              return settings.clipboard.history.retention;
            },
          },
          {
            control: { type: "retentionRules" },
            id: "history.rules",
            keywords: [
              "retention",
              "rules",
              "cleanup",
              "expire",
              "image",
              "text",
              "size",
              "sensitive",
            ],
            path: ["clipboard", "history", "rules"],
            value: (settings) => {
              return settings.clipboard.history.rules;
            },
          },
          {
            control: { min: 0, suffixKey: "items", type: "number" },
            id: "history.maxCount",
            keywords: ["max", "count", "limit"],
            path: ["clipboard", "history", "maxCount"],
            value: (settings) => {
              return settings.clipboard.history.maxCount;
            },
          },
          {
            control: {
              max: STORAGE_LIMIT_MAX_MB,
              min: STORAGE_LIMIT_MIN_MB,
              suffixKey: "mb",
              type: "number",
            },
            id: "localData.storageLimit",
            keywords: ["storage", "limit", "size", "quota", "disk"],
            path: ["clipboard", "history", "storageLimitMb"],
            value: (settings) => {
              return settings.clipboard.history.storageLimitMb;
            },
          },
          {
            control: {
              options: STORAGE_LIMIT_ACTION_OPTIONS,
              type: "segmented",
            },
            id: "localData.storageLimitAction",
            keywords: ["storage", "limit", "cleanup", "remind"],
            path: ["clipboard", "history", "storageLimitAction"],
            value: (settings) => {
              return settings.clipboard.history.storageLimitAction;
            },
          },
          {
            control: { type: "cleanupStatus" },
            id: "history.cleanupStatus",
            keywords: ["cleanup", "status", "protected", "now"],
          },
        ],
      },
      {
        id: "backup",
        settings: [
          {
            control: { type: "action" },
            id: "backup.exportHistory",
            keywords: ["export", "backup", "history"],
          },
          {
            control: { type: "action" },
            id: "backup.exportReadable",
            keywords: [
              "export",
              "excel",
              "xlsx",
              "markdown",
              "groups",
              "favorites",
            ],
          },
          {
            control: { type: "action" },
            id: "backup.importHistory",
            keywords: ["import", "backup", "history"],
          },
        ],
      },
      {
        id: "localData",
        settings: [
          {
            control: { type: "action" },
            id: "localData.dataDirectory",
            keywords: ["database", "sqlite", "local", "cache", "image", "icon"],
          },
          {
            control: { type: "action" },
            id: "localData.cleanCache",
            keywords: ["cache", "clean", "storage"],
          },
          {
            control: { danger: true, type: "action" },
            id: "localData.clearHistory",
            keywords: ["clear", "history", "records", "delete"],
          },
        ],
      },
    ],
  },
  {
    group: "data",
    icon: "i-lucide:info",
    id: "about",
    sections: [
      {
        id: "about",
        settings: [
          {
            control: { type: "action" },
            id: "about.website",
            keywords: ["website", "homepage", "download"],
          },
          {
            control: { type: "action" },
            id: "about.github",
            keywords: ["github", "source", "repository", "open source"],
          },
        ],
      },
      {
        id: "updates",
        settings: [
          {
            control: { type: "action" },
            id: "about.checkUpdates",
            keywords: ["update", "version"],
          },
          {
            control: { type: "switch" },
            id: "updates.autoCheck",
            keywords: ["update", "version"],
            path: ["update", "autoCheck"],
            value: (settings) => {
              return settings.update.autoCheck;
            },
          },
          {
            control: {
              options: [
                { value: "daily" },
                { value: "weekly" },
                { value: "monthly" },
              ],
              type: "segmented",
            },
            disabledWhen: (settings) => {
              return !settings.update.autoCheck;
            },
            id: "updates.frequency",
            keywords: ["update", "frequency", "schedule"],
            parentId: "updates.autoCheck",
            path: ["update", "frequency"],
            value: (settings) => {
              return settings.update.frequency;
            },
          },
          {
            control: { type: "switch" },
            id: "updates.beta",
            keywords: ["beta", "update"],
            path: ["update", "includeBeta"],
            value: (settings) => {
              return settings.update.includeBeta;
            },
          },
          {
            control: { type: "switch" },
            id: "updates.nightly",
            keywords: ["nightly", "update"],
            path: ["update", "includeNightly"],
            value: (settings) => {
              return settings.update.includeNightly;
            },
          },
        ],
      },
      {
        id: "diagnostics",
        settings: [
          {
            control: { type: "action" },
            id: "localData.logDirectory",
            keywords: ["log", "diagnostic"],
          },
          {
            control: { type: "action" },
            id: "diagnostics.windowLifecycle",
            keywords: ["window", "lifecycle", "debug", "phase"],
          },
          {
            control: { danger: true, type: "action" },
            id: "diagnostics.resetPreferences",
            keywords: ["reset", "preferences"],
          },
        ],
      },
    ],
  },
];

/**
 * 按稳定 setting id 查找偏好设置项，供其它窗口复用偏好 schema。
 */
export function findPreferenceSetting(
  settingId: string,
): PreferenceSetting | null {
  for (const tab of preferenceTabs) {
    for (const section of tab.sections) {
      const setting = section.settings.find((item) => {
        return item.id === settingId;
      });

      if (setting) return setting;
    }
  }

  return null;
}

/**
 * 按稳定 section id 取偏好设置项；平台条件已由 schema 自身处理。
 */
export function findPreferenceSectionSettings(
  sectionId: string,
): PreferenceSetting[] {
  for (const tab of preferenceTabs) {
    const section = tab.sections.find((item) => {
      return item.id === sectionId;
    });

    if (section) return section.settings;
  }

  return [];
}

/**
 * 父开关关闭时子项整行收起，不显示也不参与计数。
 */
export function isPreferenceSettingCollapsed(
  setting: PreferenceSetting,
  settings: Settings,
) {
  return (
    setting.parentId !== void 0 && setting.disabledWhen?.(settings) === true
  );
}

/**
 * 必需设置缺失属于 schema 维护错误，调用方无需静默降级。
 */
export function requirePreferenceSetting(settingId: string): PreferenceSetting {
  const setting = findPreferenceSetting(settingId);
  if (!setting) {
    throw new Error(`Preference setting not found: ${settingId}`);
  }

  return setting;
}

export const allPreferenceSettings = preferenceTabs.flatMap((tab) => {
  return tab.sections.flatMap((section) => {
    return section.settings.map((setting) => {
      return { section, setting, tab };
    });
  });
});
