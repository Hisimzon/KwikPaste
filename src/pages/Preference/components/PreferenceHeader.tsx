import { Input } from "antd";
import type { ChangeEvent, FC } from "react";
import { useTranslation } from "react-i18next";
import type { PreferenceTab } from "../types/preferences";
import { translatePreferenceTab } from "../utils/preferenceI18n";
import type { PreferenceSearchResult } from "../utils/preferenceSearch";
import PreferenceSearchResults from "./PreferenceSearchResults";

interface PreferenceHeaderProps {
  activeTab: PreferenceTab;
  searchQuery: string;
  searchResults: PreferenceSearchResult[];
  shouldReduceMotion: boolean;
  onPickSearchResult: (result: PreferenceSearchResult) => void;
  onSearchChange: (event: ChangeEvent<HTMLInputElement>) => void;
}

/**
 * 偏好窗口主区域头部：当前分类标题和全局搜索。
 * 系统文本放大后头部变窄，先收窄搜索框，标题放不下才截断。
 */
const PreferenceHeader: FC<PreferenceHeaderProps> = (props) => {
  const { t } = useTranslation("preferences");
  const {
    activeTab,
    searchQuery,
    searchResults,
    shouldReduceMotion,
    onPickSearchResult,
    onSearchChange,
  } = props;

  return (
    <header
      className="kp-preference-chrome flex shrink-0 items-center justify-between gap-5 border-ant-border-secondary border-b px-6 py-4"
      data-tauri-drag-region
    >
      <h1 className="m-0 min-w-0 truncate font-semibold text-ant-text text-xl leading-snug">
        {translatePreferenceTab(t, activeTab)}
      </h1>

      <div className="relative z-3 min-w-40 max-w-64 flex-1">
        <Input
          allowClear
          autoCapitalize="off"
          autoCorrect="off"
          className="border-ant-border-secondary bg-ant-fill-quaternary text-ant-text"
          onChange={onSearchChange}
          placeholder={t("search.placeholder")}
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
    </header>
  );
};

export default PreferenceHeader;
