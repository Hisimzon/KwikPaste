import { useDebounceFn } from "ahooks";
import {
  Checkbox,
  Form,
  InputNumber,
  Modal,
  Select,
  Space,
  TreeSelect,
} from "antd";
import type { FC } from "react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSnapshot } from "valtio";
import { previewHistoryCleanup, type RulePreview } from "@/commands";
import AssetImage from "@/components/AssetImage";
import ModalKeyboardLayer from "@/components/ModalKeyboardLayer";
import { sourceAppsState } from "@/stores/sourceApps";
import type { ContentCategory } from "@/types/clipboard";
import type { History, Retention, RetentionRule } from "@/types/settings";
import {
  cloneJson,
  coversAllText,
  joinSizeKb,
  RULE_RETENTION_UNITS,
  splitSizeKb,
  TEXT_CATEGORIES,
} from "../../utils/retention";
import { CATEGORY_META } from "../../utils/storageOverview";
import RetentionDurationInput from "./RetentionDurationInput";

/** 类型选择器里代表「全部文本」的父节点。 */
const ALL_TEXT_NODE = "allText";
const PREVIEW_DEBOUNCE_MS = 400;

type SizeUnit = "kb" | "mb";

interface RuleFormValues {
  categories: string[];
  keep: Retention;
  sensitiveOnly: boolean;
  sizeUnit: SizeUnit;
  sizeValue: number | null;
  sourceAppIds: string[];
  unusedOnly: boolean;
}

interface RetentionRuleModalProps {
  history: History;
  open: boolean;
  /** 编辑的规则；为 `null` 时新建，保存后放在最上方。 */
  rule: RetentionRule | null;
  template: RetentionRule;
  onCancel: () => void;
  /** 返回 `false` 表示没有保存（用户取消确认），弹窗保持打开。 */
  onSubmit: (rule: RetentionRule) => Promise<boolean>;
}

/**
 * 把规则里的类别展开成类型选择器的值：全部文本折叠成父节点。
 */
function toTreeValue(categories: ContentCategory[]) {
  if (!coversAllText(categories)) return [...categories];

  return [
    ALL_TEXT_NODE,
    ...categories.filter((category) => {
      return !TEXT_CATEGORIES.includes(category);
    }),
  ];
}

/**
 * 类型选择器的值换回规则类别：父节点展开成全部文本子类型。
 */
function fromTreeValue(values: string[]) {
  return values.flatMap((value) => {
    if (value === ALL_TEXT_NODE) return TEXT_CATEGORIES;

    return [value as ContentCategory];
  });
}

/**
 * 规则 → 表单初值。
 */
function toFormValues(rule: RetentionRule): RuleFormValues {
  const size = splitSizeKb(rule.minSizeKb);

  return {
    categories: toTreeValue(rule.categories),
    keep: rule.keep,
    sensitiveOnly: rule.sensitiveOnly,
    sizeUnit: size.unit,
    sizeValue: size.value,
    sourceAppIds: [...rule.sourceAppIds],
    unusedOnly: rule.unusedOnly,
  };
}

/**
 * 表单值 → 规则，保留原规则的 id 与启用状态。
 */
function toRule(base: RetentionRule, values: RuleFormValues): RetentionRule {
  return {
    ...base,
    categories: fromTreeValue(values.categories ?? []),
    keep: values.keep,
    minSizeKb: joinSizeKb(values.sizeValue, values.sizeUnit),
    sensitiveOnly: values.sensitiveOnly === true,
    sourceAppIds: values.sourceAppIds ?? [],
    unusedOnly: values.unusedOnly === true,
  };
}

/**
 * 把编辑中的规则放进规则列表：编辑时原位替换，新建时放在最上方。
 */
export function placeRule(
  rules: readonly RetentionRule[],
  rule: RetentionRule,
  isNew: boolean,
) {
  if (isNew) return [rule, ...rules];

  return rules.map((item) => {
    return item.id === rule.id ? rule : item;
  });
}

/**
 * 自定义清理规则编辑弹窗：内容类型、大小、来源应用、敏感 / 未复用条件与保留时长，
 * 并实时预估这条规则当前能匹配多少记录。
 */
