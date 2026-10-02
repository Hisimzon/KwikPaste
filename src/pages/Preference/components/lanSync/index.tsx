import { useMount } from "ahooks";
import { Alert, Button, Spin } from "antd";
import type { FC, ReactNode } from "react";
import { useState } from "react";
import { useTranslation } from "react-i18next";
import {
  getLanSyncState,
  refreshLanPairingCode,
  removeLanDevice,
} from "@/commands";
import CustomIconButton from "@/components/CustomIconButton";
import Tooltip from "@/components/Tooltip";
import { TAURI_EVENT } from "@/constants/events";
import { useTauriListen } from "@/hooks/useTauriListen";
import type { ClipboardPlatform } from "@/types/clipboard";
import type { Settings } from "@/types/settings";
import type {
  LanDevice,
  LanNearbyDevice,
  LanPairedPayload,
  LanSyncState,
} from "@/types/sync";
import { cn } from "@/utils/cn";
import { getMessageApi, getModalApi } from "@/utils/feedback";
import { isWin } from "@/utils/is";
import LanPairModal, { type LanPairModalTarget } from "./LanPairModal";

export const LAN_SYNC_PANEL_SETTING_ID = "sync.lan.devices";
const HIDDEN_CODE = "••••••";

interface LanSyncPanelProps {
  settings: Settings;
}

/**
 * 偏好页「同步」里的设备区：本机地址与配对码、已配对设备、附近设备。
 * 状态全部来自 Rust，变化时由 `sync://lan-state` 推送。
 */
