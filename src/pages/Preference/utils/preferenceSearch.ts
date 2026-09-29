import {
  CAPTURE_KIND_DISPLAY_ORDER,
  translateCaptureKindLabel,
} from "@/constants/captureKinds";
import { allPreferenceSettings } from "../config/preferenceSchema";
import type { PreferenceSetting } from "../types/preferences";
import {
  translatePreferenceOption,
  translatePreferenceSection,
  translatePreferenceSetting,
  translatePreferenceTab,
} from "./preferenceI18n";

export type PreferenceSearchResult = (typeof allPreferenceSettings)[number];
export type PreferenceSearchTranslator = Parameters<
  typeof translatePreferenceTab
>[0];

/**
 * 从完整设置 schema 中执行轻量本地搜索，返回最多 8 条可跳转结果。
 */
export function searchPreferenceSettings(
  query: string,
  t: PreferenceSearchTranslator,
): PreferenceSearchResult[] {
  const normalized = query.trim().toLowerCase();
  if (!normalized) return [];

  return allPreferenceSettings
    .filter(({ section, setting, tab }) => {
      const haystack = [
        translatePreferenceTab(t, tab),
        translatePreferenceSection(t, section, "title"),
        translatePreferenceSetting(t, setting, "title"),
        translatePreferenceSetting(t, setting, "description"),
        ...translateSearchableOptions(t, setting),
        ...(setting.keywords ?? []),
      ]
        .join(" ")
        .toLowerCase();

      return haystack.includes(normalized);
    })
    .slice(0, 8);
}

/**
 * 选项文案也参与搜索：很多设置只有标题没有说明，用户常按看到的选项名（如「跟随光标」「图片」）来找。
 */
function translateSearchableOptions(
  t: PreferenceSearchTranslator,
  setting: PreferenceSetting,
) {
  const { control } = setting;

  if (control.type === "captureKinds") {
    return CAPTURE_KIND_DISPLAY_ORDER.map((kind) => {
      return translateCaptureKindLabel(t, kind);
    });
  }

  if (control.type !== "select" && control.type !== "segmented") return [];

  return control.options.map((option) => {
    return translatePreferenceOption(t, setting, option).label;
  });
}
