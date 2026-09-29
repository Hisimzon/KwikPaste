import type { TFunction } from "i18next";
import { motion } from "motion/react";
import type { FC } from "react";
import { useTranslation } from "react-i18next";
import type {
  ChangeStorageLocationResult,
  CleanCacheResult,
  ExportHistoryBackupResult,
  StorageLocation,
} from "@/commands";
import type { Settings } from "@/types/settings";
import { cn } from "@/utils/cn";
import { isPreferenceSettingCollapsed } from "../config/preferenceSchema";
import type {
  PreferenceSetting,
  PreferenceSettingChangeHandler,
} from "../types/preferences";
import { translatePreferenceSetting } from "../utils/preferenceI18n";
import PreferenceStatusBadge from "./PreferenceStatusBadge";
import PreferenceSettingControl from "./settingControls/PreferenceSettingControl";

interface PreferenceSettingRowProps {
  highlighted: boolean;
  highlightToken: number;
  setting: PreferenceSetting;
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
  onChange: PreferenceSettingChangeHandler;
}

/**
 * 单个设置项行：左侧标题（必要时附一句说明），右侧渲染对应控件。
 */
const PreferenceSettingRow: FC<PreferenceSettingRowProps> = (props) => {
  const { t } = useTranslation("preferences");
  const {
    highlighted,
    highlightToken,
    setting,
    settings,
    shouldReduceMotion,
    storageLocation,
    onActionComplete,
    onChange,
  } = props;
  const value = setting.value?.(settings);
  const parentDisabled = setting.disabledWhen?.(settings) === true;
  const disabled = setting.disabled === true || parentDisabled;
  const childSetting = setting.parentId !== void 0;
  const collapsed = isPreferenceSettingCollapsed(setting, settings);
  // 外观磁贴、采集类型和清理规则列表需要整行宽度，换到标题下方单独一行。
  const isFullWidthControl =
    setting.control.type === "tiles" ||
    setting.control.type === "captureKinds" ||
    setting.control.type === "retentionRules";
  const description = resolveSettingDescription(t, setting, storageLocation);
  const highlightOpacity = shouldReduceMotion
    ? 0.08
    : [0, 0.1, 0.055, 0.085, 0.045, 0.07, 0.025, 0];
  const rowTransition = {
    duration: shouldReduceMotion ? 0 : 0.16,
    ease: "easeOut",
  } as const;

  return (
    <motion.div
      animate={{
        borderBottomWidth: collapsed ? 0 : 1,
        height: collapsed ? 0 : "auto",
        minHeight: collapsed ? 0 : "3.25rem",
        opacity: collapsed ? 0 : 1,
        paddingBottom: collapsed ? 0 : "0.625rem",
        paddingTop: collapsed ? 0 : "0.625rem",
      }}
      className={cn(
        "relative flex items-center gap-5 overflow-hidden border-ant-split border-b px-4 last:border-b-0",
        {
          "flex-wrap items-start gap-y-3": isFullWidthControl,
          "pl-8": childSetting,
          "pointer-events-none": collapsed,
        },
      )}
      data-preference-setting-id={setting.id}
      initial={false}
      transition={rowTransition}
    >
      {highlighted ? (
        <>
          <motion.span
            animate={{ opacity: highlightOpacity }}
            className="pointer-events-none absolute inset-0 bg-ant-primary"
            initial={{ opacity: 0 }}
            key={highlightToken}
            transition={{
              duration: shouldReduceMotion ? 0 : 2.35,
              ease: "easeInOut",
              times: shouldReduceMotion
                ? void 0
                : [0, 0.11, 0.28, 0.43, 0.58, 0.73, 0.88, 1],
            }}
          />
          <span className="absolute top-3 bottom-3 left-0 w-0.5 rounded-full bg-ant-primary" />
        </>
      ) : null}

      <div className="relative min-w-0 flex-1">
        <div
          className={cn("flex min-w-0 items-center gap-2", {
            "opacity-65": disabled,
          })}
        >
          <span
            className={cn(
              "truncate text-sm leading-snug",
              disabled ? "text-ant-secondary" : "text-ant-text",
            )}
          >
            {translatePreferenceSetting(t, setting, "title")}
          </span>
          <PreferenceStatusBadge compact status={setting.status} />
        </div>
        {description ? (
          <div
            className={cn(
              "mt-0.5 text-xs leading-snug",
              disabled ? "text-ant-disabled" : "text-ant-secondary",
              { "break-all": setting.id === "localData.dataDirectory" },
            )}
          >
            {description}
          </div>
        ) : null}
      </div>

      <div
        className={cn("relative flex shrink-0 justify-end", {
          "basis-full justify-start": isFullWidthControl,
        })}
      >
        <PreferenceSettingControl
          disabled={disabled}
          onActionComplete={onActionComplete}
          onChange={onChange}
          setting={setting}
          settings={settings}
          storageLocation={storageLocation}
          value={value}
        />
      </div>
    </motion.div>
  );
};

export default PreferenceSettingRow;

/**
 * 数据目录行展示当前真实路径，其它设置沿用 schema 文案。
 */
function resolveSettingDescription(
  t: TFunction<"preferences">,
  setting: PreferenceSetting,
  storageLocation: StorageLocation | null,
) {
  if (setting.id === "localData.dataDirectory" && storageLocation) {
    return storageLocation.currentPath;
  }

  return translatePreferenceSetting(t, setting, "description");
}