const LanSyncPanel: FC<LanSyncPanelProps> = (props) => {
  const { t, i18n } = useTranslation(["preferences", "common"]);
  const { settings } = props;
  const [state, setState] = useState<LanSyncState | null>(null);
  const [codeHidden, setCodeHidden] = useState(false);
  const [refreshingCode, setRefreshingCode] = useState(false);
  const [pairTarget, setPairTarget] = useState<LanPairModalTarget | null>(null);
  const enabled = settings.sync.lan.enabled;

  useMount(async () => {
    try {
      setState(await getLanSyncState());
    } catch {
      // 错误 toast 已由 commands 层统一处理。
    }
  });

  useTauriListen<LanSyncState>(TAURI_EVENT.LAN_SYNC_STATE, (event) => {
    setState(event.payload);
  });

  useTauriListen<LanPairedPayload>(TAURI_EVENT.LAN_SYNC_PAIRED, (event) => {
    getMessageApi().success(
      t("lanSync.pairedByOther", { name: event.payload.name }),
    );
  });

  const toggleCodeHidden = () => {
    setCodeHidden((hidden) => {
      return !hidden;
    });
  };

  const refreshCode = async () => {
    setRefreshingCode(true);
    try {
      setState(await refreshLanPairingCode());
    } catch {
      // 错误 toast 已由 commands 层统一处理。
    } finally {
      setRefreshingCode(false);
    }
  };

  const openManualPair = () => {
    setPairTarget({ kind: "manual" });
  };

  const openDevicePair = (device: LanNearbyDevice) => {
    setPairTarget({ device, kind: "device" });
  };

  const closePairModal = () => {
    setPairTarget(null);
  };

  const confirmRemove = (device: LanDevice) => {
    const removeDevice = async () => {
      try {
        await removeLanDevice(device.id, device.name);
      } catch {
        // 错误 toast 已由 commands 层统一处理。
      }
    };

    getModalApi().confirm({
      cancelText: t("common:actions.cancel"),
      centered: true,
      content: t("lanSync.devices.removeConfirmContent"),
      okButtonProps: { danger: true },
      okText: t("lanSync.devices.remove"),
      onOk: removeDevice,
      title: t("lanSync.devices.removeConfirmTitle", { name: device.name }),
    });
  };

  const formatLastSeen = (value: string | null) => {
    if (!value) return t("lanSync.devices.offline");

    const time = new Date(value).toLocaleString(i18n.language, {
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
      month: "numeric",
    });

    return t("lanSync.devices.lastSeen", { time });
  };

  if (!enabled) {
    return (
      <SubSection
        settingId={LAN_SYNC_PANEL_SETTING_ID}
        title={t("lanSync.devices.title")}
      >
        <p className="m-0 px-4 py-3 text-ant-secondary text-sm">
          {t("lanSync.off")}
        </p>
      </SubSection>
    );
  }

  if (!state?.running) {
    return (
      <SubSection
        settingId={LAN_SYNC_PANEL_SETTING_ID}
        title={t("lanSync.devices.title")}
      >
        {state?.error ? (
          <Alert
            className="m-3"
            description={state.error}
            showIcon
            title={t("lanSync.startFailed")}
            type="warning"
          />
        ) : (
          <div className="flex items-center gap-2 px-4 py-3 text-ant-secondary text-sm">
            <Spin size="small" />
            {t("lanSync.starting")}
          </div>
        )}
      </SubSection>
    );
  }

  const addresses =
    state.addresses.length > 0
      ? state.addresses
          .map((address) => {
            return `${address}:${state.port}`;
          })
          .join(" · ")
      : t("lanSync.thisDevice.noAddress");
  const codeHint = state.pairingCode
    ? t("lanSync.pairingCode.hint", { count: state.pairingAttemptsLeft })
    : t("lanSync.pairingCode.exhausted");

  return (
    <div
      className="flex flex-col gap-8"
      data-preference-setting-id={LAN_SYNC_PANEL_SETTING_ID}
    >
      <SubSection title={t("lanSync.thisDevice.title")}>
        <PanelRow>
          <PlatformIcon platform={state.platform} />
          <RowText secondary={addresses} title={state.deviceName} />
        </PanelRow>

        <PanelRow>
          <RowText
            secondary={codeHint}
            title={t("lanSync.pairingCode.title")}
          />
          {state.pairingCode ? (
            <span className="select-text font-mono text-ant-text text-lg tracking-widest">
              {codeHidden ? HIDDEN_CODE : state.pairingCode}
            </span>
          ) : null}
          <div className="flex shrink-0 items-center">
            <Tooltip
              title={
                codeHidden
                  ? t("lanSync.pairingCode.show")
                  : t("lanSync.pairingCode.hide")
              }
            >
              <CustomIconButton
                aria-label={
                  codeHidden
                    ? t("lanSync.pairingCode.show")
                    : t("lanSync.pairingCode.hide")
                }
                disabled={!state.pairingCode}
                icon={
                  <i
                    aria-hidden="true"
                    className={cn({
                      "i-lucide:eye": codeHidden,
                      "i-lucide:eye-off": !codeHidden,
                    })}
                  />
                }
                onClick={toggleCodeHidden}
                type="text"
              />
            </Tooltip>
            <Tooltip title={t("lanSync.pairingCode.refresh")}>
              <CustomIconButton
                aria-label={t("lanSync.pairingCode.refresh")}
                icon={<i aria-hidden="true" className="i-lucide:refresh-cw" />}
                loading={refreshingCode}
                onClick={refreshCode}
                type="text"
              />
            </Tooltip>
          </div>
        </PanelRow>
      </SubSection>

      <SubSection title={t("lanSync.devices.title")}>
        {state.devices.length > 0 ? (
          state.devices.map((device) => {
            const handleRemove = () => {
              confirmRemove(device);
            };

            return (
              <PanelRow key={device.id}>
                <PlatformIcon platform={device.platform} />
                <RowText
                  secondary={
                    device.online
                      ? [t("lanSync.devices.online"), device.address]
                          .filter(Boolean)
                          .join(" · ")
                      : formatLastSeen(device.lastSeenAt)
                  }
                  status={device.online ? "online" : "offline"}
                  title={device.name}
                />
                <Button onClick={handleRemove} size="small">
                  {t("lanSync.devices.remove")}
                </Button>
              </PanelRow>
            );
          })
        ) : (
          <EmptyRow>{t("lanSync.devices.empty")}</EmptyRow>
        )}
      </SubSection>

      <SubSection
        extra={
          <Button onClick={openManualPair} size="small" type="link">
            {t("lanSync.nearby.manual")}
          </Button>
        }
        footer={isWin ? t("lanSync.firewallHint") : void 0}
        title={t("lanSync.nearby.title")}
      >
        {state.nearby.length > 0 ? (
          state.nearby.map((device) => {
            const handlePair = () => {
              openDevicePair(device);
            };

            return (
              <PanelRow key={device.id}>
                <PlatformIcon platform={device.platform} />
                <RowText secondary={device.address} title={device.name} />
                <Button onClick={handlePair} size="small" type="primary">
                  {t("lanSync.nearby.pair")}
                </Button>
              </PanelRow>
            );
          })
        ) : (
          <EmptyRow>
            <Spin size="small" />
            {t("lanSync.nearby.empty")}
          </EmptyRow>
        )}
      </SubSection>

      <LanPairModal onClose={closePairModal} target={pairTarget} />
    </div>
  );
};

