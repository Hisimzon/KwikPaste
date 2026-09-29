import type { FC } from "react";
import { useTranslation } from "react-i18next";
import PreferenceSettingControl from "@/pages/Preference/components/settingControls/PreferenceSettingControl";
import type {
  PreferenceSetting,
  PreferenceSettingChangeHandler,
} from "@/pages/Preference/types/preferences";
import { translatePreferenceSetting } from "@/pages/Preference/utils/preferenceI18n";
import type { Settings } from "@/types/settings";
import OnboardingCard from "./OnboardingCard";

/** 引导卡片沿用的设置图标；偏好页的设置行不再放图标。 */
const SETTING_ICONS: Record<string, string> = {
  "permissions.accessibility": "i-lucide:accessibility",
  "permissions.fullDiskAccess": "i-lucide:hard-drive",
  "permissions.runAsAdministrator": "i-lucide:shield-alert",
  "shortcuts.mouseTrigger": "i-ph:mouse-bold",
  "shortcuts.openClipboard": "i-lucide:clipboard",
  "shortcuts.openPreference": "i-lucide:settings",
  "shortcuts.quickPaste": "i-lucide:clipboard-paste",
  "shortcuts.quickPasteModifiers": "i-lucide:command",
  "shortcuts.winV": "i-lucide:clipboard-list",
};

interface OnboardingPreferenceCardProps {
  compact?: boolean;
  setting: PreferenceSetting;
  settings: Settings;
  onChange: PreferenceSettingChangeHandler;
}

/**
 * 在引导页卡片样式内复用偏好设置项的文案、图标和控件行为。
 */
const OnboardingPreferenceCard: FC<OnboardingPreferenceCardProps> = (props) => {
  const { compact = false, setting, settings, onChange } = props;
  const { t } = useTranslation("preferences");
  const value = setting.value?.(settings);
  const disabled =
    setting.disabled === true || setting.disabledWhen?.(settings) === true;

  return (
    <OnboardingCard
      compact={compact}
      description={translatePreferenceSetting(t, setting, "description")}
      icon={SETTING_ICONS[setting.id] ?? "i-lucide:circle"}
      title={translatePreferenceSetting(t, setting, "title")}
    >
      <PreferenceSettingControl
        disabled={disabled}
        onChange={onChange}
        setting={setting}
        settings={settings}
        value={value}
      />
    </OnboardingCard>
  );
};

export default OnboardingPreferenceCard;
