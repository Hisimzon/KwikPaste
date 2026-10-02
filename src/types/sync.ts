import type { ClipboardPlatform } from "./clipboard";

/** 已配对设备，镜像 Rust `sync::service::LanDeviceView`。 */
export interface LanDevice {
  id: string;
  name: string;
  platform: ClipboardPlatform;
  online: boolean;
  /** 最近一次连上时对方的 `ip:端口`。 */
  address: string | null;
  lastSeenAt: string | null;
  pairedAt: string;
}

/** 同一网络里发现的、尚未配对的设备，镜像 Rust `LanNearbyView`。 */
export interface LanNearbyDevice {
  id: string;
  name: string;
  platform: ClipboardPlatform;
  address: string;
}

/** 局域网同步状态快照，镜像 Rust `sync::service::LanSyncState`；变化时推送 `sync://lan-state`。 */
export interface LanSyncState {
  running: boolean;
  /** 启动失败的技术原因；正常运行或未开启时为 null。 */
  error: string | null;
  deviceId: string | null;
  deviceName: string;
  defaultDeviceName: string;
  platform: ClipboardPlatform;
  port: number | null;
  /** 本机可被连接的 IPv4 地址（不含端口）。 */
  addresses: string[];
  /** 当前配对码；尝试次数用完后为 null，需要刷新。 */
  pairingCode: string | null;
  pairingAttemptsLeft: number;
  devices: LanDevice[];
  nearby: LanNearbyDevice[];
}

/** `sync://lan-paired`：别的设备用本机配对码配对成功。 */
export interface LanPairedPayload {
  name: string;
}

/** 配对目标：附近设备（按 id）或手动输入的地址，二选一。 */
export type LanPairTarget = { deviceId: string } | { address: string };
