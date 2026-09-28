import { useDebounceFn } from "ahooks";
import { Button, Dropdown, type MenuProps, Switch } from "antd";
import type { FC } from "react";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSnapshot } from "valtio";
import {
  type CleanupPreview,
  previewHistoryCleanup,
  type RulePreview,
} from "@/commands";
import CustomIconButton from "@/components/CustomIconButton";
import Tooltip from "@/components/Tooltip";
import { TAURI_EVENT } from "@/constants/events";
import { useTauriListen } from "@/hooks/useTauriListen";
import { sourceAppsState } from "@/stores/sourceApps";
import type { RetentionRule, Settings } from "@/types/settings";
import { cn } from "@/utils/cn";
import type { PreferenceSetting } from "../../types/preferences";
import {
  cloneJson,
  createRetentionRule,
  formatRetention,
  RETENTION_RULE_PRESETS,
  summarizeRuleConditions,
} from "../../utils/retention";
import RetentionRuleModal, { placeRule } from "./RetentionRuleModal";
import type { ControlProps } from "./types";

/** 统计刷新防抖：连续采集或连续改设置时只统计一次。 */
const STATS_REFRESH_DEBOUNCE_MS = 800;
const CUSTOM_RULE_KEY = "custom";

interface RetentionRulesControlProps extends ControlProps {
  setting: PreferenceSetting;
  settings: Settings;
}

interface RuleEditorState {
  /** 为 `null` 时新建。 */
  rule: RetentionRule | null;
  template: RetentionRule;
}

/**
 * 自定义清理规则列表：启停、排序、编辑、删除与添加预设规则，并展示每条规则当前匹配的记录数。
 * 任何会立即删除记录的改动都由偏好页统一预演并确认。
 */
