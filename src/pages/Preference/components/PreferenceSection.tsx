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
  SettingValue,
} from "../types/preferences";
import { translatePreferenceSection } from "../utils/preferenceI18n";
import PreferenceSettingRow from "./PreferenceSettingRow";
import SourceAppsTransfer from "./SourceAppsTransfer";
import StorageOverviewPanel from "./storageOverview";

interface PreferenceSectionProps {
  highlightedSettingId: string | null;
  highlightToken: number;
  section: PreferenceSectionModel;
  settings: Settings;
  shouldReduceMotion: boolean;
  storageLocation: StorageLocation | null;
  onActionComplete?: (
    setting: PreferenceSetting,
    result?:
      | ChangeStorageLocationResult
      | CleanCacheResult
      | ExportHistoryBackupResult,
  ) => void;
  onChange: (setting: PreferenceSetting, value: SettingValue) => Promise<void>;
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
    storageLocation,
    onActionComplete,
    onChange,
    onNavigateSetting,
    onStorageUsageChange,
  } = props;
  const sourceAppsSettings = resolveSourceAppsSettings(section.settings);
  const isStorageOverview = section.settings.some((setting) => {
    return setting.control.type === "storageOverview";
  });

  if (isStorageOverview) {
    return (
      <SectionFrame section={section}>
        <StorageOverviewPanel
          onNavigateSetting={onNavigateSetting}
          onStorageUsageChange={onStorageUsageChange}
          settings={settings}
        />
      </SectionFrame>
    );
  }

  if (sourceAppsSettings) {
    return (
      <SectionFrame section={section}>
        <div className="kp-preference-panel h-120 rounded-2 border border-ant-border-secondary p-4">
          <SourceAppsTransfer
            excludedAppsSetting={sourceAppsSettings.excludedApps}
            onChange={onChange}
            settings={settings}
          />
        </div>
      </SectionFrame>
    );
  }

  return (
    <SectionFrame section={section}>
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
  children: ReactNode;
}

/**
 * 分组外框：卡片上方一行小标题，页内目录按 `data-preference-section-id` 定位它。
 */
const SectionFrame: FC<SectionFrameProps> = (props) => {
  const { t } = useTranslation("preferences");
  const { section, children } = props;

  return (
    <section data-preference-section-id={section.id}>
      <h2 className="m-0 mb-2 px-1 font-semibold text-ant-text text-sm leading-snug">
        {translatePreferenceSection(t, section, "title")}
      </h2>

      {children}
    </section>
  );
};

/**
 * 识别来源应用分组所需的设置项，缺失则退回通用行渲染。
 */
function resolveSourceAppsSettings(settings: PreferenceSetting[]) {
  const excludedApps = settings.find((setting) => {
    return setting.id === "source.excludedApps";
  });

  if (!excludedApps) return null;

  return { excludedApps };
}