const RetentionRuleModal: FC<RetentionRuleModalProps> = (props) => {
  const { t } = useTranslation(["preferences", "common"]);
  const { history, open, rule, template, onCancel, onSubmit } = props;
  const [form] = Form.useForm<RuleFormValues>();
  const { apps } = useSnapshot(sourceAppsState);
  const [submitting, setSubmitting] = useState(false);
  const [rulePreview, setRulePreview] = useState<RulePreview | null>(null);
  const previewTokenRef = useRef(0);
  const values = Form.useWatch([], form);
  const base = rule ?? template;
  const isNew = rule === null;
  const categories = fromTreeValue(values?.categories ?? []);
  const sizeSet = (values?.sizeValue ?? 0) > 0;
  const filesWithSize =
    sizeSet && (categories.length === 0 || categories.includes("files"));

  const { run: schedulePreview } = useDebounceFn(
    async () => {
      const current = form.getFieldsValue(true) as RuleFormValues;
      if (!current.keep) return;

      const candidate = toRule(base, current);
      const nextHistory = {
        ...cloneJson(history),
        rules: placeRule(history.rules, candidate, isNew),
      };
      const token = previewTokenRef.current + 1;
      previewTokenRef.current = token;

      try {
        const preview = await previewHistoryCleanup(nextHistory);
        if (previewTokenRef.current !== token) return;

        setRulePreview(
          preview.rules.find((item) => {
            return item.id === candidate.id;
          }) ?? null,
        );
      } catch {
        setRulePreview(null);
      }
    },
    { wait: PREVIEW_DEBOUNCE_MS },
  );

  useEffect(() => {
    if (!open) return;

    form.setFieldsValue(toFormValues(base));
    setRulePreview(null);
  }, [base, form, open]);

  useEffect(() => {
    if (!open || !values) return;

    schedulePreview();
  }, [open, schedulePreview, values]);

  const treeData = [
    {
      children: TEXT_CATEGORIES.map((category) => {
        return {
          title: t(CATEGORY_META[category].labelKey),
          value: category,
        };
      }),
      title: t("retentionRules.summary.allText"),
      value: ALL_TEXT_NODE,
    },
    { title: t(CATEGORY_META.image.labelKey), value: "image" },
    { title: t(CATEGORY_META.files.labelKey), value: "files" },
  ];

  const knownAppIds = new Set(
    apps.map((app) => {
      return app.id;
    }),
  );
  const appOptions = [
    ...apps.map((app) => {
      return { iconPath: app.iconPath, label: app.name, value: app.id };
    }),
    ...(values?.sourceAppIds ?? [])
      .filter((appId) => {
        return !knownAppIds.has(appId);
      })
      .map((appId) => {
        return { iconPath: null, label: appId, value: appId };
      }),
  ];

  const sizeUnitOptions = [
    { label: "KB", value: "kb" },
    { label: "MB", value: "mb" },
  ];

  const handleSubmit = async () => {
    const submitted = await form.validateFields();

    setSubmitting(true);

    try {
      const saved = await onSubmit(toRule(base, submitted));
      if (saved) onCancel();
    } finally {
      setSubmitting(false);
    }
  };

  const renderAppOption = (option: {
    data: { iconPath: string | null; label: string };
  }) => {
    return (
      <span className="flex min-w-0 items-center gap-2">
        {option.data.iconPath ? (
          <AssetImage
            alt=""
            className="size-4 shrink-0 object-contain"
            src={option.data.iconPath}
          />
        ) : (
          <i
            aria-hidden="true"
            className="i-lucide:app-window shrink-0 text-ant-secondary"
          />
        )}
        <span className="truncate">{option.data.label}</span>
      </span>
    );
  };

  return (
    <Modal
      confirmLoading={submitting}
      destroyOnHidden
      mask={{ closable: false }}
      okText={t("common:actions.save")}
      onCancel={onCancel}
      onOk={handleSubmit}
      open={open}
      title={t(
        isNew
          ? "retentionRules.form.titleCreate"
          : "retentionRules.form.titleEdit",
      )}
    >
      <ModalKeyboardLayer />

      <Form<RuleFormValues>
        form={form}
        initialValues={toFormValues(base)}
        layout="vertical"
      >
        <Form.Item
          label={t("retentionRules.form.categories")}
          name="categories"
        >
          <TreeSelect
            allowClear
            maxTagCount="responsive"
            placeholder={t("retentionRules.form.categoriesPlaceholder")}
            showCheckedStrategy={TreeSelect.SHOW_PARENT}
            treeCheckable
            treeData={treeData}
            treeDefaultExpandAll={false}
          />
        </Form.Item>

        <Form.Item
          extra={filesWithSize ? t("retentionRules.form.filesNoSize") : null}
          label={t("retentionRules.form.minSize")}
        >
          <Space.Compact>
            <Form.Item name="sizeValue" noStyle>
              <InputNumber
                className="w-32"
                min={0}
                placeholder={t("retentionRules.form.minSizePlaceholder")}
                precision={0}
              />
            </Form.Item>
            <Form.Item name="sizeUnit" noStyle>
              <Select className="w-20" options={sizeUnitOptions} />
            </Form.Item>
          </Space.Compact>
        </Form.Item>

        <Form.Item label={t("retentionRules.form.apps")} name="sourceAppIds">
          <Select
            allowClear
            maxTagCount="responsive"
            mode="multiple"
            optionFilterProp="label"
            optionRender={renderAppOption}
            options={appOptions}
            placeholder={t("retentionRules.form.appsPlaceholder")}
            showSearch
          />
        </Form.Item>

        <Form.Item label={t("retentionRules.form.conditions")}>
          <div className="flex flex-col gap-1">
            <Form.Item name="sensitiveOnly" noStyle valuePropName="checked">
              <Checkbox>{t("retentionRules.form.sensitiveOnly")}</Checkbox>
            </Form.Item>
            <Form.Item name="unusedOnly" noStyle valuePropName="checked">
              <Checkbox>{t("retentionRules.form.unusedOnly")}</Checkbox>
            </Form.Item>
          </div>
        </Form.Item>

        <Form.Item
          extra={t("retentionRules.form.keepHint")}
          label={t("retentionRules.form.keep")}
          name="keep"
        >
          <KeepField fallback={base.keep} />
        </Form.Item>
      </Form>

      <p className="m-0 text-ant-secondary text-xs">
        {rulePreview === null
          ? null
          : rulePreview.matched === 0
            ? t("retentionRules.form.previewNone")
            : t("retentionRules.form.preview", {
                expired: rulePreview.expired,
                matched: rulePreview.matched,
              })}
      </p>
    </Modal>
  );
};

interface KeepFieldProps {
  fallback: Retention;
  value?: Retention;
  onChange?: (next: Retention) => void;
}

/**
 * 表单里的保留时长字段：`Form.Item` 注入 value / onChange。
 */
const KeepField: FC<KeepFieldProps> = (props) => {
  const { fallback, value, onChange } = props;

  const handleChange = (next: Retention) => {
    onChange?.(next);
  };

  return (
    <RetentionDurationInput
      onChange={handleChange}
      units={RULE_RETENTION_UNITS}
      value={value ?? fallback}
    />
  );
};

export default RetentionRuleModal;
