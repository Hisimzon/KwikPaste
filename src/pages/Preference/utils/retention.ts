import type { TFunction } from "i18next";
import type { ContentCategory } from "@/types/clipboard";
import type {
  History,
  Retention,
  RetentionRule,
  RetentionUnit,
} from "@/types/settings";

type PreferenceTranslator = TFunction<"preferences">;

/** 默认保留时长可选单位：只用已发布版本认识的单位。 */
export const DEFAULT_RETENTION_UNITS: RetentionUnit[] = [
  "hours",
  "days",
  "weeks",
  "months",
  "forever",
];

/** 自定义规则可选单位，比默认保留时长多出分钟，便于让敏感内容很快过期。 */
export const RULE_RETENTION_UNITS: RetentionUnit[] = [
  "minutes",
  ...DEFAULT_RETENTION_UNITS,
];

/** 从「永久保留」切到具体单位时预填的数值。 */
const UNIT_DEFAULT_VALUES: Record<Exclude<RetentionUnit, "forever">, number> = {
  days: 30,
  hours: 24,
  minutes: 30,
  months: 3,
  weeks: 4,
};

/** 文本类内容类别，顺序即规则表单里的展示顺序。 */
export const TEXT_CATEGORIES: ContentCategory[] = [
  "text",
  "html",
  "rtf",
  "url",
  "email",
  "color",
  "path",
];

const KB_PER_MB = 1024;

/**
 * 保留时长是否表示「不按时间清理」：单位为永久或数值为 0。
 */
export function isForeverRetention(retention: Retention) {
  return retention.unit === "forever" || retention.value <= 0;
}

/**
 * 切换单位后的保留时长：切到永久清零数值，从永久切出时预填该单位的常用值。
 */
export function retentionWithUnit(
  retention: Retention,
  unit: RetentionUnit,
): Retention {
  if (unit === "forever") return { unit, value: 0 };
  if (isForeverRetention(retention)) {
    return { unit, value: UNIT_DEFAULT_VALUES[unit] };
  }

  return { unit, value: retention.value };
}

/**
 * 把保留时长格式化成「3 天」「永久保留」这类短文案。
 */
export function formatRetention(t: PreferenceTranslator, retention: Retention) {
  if (isForeverRetention(retention)) {
    return t("retentionRules.summary.keepForever");
  }

  const duration = t(`retentionRules.durations.${retention.unit}`, {
    count: retention.value,
  });

  return t("retentionRules.summary.keep", { duration });
}

/**
 * 大小（KB）格式化：整 MB 显示 MB，其余显示 KB，与规则表单里填的数值一致。
 */
export function formatSizeKb(sizeKb: number) {
  const { unit, value } = splitSizeKb(sizeKb);

  return `${value ?? 0} ${unit === "mb" ? "MB" : "KB"}`;
}

/**
 * 把 KB 拆成表单里的「数值 + 单位」：能整除 1 MB 时用 MB。
 */
export function splitSizeKb(sizeKb: number): {
  unit: "kb" | "mb";
  value: number | null;
} {
  if (sizeKb <= 0) return { unit: "mb", value: null };
  if (sizeKb % KB_PER_MB === 0)
    return { unit: "mb", value: sizeKb / KB_PER_MB };

  return { unit: "kb", value: sizeKb };
}

/**
 * 表单里的「数值 + 单位」换回 KB；空值或非正数表示不限。
 */
export function joinSizeKb(value: number | null, unit: "kb" | "mb") {
  if (value === null || value <= 0) return 0;

  return Math.round(unit === "mb" ? value * KB_PER_MB : value);
}

/**
 * 规则是否覆盖全部文本子类型，用于摘要与类型选择器里折叠成「全部文本」。
 */
export function coversAllText(categories: ContentCategory[]) {
  return TEXT_CATEGORIES.every((category) => {
    return categories.includes(category);
  });
}

/**
 * 生成规则条件摘要，如「图片 · 大于 5 MB · 来自 微信」；没有条件时为「全部记录」。
 */
