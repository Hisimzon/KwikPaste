import { Button } from "antd";
import type { FC } from "react";
import { useTranslation } from "react-i18next";
import Tooltip from "@/components/Tooltip";
import { formatShortcutDisplay } from "@/utils/shortcut";

interface SelectionBarProps {
  count: number;
  /** 当前视图里能删的记录已全部选中，全选按钮切成取消全选。 */
  allChecked: boolean;
  /** 正在拉取全选范围或删除，按钮暂不可用。 */
  busy: boolean;
  onToggleAll: () => void;
  onDelete: () => void;
  onExit: () => void;
}

/**
 * 多选时替换底部栏：左侧显示已选条数，右侧是全选、删除和退出多选。
 */
const SelectionBar: FC<SelectionBarProps> = (props) => {
  const { allChecked, busy, count, onDelete, onExit, onToggleAll } = props;
  const { t } = useTranslation("clipboard");

  return (
    <div className="flex shrink-0 items-center justify-between gap-2 px-3 py-1">
      <span className="min-w-0 truncate text-ant-secondary text-xs">
        {count > 0 ? t("selection.selected", { count }) : t("selection.hint")}
      </span>

      <div className="flex shrink-0 items-center gap-1">
        <Tooltip title={formatShortcutDisplay("CmdOrCtrl+A")}>
          <Button
            disabled={busy}
            onClick={onToggleAll}
            size="small"
            type="text"
          >
            {t(allChecked ? "selection.clear" : "selection.selectAll")}
          </Button>
        </Tooltip>

        <Tooltip title={formatShortcutDisplay("CmdOrCtrl+Backspace")}>
          <Button
            danger
            disabled={busy || count === 0}
            onClick={onDelete}
            size="small"
          >
            {t("selection.delete")}
          </Button>
        </Tooltip>

        <Tooltip title={formatShortcutDisplay("Escape")}>
          <Button onClick={onExit} size="small" type="text">
            {t("selection.exit")}
          </Button>
        </Tooltip>
      </div>
    </div>
  );
};

export default SelectionBar;