const RetentionRulesControl: FC<RetentionRulesControlProps> = (props) => {
  const { t } = useTranslation("preferences");
  const { disabled, setting, settings, onChange } = props;
  const history = settings.clipboard.history;
  const rules = history.rules;
  const historyKey = JSON.stringify(history);
  const { apps } = useSnapshot(sourceAppsState);
  const [preview, setPreview] = useState<CleanupPreview | null>(null);
  const [editor, setEditor] = useState<RuleEditorState | null>(null);
  const previewTokenRef = useRef(0);

  const { run: refreshStats } = useDebounceFn(
    async () => {
      const token = previewTokenRef.current + 1;
      previewTokenRef.current = token;

      try {
        const next = await previewHistoryCleanup(
          cloneJson(settings.clipboard.history),
        );
        if (previewTokenRef.current !== token) return;

        setPreview(next);
      } catch {
        setPreview(null);
      }
    },
    { wait: STATS_REFRESH_DEBOUNCE_MS },
  );

  // 挂载和清理设置变化后重新统计；historyKey 只作为变化信号。
  useEffect(() => {
    void historyKey;
    refreshStats();
  }, [historyKey, refreshStats]);

  useTauriListen(TAURI_EVENT.CLIPBOARD_UPDATED, () => {
    refreshStats();
  });

  /**
   * 来源应用 id → 展示名；未知应用取路径或 bundle id 的最后一段。
   */
  const appName = (appId: string) => {
    const app = apps.find((item) => {
      return item.id === appId;
    });
    if (app) return app.name;

    const segments = appId.split(/[\\/]/);
    const lastSegment = segments[segments.length - 1] || appId;

    return lastSegment.replace(/\.(exe|app)$/i, "");
  };

  const statsOf = (ruleId: string) => {
    return preview?.rules.find((item) => {
      return item.id === ruleId;
    });
  };

  const saveRules = async (nextRules: RetentionRule[]) => {
    const saved = await onChange(setting, nextRules);

    return saved !== false;
  };

  const openCustomEditor = () => {
    setEditor({ rule: null, template: createRetentionRule() });
  };

  const closeEditor = () => {
    setEditor(null);
  };

  const handleAddMenuClick: MenuProps["onClick"] = async (info) => {
    if (info.key === CUSTOM_RULE_KEY) {
      openCustomEditor();
      return;
    }

    const preset = RETENTION_RULE_PRESETS.find((item) => {
      return item.id === info.key;
    });
    if (!preset) return;

    await saveRules([preset.build(), ...rules]);
  };

  const submitEditor = async (rule: RetentionRule) => {
    if (!editor) return false;

    return await saveRules(placeRule(rules, rule, editor.rule === null));
  };

  const editRule = (rule: RetentionRule) => {
    setEditor({ rule, template: rule });
  };

  const toggleRule = async (ruleId: string, enabled: boolean) => {
    await saveRules(
      rules.map((rule) => {
        return rule.id === ruleId ? { ...rule, enabled } : rule;
      }),
    );
  };

  const moveRule = async (ruleId: string, offset: -1 | 1) => {
    const index = rules.findIndex((rule) => {
      return rule.id === ruleId;
    });
    const target = index + offset;
    if (index < 0 || target < 0 || target >= rules.length) return;

    const nextRules = [...rules];
    [nextRules[index], nextRules[target]] = [
      nextRules[target],
      nextRules[index],
    ];
    await saveRules(nextRules);
  };

  const deleteRule = async (ruleId: string) => {
    await saveRules(
      rules.filter((rule) => {
        return rule.id !== ruleId;
      }),
    );
  };

  const addMenuItems: MenuProps["items"] = [
    { key: CUSTOM_RULE_KEY, label: t("retentionRules.addCustom") },
    { type: "divider" },
    {
      children: RETENTION_RULE_PRESETS.map((preset) => {
        return {
          key: preset.id,
          label: t(`retentionRules.presets.${preset.id}`),
        };
      }),
      key: "presets",
      label: t("retentionRules.presets.title"),
      type: "group",
    },
  ];

  return (
    <div className="flex w-full min-w-0 flex-col gap-2">
      {rules.length > 0 ? (
        <ol className="m-0 flex list-none flex-col overflow-hidden rounded-2 border border-ant-border-secondary p-0">
          {rules.map((rule, index) => {
            return (
              <RuleItem
                disabled={disabled}
                first={index === 0}
                key={rule.id}
                last={index === rules.length - 1}
                onDelete={deleteRule}
                onEdit={editRule}
                onMove={moveRule}
                onToggle={toggleRule}
                rule={rule}
                stats={statsOf(rule.id)}
                summary={summarizeRuleConditions(t, rule, appName)}
              />
            );
          })}
          <li className="flex items-center gap-3 bg-ant-fill-quaternary px-3 py-2">
            <span className="w-7 shrink-0" />
            <div className="min-w-0 flex-1">
              <div className="truncate text-ant-secondary text-sm">
                {t("retentionRules.fallback")}
              </div>
              <RuleMeta
                keepText={formatRetention(t, history.retention)}
                stats={preview?.fallback}
              />
            </div>
          </li>
        </ol>
      ) : (
        <p className="m-0 text-ant-tertiary text-xs">
          {t("retentionRules.empty")}
        </p>
      )}

      <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
        <Dropdown
          disabled={disabled}
          menu={{ items: addMenuItems, onClick: handleAddMenuClick }}
          trigger={["click"]}
        >
          <Button icon={<i aria-hidden="true" className="i-lucide:plus" />}>
            {t("retentionRules.add")}
          </Button>
        </Dropdown>
        <span className="text-ant-tertiary text-xs">
          {t("retentionRules.order")}
        </span>
      </div>

      <RetentionRuleModal
        history={history}
        onCancel={closeEditor}
        onSubmit={submitEditor}
        open={editor !== null}
        rule={editor?.rule ?? null}
        template={editor?.template ?? EMPTY_TEMPLATE}
      />
    </div>
  );
};

/** 弹窗关闭时占位的模板，保持引用稳定，避免弹窗重复重置表单。 */
const EMPTY_TEMPLATE = createRetentionRule();

