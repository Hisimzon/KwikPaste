import { InputNumber, Select, Space } from "antd";
import type { FC } from "react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { Retention, RetentionUnit } from "@/types/settings";
import { isForeverRetention, retentionWithUnit } from "../../utils/retention";

interface RetentionDurationInputProps {
  disabled?: boolean;
  units: RetentionUnit[];
  value: Retention;
  /** 数值在失焦 / 回车时提交，单位切换立即提交。 */
  onChange: (next: Retention) => void | Promise<void>;
}

/**
 * 保留时长输入：数值 + 单位；选「永久保留」时隐藏数值。
 */
const RetentionDurationInput: FC<RetentionDurationInputProps> = (props) => {
  const { t } = useTranslation("preferences");
  const { disabled, units, value, onChange } = props;
  const forever = isForeverRetention(value);
  const [draftValue, setDraftValue] = useState<number | null>(value.value);
  const committingRef = useRef(false);

  useEffect(() => {
    setDraftValue(value.value);
  }, [value]);

  const unitOptions = units.map((unit) => {
    return { label: t(`schema.retentionUnits.${unit}`), value: unit };
  });

  const commitValue = async () => {
    // 回车提交后弹出确认框会让输入框失焦，再触发一次提交；等上一次结束前忽略。
    if (committingRef.current) return;

    const next = Math.max(1, Math.round(draftValue ?? value.value));
    setDraftValue(next);
    if (next === value.value) return;

    committingRef.current = true;
    try {
      await onChange({ unit: value.unit, value: next });
    } finally {
      committingRef.current = false;
    }
  };

  const handleUnitChange = async (unit: RetentionUnit) => {
    await onChange(retentionWithUnit(value, unit));
  };

  if (forever) {
    return (
      <Select
        className="w-32"
        disabled={disabled}
        onChange={handleUnitChange}
        options={unitOptions}
        value="forever"
      />
    );
  }

  return (
    <Space.Compact>
      <InputNumber
        className="w-20"
        disabled={disabled}
        min={1}
        onBlur={commitValue}
        onChange={setDraftValue}
        onPressEnter={commitValue}
        precision={0}
        value={draftValue}
      />
      <Select
        className="w-28"
        disabled={disabled}
        onChange={handleUnitChange}
        options={unitOptions}
        value={value.unit}
      />
    </Space.Compact>
  );
};

export default RetentionDurationInput;
