import { Checkbox } from "antd";
import type { FC } from "react";
import { useTranslation } from "react-i18next";
import {
  CAPTURE_KIND_DISPLAY_ORDER,
  translateCaptureKindLabel,
} from "@/constants/captureKinds";
import type { CaptureKind } from "@/types/settings";
import type { PreferenceSetting, SettingValue } from "../../types/preferences";
import type { ControlProps } from "./types";

interface CaptureKindsControlProps extends ControlProps {
  setting: PreferenceSetting;
  value?: SettingValue;
}

/**
 * 用一组复选框即时保存要记录的内容类型，代替逐类型一行的开关。
 */
const CaptureKindsControl: FC<CaptureKindsControlProps> = (props) => {
  const { t } = useTranslation("preferences");
  const { disabled, onChange, setting, value } = props;
  const selected = Array.isArray(value) ? value : [];
  const options = CAPTURE_KIND_DISPLAY_ORDER.map((kind) => {
    return { label: translateCaptureKindLabel(t, kind), value: kind };
  });

  const handleChange = async (next: CaptureKind[]) => {
    await onChange(setting, next);
  };

  return (
    <Checkbox.Group<CaptureKind>
      className="gap-x-5 gap-y-2"
      disabled={disabled}
      onChange={handleChange}
      options={options}
      value={selected as CaptureKind[]}
    />
  );
};

export default CaptureKindsControl;