interface RuleItemProps {
  disabled: boolean;
  first: boolean;
  last: boolean;
  rule: RetentionRule;
  stats?: RulePreview;
  summary: string;
  onDelete: (ruleId: string) => Promise<void>;
  onEdit: (rule: RetentionRule) => void;
  onMove: (ruleId: string, offset: -1 | 1) => Promise<void>;
  onToggle: (ruleId: string, enabled: boolean) => Promise<void>;
}

/**
 * 规则列表里的一行：启停开关、条件摘要、保留时长与匹配统计、排序和编辑操作。
 */
const RuleItem: FC<RuleItemProps> = (props) => {
  const { t } = useTranslation("preferences");
  const {
    disabled,
    first,
    last,
    rule,
    stats,
    summary,
    onDelete,
    onEdit,
    onMove,
    onToggle,
  } = props;

  const handleToggle = async (enabled: boolean) => {
    await onToggle(rule.id, enabled);
  };

  const moveUp = async () => {
    await onMove(rule.id, -1);
  };

  const moveDown = async () => {
    await onMove(rule.id, 1);
  };

  const edit = () => {
    onEdit(rule);
  };

  const remove = async () => {
    await onDelete(rule.id);
  };

  return (
    <li className="flex items-center gap-3 border-ant-split border-b px-3 py-2">
      <Switch
        checked={rule.enabled}
        className="shrink-0"
        disabled={disabled}
        onChange={handleToggle}
        size="small"
      />
      <div className="min-w-0 flex-1">
        <div
          className={cn("truncate text-sm", {
            "text-ant-secondary": !rule.enabled,
          })}
          title={summary}
        >
          {summary}
        </div>
        {rule.enabled ? (
          <RuleMeta keepText={formatRetention(t, rule.keep)} stats={stats} />
        ) : (
          <div className="text-ant-tertiary text-xs">
            {t("retentionRules.disabled")}
          </div>
        )}
      </div>
      <div className="flex shrink-0 items-center">
        <Tooltip title={t("retentionRules.moveUp")}>
          <CustomIconButton
            aria-label={t("retentionRules.moveUp")}
            disabled={disabled || first}
            icon={<i aria-hidden="true" className="i-lucide:arrow-up" />}
            onClick={moveUp}
            size="small"
            type="text"
          />
        </Tooltip>
        <Tooltip title={t("retentionRules.moveDown")}>
          <CustomIconButton
            aria-label={t("retentionRules.moveDown")}
            disabled={disabled || last}
            icon={<i aria-hidden="true" className="i-lucide:arrow-down" />}
            onClick={moveDown}
            size="small"
            type="text"
          />
        </Tooltip>
        <Tooltip title={t("retentionRules.edit")}>
          <CustomIconButton
            aria-label={t("retentionRules.edit")}
            disabled={disabled}
            icon={<i aria-hidden="true" className="i-lucide:pencil" />}
            onClick={edit}
            size="small"
            type="text"
          />
        </Tooltip>
        <Tooltip title={t("retentionRules.delete")}>
          <CustomIconButton
            aria-label={t("retentionRules.delete")}
            danger
            disabled={disabled}
            icon={<i aria-hidden="true" className="i-lucide:trash-2" />}
            onClick={remove}
            size="small"
            type="text"
          />
        </Tooltip>
      </div>
    </li>
  );
};

interface RuleMetaProps {
  keepText: string;
  stats?: RulePreview;
}

/**
 * 规则第二行：保留时长 · 当前匹配条数（· 待清理条数）。
 */
const RuleMeta: FC<RuleMetaProps> = (props) => {
  const { t } = useTranslation("preferences");
  const { keepText, stats } = props;

  return (
    <div className="truncate text-ant-tertiary text-xs">
      {keepText}
      {stats ? (
        <>
          {" · "}
          {t("retentionRules.matched", { count: stats.matched })}
          {stats.expired > 0 ? (
            <span className="text-ant-warning">
              {" · "}
              {t("retentionRules.expired", { count: stats.expired })}
            </span>
          ) : null}
        </>
      ) : null}
    </div>
  );
};

export default RetentionRulesControl;
