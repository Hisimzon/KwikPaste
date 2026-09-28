import { useMount } from "ahooks";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useSnapshot } from "valtio";
import {
  type CleanupStatus,
  getCleanupStatus,
  openPreferenceWithHighlight,
  type StorageCheck,
} from "@/commands";
import Tooltip from "@/components/Tooltip";
import { TAURI_EVENT } from "@/constants/events";
import { useTauriListen } from "@/hooks/useTauriListen";
import { settingsState } from "@/stores/settings";
import { log } from "@/utils/log";

const STORAGE_LIMIT_SETTING_ID = "localData.storageLimit";

/**
 * 存储超出上限时在底部条提示：「仅提醒」模式一直提示，「自动清理」模式只在清理也回不到上限以内时提示。
 * 点击打开偏好设置并定位到存储上限。
 */
const StorageLimitAlert = () => {
  const { t } = useTranslation("clipboard");
  const { storageLimitAction } = useSnapshot(settingsState).clipboard.history;
  const [storage, setStorage] = useState<StorageCheck | null>(null);

  useMount(async () => {
    try {
      const status = await getCleanupStatus();
      setStorage(status.storage);
    } catch (error) {
      log.warn("load cleanup status failed", error);
    }
  });

  useTauriListen<CleanupStatus>(TAURI_EVENT.CLEANUP_STATUS, (event) => {
    setStorage(event.payload.storage);
  });

  const openStorageSettings = async () => {
    try {
      await openPreferenceWithHighlight(STORAGE_LIMIT_SETTING_ID);
    } catch {
      // 错误 toast 已由 commands 层统一处理。
    }
  };

  if (!storage?.overLimit) return null;

  const blocked = storageLimitAction === "cleanup";
  if (blocked && !storage.cleanupBlocked) return null;

  const tooltip = t(
    blocked ? "footer.storageAlert.blocked" : "footer.storageAlert.remind",
    {
      limit: formatStorageBytes(storage.limitBytes),
      used: formatStorageBytes(storage.usedBytes),
    },
  );

  return (
    <Tooltip title={tooltip}>
      <button
        className="flex cursor-pointer items-center gap-1 border-none bg-transparent p-0 text-ant-warning text-xs hover:text-ant-warning-hover"
        onClick={openStorageSettings}
        type="button"
      >
        <i aria-hidden="true" className="i-lucide:triangle-alert" />
        {t("footer.storageAlert.label")}
      </button>
    </Tooltip>
  );
};

/**
 * 占用字节格式化成 MB / GB。
 */
function formatStorageBytes(bytes: number) {
  const mb = bytes / 1024 / 1024;
  if (mb < 1024) return `${Math.round(mb)} MB`;

  return `${(mb / 1024).toFixed(1)} GB`;
}

export default StorageLimitAlert;
