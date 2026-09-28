import type { FC } from "react";
import { useState } from "react";
import type { Retention } from "@/types/settings";
import type {
  PreferenceSetting,
  RetentionSettingValue,
  SettingValue,
} from "../../types/preferences";
import { DEFAULT_RETENTION_UNITS } from "../../utils/retention";
import RetentionDurationInput from "./RetentionDurationInput";
import type { ControlProps } from "./types";

interface RetentionControlProps extends ControlProps {
  setting: PreferenceSetting;
  value: RetentionSettingValue;
}

/**
 * 将未知设置值归一成保留周期对象，避免空快照或 schema 误配导致控件崩溃。
 */
export function resolveRetentionValue(
  value?: SettingValue,
): RetentionSettingValue {
  if (
    typeof value === "object" &&
    value !== null &&
    !Array.isArray(value) &&
    "unit" in value &&
    "value" in value
  ) {
    return value;
  }

  return { unit: "forever", value: 0 };
}

/**
 * 默认保留时长控件：数值 + 单位，可选永久保留。
 */
const RetentionControl: FC<RetentionControlProps> = (props) => {
  const { disabled, onChange, setting, value } = props;
  // 用户取消确认时设置不变，换 key 重建输入框丢掉草稿值。
  const [revision, setRevision] = useState(0);

  const handleChange = async (next: Retention) => {
    const saved = await onChange(setting, next);
    if (saved !== false) return;

    setRevision((current) => {
      return current + 1;
    });
  };

  return (
    <RetentionDurationInput
      disabled={disabled}
      key={revision}
      onChange={handleChange}
      units={DEFAULT_RETENTION_UNITS}
      value={value}
    />
  );
};

export default RetentionControl;
