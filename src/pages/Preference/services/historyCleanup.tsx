import { type CleanupPreview, previewHistoryCleanup } from "@/commands";
import i18n from "@/i18n";
import type { History } from "@/types/settings";
import { getModalApi } from "@/utils/feedback";
import { formatBytes } from "../utils/storageUsage";

/**
 * 保存清理设置前预演一轮清理：会立即删除记录时弹窗确认，返回是否继续保存。
 * 预演失败时（错误已由命令层提示）仍给出一次通用确认，不静默删除。
 */
export async function confirmHistoryCleanupChange(nextHistory: History) {
  let preview: CleanupPreview | null = null;

  try {
    preview = await previewHistoryCleanup(nextHistory);
  } catch {
    preview = null;
  }

  if (preview?.removed === 0) return true;

  return await askToContinue(preview);
}

/**
 * 弹出确认框并等待用户选择。
 */
function askToContinue(preview: CleanupPreview | null) {
  const t = i18n.getFixedT(null, "preferences");

  return new Promise<boolean>((resolve) => {
    getModalApi().confirm({
      cancelText: t("common:actions.cancel"),
      content: preview ? (
        <div className="flex flex-col gap-1">
          <span>
            {preview.freedBytes > 0
              ? t("cleanupConfirm.freed", {
                  size: formatBytes(preview.freedBytes),
                })
              : null}
            {t("cleanupConfirm.content")}
          </span>
          <span className="text-ant-secondary text-xs">
            {t("cleanupConfirm.breakdown", {
              expired: preview.expired,
              overCount: preview.overCount,
              overStorage: preview.overStorage,
            })}
          </span>
        </div>
      ) : (
        t("cleanupConfirm.unknownContent")
      ),
      okButtonProps: { danger: true },
      okText: t("cleanupConfirm.ok"),
      onCancel: () => {
        resolve(false);
      },
      onOk: () => {
        resolve(true);
      },
      title: preview
        ? t("cleanupConfirm.title", { count: preview.removed })
        : t("cleanupConfirm.unknownTitle"),
    });
  });
}
