import { Button, Modal } from "antd";
import type { FC } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import { useSnapshot } from "valtio";
import AssetImage from "@/components/AssetImage";
import { sourceAppsState } from "@/stores/sourceApps";
import type { ClipboardApp } from "@/types/clipboard";
import type { Settings } from "@/types/settings";
import type { PreferenceSetting } from "../../types/preferences";
import SourceAppsTransfer from "../SourceAppsTransfer";
import type { ControlProps } from "./types";

/** 行内最多叠放几个已忽略应用的图标，其余只体现在数量里。 */
const PREVIEW_ICON_LIMIT = 4;

interface AppExclusionControlProps extends ControlProps {
  setting: PreferenceSetting;
  settings: Settings;
}

/**
 * 忽略应用设置行：行内只展示已忽略的应用概况，选择应用的穿梭框放进弹窗，页面里不再嵌套滚动区。
 */
const AppExclusionControl: FC<AppExclusionControlProps> = (props) => {
  const { t } = useTranslation("preferences");
  const { disabled, onChange, setting, settings } = props;
  const sourceApps = useSnapshot(sourceAppsState);
  const [open, setOpen] = useState(false);
  const excludedAppIds = settings.clipboard.filters.excludedAppIds;
  const previewApps = excludedAppIds
    .flatMap((id) => {
      const app = sourceApps.apps.find((item) => {
        return item.id === id;
      });

      return app ? [app] : [];
    })
    .slice(0, PREVIEW_ICON_LIMIT);

  const openModal = () => {
    setOpen(true);
  };

  const closeModal = () => {
    setOpen(false);
  };

  return (
    <div className="flex items-center gap-3">
      {excludedAppIds.length > 0 ? (
        <span className="flex items-center gap-2 text-ant-secondary text-sm">
          <span className="flex items-center -space-x-1.5">
            {previewApps.map((app) => {
              return <AppIcon app={app} key={app.id} />;
            })}
          </span>
          {t("schema.settings.source.excludedApps.count", {
            count: excludedAppIds.length,
          })}
        </span>
      ) : null}

      <Button disabled={disabled} onClick={openModal}>
        {t("schema.settings.source.excludedApps.controlLabel")}
      </Button>

      <Modal
        destroyOnHidden
        footer={null}
        onCancel={closeModal}
        open={open}
        title={t("schema.settings.source.excludedApps.title")}
        width="45rem"
      >
        {/* 系统文本放大后窗口视口变矮，穿梭框跟着缩短，弹窗不超出窗口。 */}
        <div className="h-105 max-h-[calc(100vh-11rem)]">
          <SourceAppsTransfer
            excludedAppsSetting={setting}
            onChange={onChange}
            settings={settings}
          />
        </div>
      </Modal>
    </div>
  );
};

export default AppExclusionControl;

interface AppIconProps {
  app: ClipboardApp;
}

/**
 * 叠放预览用的应用小图标，带底色描边区分相邻图标；没有图标时用通用窗口图形。
 */
const AppIcon: FC<AppIconProps> = (props) => {
  const { app } = props;

  return (
    <span
      className="flex size-6 shrink-0 items-center justify-center overflow-hidden rounded-1.5 bg-ant-container ring-2 ring-ant-container"
      title={app.name}
    >
      {app.iconPath ? (
        <AssetImage alt={app.name} className="size-6" src={app.iconPath} />
      ) : (
        <i aria-hidden="true" className="i-lucide:app-window text-base" />
      )}
    </span>
  );
};
