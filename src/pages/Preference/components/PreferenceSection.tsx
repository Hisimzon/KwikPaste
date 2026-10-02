import type { FC, ReactNode } from "react";
import { useTranslation } from "react-i18next";
import type {
  ChangeStorageLocationResult,
  CleanCacheResult,
  ExportHistoryBackupResult,
  StorageLocation,
  StorageUsage,
} from "@/commands";
import type { Settings } from "@/types/settings";
import type {
  PreferenceSection as PreferenceSectionModel,
  PreferenceSetting,
  PreferenceSettingChangeHandler,
} from "../types/preferences";
import { translatePreferenceSection } from "../utils/preferenceI18n";
import LanSyncPanel from "./lanSync";
import PreferenceSettingRow from "./PreferenceSettingRow";
import StorageOverviewPanel from "./storageOverview";

interface PreferenceSectionProps {
  highlightedSettingId: string | null;
  highlightToken: number;
  section: PreferenceSectionModel;
  settings: Settings;
  shouldReduceMotion: boolean;
  showTitle: boolean;
  storageLocation: StorageLocation | null;
  onActionComplete?: (
    setting: PreferenceSetting,
    result?:
      | ChangeStorageLocationResult
      | CleanCacheResult
      | ExportHistoryBackupResult,
  ) => void;
  onChange: PreferenceSettingChangeHandler;
  onNavigateSetting: (settingId: string) => void;
  onStorageUsageChange: (usage: StorageUsage) => void;
}

/**
 * 偏好页主内容里的一个语义分组；同一分类的所有分组在同一页纵向排列。
 */
const PreferenceSection: FC<PreferenceSectionProps> = (props) => {
  const {
    highlightedSettingId,
    highlightToken,
    section,
    settings,
    shouldReduceMotion,
    showTitle,
    storageLocation,
    onActionComplete,
    onChange,
    onNavigateSetting,
    onStorageUsageChange,
  } = props;
  const isStorageOverview = section.settings.some((setting) => {
    return setting.control.type === "storageOverview";
  });
  const isLanSync = section.settings.some((setting) => {
    return setting.control.type === "lanSync";
  });

  // 同步设备区自己分成本机、已配对、附近几组小标题，不再套分组标题。
  if (isLanSync) {
    return <LanSyncPanel settings={settings} />;
  }

  if (isStorageOverview) {
    return (
      <SectionFrame section={section} showTitle={showTitle}>
        <StorageOverviewPanel
          onNavigateSetting={onNavigateSetting}
          onStorageUsageChange={onStorageUsageChange}
          settings={settings}
        />
      </SectionFrame>
    );
  }

  return (
    <SectionFrame section={section} showTitle={showTitle}>
      <div className="kp-preference-panel overflow-hidden rounded-2 border border-ant-border-secondary">
        {section.settings.map((setting) => {
          return (
            <PreferenceSettingRow
              highlighted={setting.id === highlightedSettingId}
              highlightToken={highlightToken}
              key={setting.id}
              onActionComplete={onActionComplete}
              onChange={onChange}
              setting={setting}
              settings={settings}
              shouldReduceMotion={shouldReduceMotion}
              storageLocation={storageLocation}
            />
          );
        })}
      </div>
    </SectionFrame>
  );
};

export default PreferenceSection;

interface SectionFrameProps {
  section: PreferenceSectionModel;
  showTitle: boolean;
  children: ReactNode;
}

/**
 * 分组外框：卡片上方一行小标题；分类只有一个分组时标题与页标题重复，不再显示。
 */
const SectionFrame: FC<SectionFrameProps> = (props) => {
  const { t } = useTranslation("preferences");
  const { section, showTitle, children } = props;

  return (
    <section>
      {showTitle ? (
        <h2 className="m-0 mb-2 px-1 font-semibold text-ant-text text-sm leading-snug">
          {translatePreferenceSection(t, section, "title")}
        </h2>
      ) : null}

      {children}
    </section>
  );
};