interface SubSectionProps {
  children: ReactNode;
  extra?: ReactNode;
  footer?: string;
  /** 设置搜索跳转定位用；整块设备区只挂一处。 */
  settingId?: string;
  title: string;
}

/**
 * 与偏好页分组同款的小标题 + 面板外框；设备区自己分成几组，标题样式保持一致。
 */
const SubSection: FC<SubSectionProps> = (props) => {
  const { children, extra, footer, settingId, title } = props;

  return (
    <section data-preference-setting-id={settingId}>
      <div className="mb-2 flex min-h-6 items-center justify-between gap-2 px-1">
        <h2 className="m-0 font-semibold text-ant-text text-sm leading-snug">
          {title}
        </h2>
        {extra}
      </div>
      <div className="kp-preference-panel overflow-hidden rounded-2 border border-ant-border-secondary">
        {children}
      </div>
      {footer ? (
        <p className="m-0 mt-2 px-1 text-ant-secondary text-xs leading-snug">
          {footer}
        </p>
      ) : null}
    </section>
  );
};

interface PanelRowProps {
  children: ReactNode;
}

const PanelRow: FC<PanelRowProps> = (props) => {
  const { children } = props;

  return (
    <div className="flex min-h-13 items-center gap-3 border-ant-split border-b px-4 py-2.5 last:border-b-0">
      {children}
    </div>
  );
};

interface EmptyRowProps {
  children: ReactNode;
}

const EmptyRow: FC<EmptyRowProps> = (props) => {
  const { children } = props;

  return (
    <div className="flex min-h-13 items-center gap-2 px-4 py-2.5 text-ant-secondary text-sm">
      {children}
    </div>
  );
};

interface RowTextProps {
  secondary?: ReactNode;
  status?: "online" | "offline";
  title: ReactNode;
}

/**
 * 行内左侧的标题 + 次级信息；设备行在标题前加在线状态圆点。
 */
const RowText: FC<RowTextProps> = (props) => {
  const { secondary, status, title } = props;

  return (
    <div className="min-w-0 flex-1">
      <div className="flex min-w-0 items-center gap-2">
        {status ? (
          <span
            aria-hidden="true"
            className={cn("size-2 shrink-0 rounded-full", {
              "bg-ant-quaternary": status === "offline",
              "bg-ant-success": status === "online",
            })}
          />
        ) : null}
        <span className="truncate text-sm leading-snug">{title}</span>
      </div>
      {secondary ? (
        <div className="mt-0.5 truncate text-ant-secondary text-xs leading-snug">
          {secondary}
        </div>
      ) : null}
    </div>
  );
};

interface PlatformIconProps {
  platform: ClipboardPlatform;
}

const PlatformIcon: FC<PlatformIconProps> = (props) => {
  const { platform } = props;

  return (
    <i
      aria-hidden="true"
      className={cn("shrink-0 text-ant-secondary text-lg", {
        "i-lucide:laptop": platform === "macos",
        "i-lucide:monitor": platform === "windows",
      })}
    />
  );
};

export default LanSyncPanel;