export function summarizeRuleConditions(
  t: PreferenceTranslator,
  rule: RetentionRule,
  appName: (appId: string) => string,
) {
  const parts: string[] = [];
  const categories = [...rule.categories];

  if (coversAllText(categories)) {
    parts.push(t("retentionRules.summary.allText"));
  }

  const restCategories = coversAllText(categories)
    ? categories.filter((category) => {
        return !TEXT_CATEGORIES.includes(category);
      })
    : categories;
  if (restCategories.length > 0) {
    parts.push(
      restCategories
        .map((category) => {
          return t(`overview.categories.names.${category}`);
        })
        .join(" / "),
    );
  }

  if (rule.minSizeKb > 0) {
    parts.push(
      t("retentionRules.summary.larger", {
        size: formatSizeKb(rule.minSizeKb),
      }),
    );
  }

  if (rule.sourceAppIds.length > 0) {
    const [firstAppId] = rule.sourceAppIds;
    const apps =
      rule.sourceAppIds.length === 1
        ? appName(firstAppId)
        : t("retentionRules.summary.moreApps", {
            count: rule.sourceAppIds.length,
            first: appName(firstAppId),
          });
    parts.push(t("retentionRules.summary.apps", { apps }));
  }

  if (rule.sensitiveOnly) parts.push(t("retentionRules.summary.sensitive"));
  if (rule.unusedOnly) parts.push(t("retentionRules.summary.unused"));

  if (parts.length === 0) return t("retentionRules.summary.all");

  return parts.join(" · ");
}

/**
 * 新建一条启用的规则；id 在前端生成，保存后用于对应逐条统计。
 */
export function createRetentionRule(
  patch: Partial<Omit<RetentionRule, "id">> = {},
): RetentionRule {
  return {
    categories: [],
    enabled: true,
    id: crypto.randomUUID(),
    keep: { unit: "days", value: 7 },
    minSizeKb: 0,
    sensitiveOnly: false,
    sourceAppIds: [],
    unusedOnly: false,
    ...patch,
  };
}

export type RetentionRulePresetId =
  | "largeImages"
  | "images"
  | "secrets"
  | "unusedText";

/** 常用规则预设，添加规则菜单里一键插入。 */
export const RETENTION_RULE_PRESETS: {
  id: RetentionRulePresetId;
  build: () => RetentionRule;
}[] = [
  {
    build: () => {
      return createRetentionRule({
        categories: ["image"],
        keep: { unit: "days", value: 3 },
        minSizeKb: 5 * KB_PER_MB,
      });
    },
    id: "largeImages",
  },
  {
    build: () => {
      return createRetentionRule({
        categories: ["image"],
        keep: { unit: "days", value: 30 },
      });
    },
    id: "images",
  },
  {
    build: () => {
      return createRetentionRule({
        keep: { unit: "hours", value: 1 },
        sensitiveOnly: true,
      });
    },
    id: "secrets",
  },
  {
    build: () => {
      return createRetentionRule({
        categories: [...TEXT_CATEGORIES],
        keep: { unit: "days", value: 7 },
        unusedOnly: true,
      });
    },
    id: "unusedText",
  },
];

/**
 * 把设置路径上的新值套到当前清理设置上，得到保存后的完整候选设置，供预演清理。
 * 只处理 `clipboard.history` 下的直接字段。
 */
export function buildNextHistory(
  history: History,
  path: readonly string[],
  value: unknown,
): History | null {
  const [root, section, field] = path;
  if (root !== "clipboard" || section !== "history" || !field) return null;
  if (path.length !== 3) return null;

  // 设置镜像来自 valtio 快照（可能是 Proxy），structuredClone 无法复制，走 JSON 深拷贝。
  return { ...cloneJson(history), [field]: cloneJson(value) };
}

/**
 * JSON 深拷贝，兼容 valtio 快照 Proxy。
 */
export function cloneJson<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}
