export const APP_NAME_PLACEHOLDER = "KwikPaste";

/** 与 Rust `MIN_STORAGE_LIMIT_MB` 保持一致。 */
export const STORAGE_LIMIT_MIN_MB = 100;
export const STORAGE_LIMIT_MAX_MB = 1024 * 1024;
/** 1.x 不开放局域网同步（2.0 原生版正式推出），隐藏「同步」页；与 Rust `sync::AVAILABLE` 保持一致。 */
export const LAN_SYNC_AVAILABLE = false;
/** 与 Rust `LAN_SYNC_MAX_IMAGE_MB_MIN` / `LAN_SYNC_MAX_IMAGE_MB_MAX` 保持一致。 */
export const LAN_SYNC_MAX_IMAGE_MB_MIN = 1;
export const LAN_SYNC_MAX_IMAGE_MB_MAX = 100;
/** 占用达到上限的这个比例后侧栏转为警示色。 */
export const STORAGE_WARNING_RATIO = 0.8;
