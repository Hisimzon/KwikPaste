import { Input } from "antd";
import type { ChangeEvent, FC } from "react";
import { useEffect, useRef } from "react";
import { useTranslation } from "react-i18next";
import ScrollArea from "@/components/ScrollArea";
import { cn } from "@/utils/cn";
import type { PreferenceSection, PreferenceTab } from "../types/preferences";
import {
  translatePreferenceSection,
  translatePreferenceTab,
} from "../utils/preferenceI18n";
import type { PreferenceSearchResult } from "../utils/preferenceSearch";
import PreferenceCountTag from "./PreferenceCountTag";
import PreferenceSearchResults from "./PreferenceSearchResults";

interface PreferenceHeaderProps {
  activeSectionId: string;
  activeTab: PreferenceTab;
  searchQuery: string;
  searchResults: PreferenceSearchResult[];
  shouldReduceMotion: boolean;
  totalSettings: number;
  onPickSearchResult: (result: PreferenceSearchResult) => void;
  onSearchChange: (event: ChangeEvent<HTMLInputElement>) => void;
  onSectionSelect: (sectionId: string) => void;
}

/**
 * 偏好窗口主区域头部：标题、全局搜索和页内分组目录。
 */
const PreferenceHeader: FC<PreferenceHeaderProps> = (props) => {
  const { t } = useTranslation(["preferences", "common"]);
  const {
    activeSectionId,
    activeTab,
    searchQuery,
    searchResults,
    shouldReduceMotion,
    totalSettings,
    onPickSearchResult,
    onSearchChange,
    onSectionSelect,
  } = props;

  return (
    <header
      className="kp-preference-chrome shrink-0 border-ant-border-secondary border-b px-6 pt-4 pb-2"
      data-tauri-drag-region
    >
      <div
        className="flex items-center justify-between gap-5"
        data-tauri-drag-region
      >
        <div className="min-w-0">
          <h1 className="m-0 flex items-center gap-2 font-semibold text-ant-text text-lg leading-snug">
            <i
              aria-hidden="true"
              className={cn("text-ant-primary text-lg", activeTab.icon)}
            />
            <span className="truncate">
              {translatePreferenceTab(t, activeTab)}
            </span>
          </h1>
        </div>

        <div className="flex shrink-0 items-center">
          <div className="relative z-3 w-64">
            <Input
              allowClear
              autoCapitalize="off"
              autoCorrect="off"
              className="border-ant-border-secondary bg-ant-fill-quaternary text-ant-text"
              onChange={onSearchChange}
              placeholder={t("preferences:search.placeholder")}
              prefix={
                <i
                  aria-hidden="true"
                  className="i-lucide:search text-ant-secondary text-base"
                />
              }
              spellCheck={false}
              value={searchQuery}
            />

            <PreferenceSearchResults
              onPick={onPickSearchResult}
              query={searchQuery.trim()}
              results={searchResults}
              shouldReduceMotion={shouldReduceMotion}
            />
          </div>
        </div>
      </div>

      <SectionTabs
        activeSectionId={activeSectionId}
        onSectionSelect={onSectionSelect}
        sections={activeTab.sections}
        totalSettings={totalSettings}
      />
    </header>
  );
};

interface SectionTabsProps {
  activeSectionId: string;
  sections: PreferenceSection[];
  totalSettings: number;
  onSectionSelect: (sectionId: string) => void;
}

/**
 * 当前分类的页内目录：点击滚到对应分组，滚动时跟随高亮；只有一个分组时不显示。
 * 系统文本放大后窗口可能窄于设计宽度，目录放不下时横向滚动，并把高亮的分组滚进视野。
 */
const SectionTabs: FC<SectionTabsProps> = (props) => {
  const { t } = useTranslation(["preferences", "common"]);
  const { activeSectionId, sections, totalSettings, onSectionSelect } = props;
  const tabsRef = useRef<HTMLDivElement | null>(null);
  const visibleSections = sections.length > 1 ? sections : [];

  useEffect(() => {
    const viewport = tabsRef.current;
    const tab = viewport?.querySelector<HTMLElement>(
      `[data-section-id="${activeSectionId}"]`,
    );
    if (!viewport || !tab) return;

    const viewportRect = viewport.getBoundingClientRect();
    const tabRect = tab.getBoundingClientRect();

    if (tabRect.left < viewportRect.left) {
      viewport.scrollLeft -= viewportRect.left - tabRect.left;
    } else if (tabRect.right > viewportRect.right) {
      viewport.scrollLeft += tabRect.right - viewportRect.right;
    }
  }, [activeSectionId]);

  return (
    <div className="mt-3 flex h-7.5 items-center gap-3" data-tauri-drag-region>
      <ScrollArea
        className="min-w-0 flex-1"
        data-tauri-drag-region
        ref={tabsRef}
      >
        <div className="flex h-7.5 items-center gap-3" data-tauri-drag-region>
          {visibleSections.map((section) => {
            const selected = section.id === activeSectionId;
            const handleClick = () => {
              onSectionSelect(section.id);
            };

            return (
              <button
                className={cn(
                  "relative h-7.5 shrink-0 cursor-pointer whitespace-nowrap border-0 bg-transparent px-0.5 font-medium text-sm transition-colors focus-visible:ring-1 focus-visible:ring-ant-primary motion-reduce:transition-none",
                  selected
                    ? "text-ant-text"
                    : "text-ant-secondary hover:text-ant-text",
                )}
                data-section-id={section.id}
                key={section.id}
                onClick={handleClick}
                type="button"
              >
                {translatePreferenceSection(t, section, "title")}
                <span
                  className={cn(
                    "absolute right-0 bottom-0 left-0 h-0.5 rounded-full transition-colors motion-reduce:transition-none",
                    selected ? "bg-ant-primary" : "bg-transparent",
                  )}
                />
              </button>
            );
          })}
        </div>
      </ScrollArea>

      <PreferenceCountTag>
        {t("common:units.settings", { count: totalSettings })}
      </PreferenceCountTag>
    </div>
  );
};

export default PreferenceHeader;
