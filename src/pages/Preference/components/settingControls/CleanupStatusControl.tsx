import { useMount } from "ahooks";
import { Button } from "antd";
import type { TFunction } from "i18next";
import type { FC } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  type CleanupReport,
  type CleanupStatus,
  getCleanupStatus,
  runHistoryCleanup,
} from "@/commands";
import Tooltip from "@/components/Tooltip";
import { TAURI_EVENT } from "@/constants/events";
import { useTauriListen } from "@/hooks/useTauriListen";
import type { Language } from "@/types/settings";
import { log } from "@/utils/log";
import type { PreferenceSetting } from "../../types/preferences";
import { translatePreferenceControlLabel } from "../../utils/preferenceI18n";
import { formatBytes } from "../../utils/storageUsage";

interface CleanupStatusControlProps {
  disabled: boolean;
  language: Language;
  setting: PreferenceSetting;
}

/**
 * 清理状态：展示本次启动后最近一次清理的时间与条数，提供立即清理入口。
 */
const CleanupStatusControl: FC<CleanupStatusControlProps> = (props) => {
  const { t } = useTranslation("preferences");
  const { disabled, language, setting } = props;
  const [status, setStatus] = useState<CleanupStatus | null>(null);
  const [running, setRunning] = useState(false);
  const lastRun = status?.lastRun ?? null;
  const storageBlocked = status?.storage?.cleanupBlocked === true;

  useMount(async () => {
    try {
      setStatus(await getCleanupStatus());
    } catch (error) {
      log.warn("load cleanup status failed", error);
    }
  });

  useTauriListen<CleanupStatus>(TAURI_EVENT.CLEANUP_STATUS, (event) => {
    setStatus(event.payload);
  });

  const runNow = async () => {
    setRunning(true);

    try {
      await runHistoryCleanup();
    } catch {
      // 错误 toast 已由 commands 层统一处理。
    } finally {
      setRunning(false);
    }
  };

  const summary = lastRun
    ? t("cleanupStatus.lastRun", {
        count: lastRun.removed,
        time: formatFinishedAt(t, lastRun, language),
      })
    : t("cleanupStatus.never");
  const detail = [
    lastRun
      ? t("cleanupStatus.lastRunDetail", {
          expired: lastRun.expired,
          overCount: lastRun.overCount,
          overStorage: lastRun.overStorage,
          size: formatBytes(lastRun.freedBytes),
        })
      : null,
    storageBlocked ? t("cleanupStatus.storageBlocked") : null,
  ]
    .filter(Boolean)
    .join("\n");

  return (
    <div className="flex items-center gap-3">
      <Tooltip
        classNames={{ container: "whitespace-pre-line" }}
        title={detail || null}
      >
        <span className="flex items-center gap-1 text-ant-secondary text-xs">
          {storageBlocked ? (
            <i
              aria-hidden="true"
              className="i-lucide:triangle-alert text-ant-warning"
            />
          ) : null}
          {summary}
        </span>
      </Tooltip>
      <Button disabled={disabled} loading={running} onClick={runNow}>
        {translatePreferenceControlLabel(t, setting)}
      </Button>
    </div>
  );
};

/**
 * 清理完成时间：今天只显示时刻，其余显示日期和时刻。
 */
function formatFinishedAt(
  t: TFunction<"preferences">,
  report: CleanupReport,
  language: Language,
) {
  const finishedAt = new Date(report.finishedAt);
  const time = new Intl.DateTimeFormat(language, {
    hour: "2-digit",
    minute: "2-digit",
  }).format(finishedAt);

  if (finishedAt.toDateString() === new Date().toDateString()) {
    return t("cleanupStatus.today", { time });
  }

  return new Intl.DateTimeFormat(language, {
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    month: "numeric",
  }).format(finishedAt);
}

export default CleanupStatusControl;
