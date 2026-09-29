/* @unocss-include */
import { useSnapshot } from "valtio";
import {
  LIST_DENSITY_PRESETS,
  LIST_ITEM_GAP_VALUES,
  LIST_PADDING_Y_VALUES,
} from "@/constants/listLayout";
import { settingsState } from "@/stores/settings";
import type {
  CustomListLayout,
  ListDensity,
  ListStyle,
} from "@/types/settings";
import { cn } from "@/utils/cn";

const ITEM_GAP_CLASS: Record<(typeof LIST_ITEM_GAP_VALUES)[number], string> = {
  0: "pt-0",
  2: "pt-0.5",
  4: "pt-1",
  6: "pt-1.5",
  8: "pt-2",
  12: "pt-3",
};

const PADDING_Y_CLASS: Record<(typeof LIST_PADDING_Y_VALUES)[number], string> =
  {
    2: "py-0.5",
    4: "py-1",
    6: "py-1.5",
    8: "py-2",
  };

export interface ListLayout {
  /** 条目外层留白：卡片之间的间距与左右边距；无间风格贴边，不留白。 */
  itemClassName: string;
  /** 条目本体的边框、圆角、内边距，以及头部行 / 正文的排布方向。 */
  cardClassName: string;
  /** 头部行（来源图标、类型、时间）独占一行时的行高；为 null 时来源图标并入正文左侧。 */
  headerClassName: string | null;
  /** 置顶标记：卡片描主色边框；无间风格没有整圈边框，改铺浅底色。 */
  pinnedClassName: string;
  /** 选中外环：条目之间放不下外环时画在内侧，免得被相邻条目和视口顶边截掉。 */
  selectedClassName: string;
  seamless: boolean;
}

/**
 * 把列表风格与密度换算成条目类名；类名都写成字面量，UnoCSS 才扫描得到。
 */
export function resolveListLayout(
  listStyle: ListStyle,
  density: ListDensity,
  customLayout: CustomListLayout,
): ListLayout {
  const comfortable = density === "comfortable";
  const spec =
    density === "custom"
      ? customLayout
      : (LIST_DENSITY_PRESETS[density] ?? LIST_DENSITY_PRESETS.standard);
  const seamless = listStyle === "seamless";
  const itemGap = nearestValue(LIST_ITEM_GAP_VALUES, spec.itemGap);
  const paddingY = nearestValue(LIST_PADDING_Y_VALUES, spec.paddingY);

  return {
    cardClassName: cn(PADDING_Y_CLASS[paddingY], {
      "border-b px-3": seamless,
      "flex-col gap-0.5": spec.headerRow && !comfortable,
      "flex-col gap-1": spec.headerRow && comfortable,
      "items-start gap-2": !spec.headerRow,
      "rounded-2 border px-2": !seamless,
    }),
    headerClassName: resolveHeaderClassName(spec.headerRow, comfortable),
    itemClassName: seamless ? "" : cn("px-3", ITEM_GAP_CLASS[itemGap]),
    pinnedClassName: seamless ? "bg-ant-fill-quaternary" : "border-ant-primary",
    seamless,
    selectedClassName: cn("ring-2 ring-ant-primary/35", {
      "ring-inset": seamless || itemGap < 2,
    }),
  };
}

/**
 * 读取设置里的列表风格与密度，给条目和占位骨架共用。
 */
export function useListLayout() {
  const { clipboard } = useSnapshot(settingsState);
  const { customLayout, density, listStyle } = clipboard.display;

  return resolveListLayout(listStyle, density, customLayout);
}

function resolveHeaderClassName(headerRow: boolean, comfortable: boolean) {
  if (!headerRow) return null;

  return comfortable ? "h-6" : "h-5";
}

/**
 * 手改配置文件等来源可能写入档位以外的数值，按最接近的档位渲染。
 */
function nearestValue<T extends number>(values: readonly T[], value: number) {
  return values.reduce((best, current) => {
    return Math.abs(current - value) < Math.abs(best - value) ? current : best;
  });
}
