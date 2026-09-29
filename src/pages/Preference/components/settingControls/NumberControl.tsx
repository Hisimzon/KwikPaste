import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { InputNumber, Space } from "antd";
import type { FC } from "react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { setWindowDirty } from "@/commands";
import type { PreferenceSetting } from "../../types/preferences";
import { translatePreferenceNumberSuffix } from "../../utils/preferenceI18n";
import type { ControlProps } from "./types";

interface NumberControlProps extends ControlProps {
  setting: PreferenceSetting;
  value: number;
}

/**
 * 数字输入失焦保存，避免连续输入时频繁落盘。
 */
const NumberControl: FC<NumberControlProps> = (props) => {
  const { t } = useTranslation("preferences");
  const { disabled, onChange, setting, value } = props;
  const [draft, setDraft] = useState<number | null>(value);
  const dirtyRef = useRef(false);
  const committingRef = useRef(false);
  const windowLabelRef = useRef(getCurrentWebviewWindow().label);
  const control = setting.control.type === "number" ? setting.control : null;
  const dirtyOwner = `number:${setting.id}`;

  useEffect(() => {
    setDraft(value);
  }, [value]);

  useEffect(() => {
    const dirty = draft !== value;
    if (dirtyRef.current === dirty) return;

    dirtyRef.current = dirty;
    void setWindowDirty(windowLabelRef.current, dirtyOwner, dirty);
  }, [dirtyOwner, draft, value]);

  useEffect(() => {
    return () => {
      if (!dirtyRef.current) return;

      dirtyRef.current = false;
      void setWindowDirty(windowLabelRef.current, dirtyOwner, false);
    };
  }, [dirtyOwner]);

  if (!control) return null;

  const suffix = translatePreferenceNumberSuffix(t, setting);

  const handleChange = (next: number | null) => {
    setDraft(next);
  };

  const commit = async () => {
    // 回车提交后弹出确认框会让输入框失焦，再触发一次提交；等上一次结束前忽略。
    if (committingRef.current) return;

    const min = control.min ?? 0;
    const max = control.max ?? Number.POSITIVE_INFINITY;
    const current = typeof draft === "number" ? draft : min;
    const next = Math.min(max, Math.max(min, current));
    setDraft(next);

    committingRef.current = true;
    try {
      const saved = await onChange(setting, next);
      // 用户取消确认时设置不变，草稿退回当前值。
      if (saved === false) setDraft(value);
    } finally {
      committingRef.current = false;
    }
  };

  const handleBlur = async () => {
    await commit();
  };

  const handlePressEnter = async () => {
    await commit();
  };

  if (!suffix) {
    return (
      <InputNumber
        className="w-40"
        disabled={disabled}
        max={control.max}
        min={control.min}
        onBlur={handleBlur}
        onChange={handleChange}
        onPressEnter={handlePressEnter}
        value={draft}
      />
    );
  }

  return (
    <Space.Compact className="w-40">
      <InputNumber
        className="min-w-0 flex-1"
        disabled={disabled}
        max={control.max}
        min={control.min}
        onBlur={handleBlur}
        onChange={handleChange}
        onPressEnter={handlePressEnter}
        value={draft}
      />
      <Space.Addon>{suffix}</Space.Addon>
    </Space.Compact>
  );
};

export default NumberControl;
