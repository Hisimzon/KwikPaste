import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useSnapshot } from "valtio";
import CustomIconButton from "@/components/CustomIconButton";
import KeyHint from "@/components/KeyHint";
import Popover, { POPOVER_KEYBOARD_LAYER } from "@/components/Popover";
import Tooltip from "@/components/Tooltip";
import {
  clipboardSelectionState,
  enterClipboardSelection,
} from "@/stores/clipboardSelection";
import { clipboardStatsState } from "@/stores/clipboardStats";
import ShortcutList from "./ShortcutList";
import StorageLimitAlert from "./StorageLimitAlert";

/**
 * 剪贴板窗口底部条：左侧统计当前过滤下的总条数（由 List 写入共享 store，
 * Rust 列表查询附带返回）和存储超限提示，右侧是多选入口与窗口快捷键提示。
 * 多选期间由列表渲染的多选栏接管这一行。
 */
const Footer = () => {
  const { t } = useTranslation("clipboard");
  const { total } = useSnapshot(clipboardStatsState);
  const { active: selecting } = useSnapshot(clipboardSelectionState);

  // Popover 打开时强制收起 Tooltip，避免两层浮层叠加遮挡。
  const [popoverOpen, setPopoverOpen] = useState(false);

  const handleShortcutKeyPress = () => {
    setPopoverOpen((prev) => {
      return !prev;
    });
  };

  if (selecting) return null;

  return (
    <div className="flex items-center justify-between px-3 py-1">
      <div className="flex min-w-0 items-center gap-3">
        <span className="text-ant-tertiary text-xs">
          {t("footer.total", { count: total ?? 0 })}
        </span>
        <StorageLimitAlert />
      </div>

      <div className="flex shrink-0 items-center gap-1">
        {/* 只展示 A 徽标：Ctrl / ⌘ + A 由列表处理，进入多选并全选。 */}
        <Tooltip title={t("footer.select")}>
          <CustomIconButton
            disabled={!total}
            icon={<KeyHint hintKey="A" iconName="i-lucide:list-checks" />}
            onClick={enterClipboardSelection}
            size="small"
            type="text"
          />
        </Tooltip>

        <Popover
          content={<ShortcutList />}
          onOpenChange={setPopoverOpen}
          open={popoverOpen}
          title={t("footer.shortcuts")}
          tooltip={t("footer.shortcuts")}
          trigger="click"
        >
          <CustomIconButton
            icon={
              <KeyHint
                hintKey="K"
                iconName="i-lucide:keyboard"
                layer={popoverOpen ? POPOVER_KEYBOARD_LAYER : void 0}
                onKeyPress={handleShortcutKeyPress}
              />
            }
            size="small"
            type="text"
          />
        </Popover>
      </div>
    </div>
  );
};

export default Footer;
