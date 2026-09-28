import { Popover as AntdPopover, type PopoverProps } from "antd";
import type { FC } from "react";
import { useState } from "react";
import PopupKeyboardLayer from "@/components/PopupKeyboardLayer";
import Tooltip, {
  type OverlayTooltipConfig,
  resolveOverlayTooltipProps,
} from "@/components/Tooltip";

/**
 * 气泡卡片打开期间独占的键盘层；卡片开着时仍要响应的快捷键（如再按一次关闭卡片）登记在这一层。
 */
export const POPOVER_KEYBOARD_LAYER = "popover";

export interface AppPopoverProps extends PopoverProps {
  tooltip?: OverlayTooltipConfig | false;
}

/**
 * 包装触发节点 Tooltip，并在 Popover 打开时强制收起 Tooltip。
 */
const renderPopoverTrigger = (
  children: PopoverProps["children"],
  tooltip: OverlayTooltipConfig | false | undefined,
  open: boolean,
): PopoverProps["children"] => {
  if (tooltip === false || tooltip === null || tooltip === void 0) {
    return children;
  }

  const tooltipProps = resolveOverlayTooltipProps(tooltip);

  return (
    <Tooltip {...tooltipProps} open={open ? false : tooltipProps.open}>
      {children}
    </Tooltip>
  );
};

/**
 * antd Popover 的统一封装：默认开启 `align.overflow.shiftX/Y`，
 * 让浮层贴边时沿轴向平移避开窗口边界（箭头位置不动，仍指向 trigger 中心）。
 * 调用方与原生 Popover 完全兼容，传入的 `align` 会整体覆盖默认值。打开期间独占键盘。
 */
const Popover: FC<AppPopoverProps> = (props) => {
  const { align, children, onOpenChange, open, tooltip, ...rest } = props;
  const [innerOpen, setInnerOpen] = useState(false);
  const mergedOpen = open ?? innerOpen;

  const handleOpenChange: NonNullable<PopoverProps["onOpenChange"]> = (
    nextOpen,
  ) => {
    setInnerOpen(nextOpen);
    onOpenChange?.(nextOpen);
  };

  const close = () => {
    handleOpenChange(false);
  };

  return (
    <>
      <AntdPopover
        align={
          align ?? { overflow: { adjustY: true, shiftX: true, shiftY: true } }
        }
        onOpenChange={handleOpenChange}
        open={mergedOpen}
        {...rest}
      >
        {renderPopoverTrigger(children, tooltip, mergedOpen)}
      </AntdPopover>

      {mergedOpen ? (
        <PopupKeyboardLayer layer={POPOVER_KEYBOARD_LAYER} onClose={close} />
      ) : null}
    </>
  );
};

export default Popover;
