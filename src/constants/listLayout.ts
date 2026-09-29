import type { CustomListLayout, ListDensity } from "@/types/settings";

export const LIST_STYLE_OPTIONS = [{ value: "card" }, { value: "seamless" }];

export const LIST_DENSITY_OPTIONS = [
  { value: "comfortable" },
  { value: "standard" },
  { value: "compact" },
  { value: "custom" },
];

/** 自定义密度可选的条目间距（px），与列表的类名映射一一对应。 */
export const LIST_ITEM_GAP_VALUES = [0, 2, 4, 6, 8, 12] as const;

/** 自定义密度可选的上下内边距（px），与列表的类名映射一一对应。 */
export const LIST_PADDING_Y_VALUES = [2, 4, 6, 8] as const;

/**
 * 三档预设换算成自定义尺寸，进入「自定义」时从当前档位起步。
 * 「舒适」即旧版排布，头部行比其它档位高 4px，由列表按档位单独处理。
 */
export const LIST_DENSITY_PRESETS: Record<
  Exclude<ListDensity, "custom">,
  CustomListLayout
> = {
  comfortable: { headerRow: true, itemGap: 12, paddingY: 8 },
  compact: { headerRow: false, itemGap: 4, paddingY: 4 },
  standard: { headerRow: true, itemGap: 8, paddingY: 6 },
};

/** 与 Rust `CustomListLayout::default()` 一致：没调过的自定义尺寸就是「标准」档。 */
export const DEFAULT_CUSTOM_LIST_LAYOUT = LIST_DENSITY_PRESETS.standard;
