use tauri::{AppHandle, State};

use crate::core::{AppError, Result};
use crate::sync::{LanSyncService, LanSyncState, PairTarget};

#[tauri::command]
pub async fn get_lan_sync_state(
    app: AppHandle,
    service: State<'_, LanSyncService>,
) -> Result<LanSyncState> {
    Ok(service.snapshot(&app))
}

/// 换一个新的配对码（剩余尝试次数同时重置）。
#[tauri::command]
pub async fn refresh_lan_pairing_code(
    app: AppHandle,
    service: State<'_, LanSyncService>,
) -> Result<LanSyncState> {
    service.refresh_pairing_code(&app)?;
    crate::sync::emit_state(&app);
    Ok(service.snapshot(&app))
}

/// 用对方设备上显示的配对码配对；`device_id`（附近设备）与 `address`（手动输入）二选一。
/// 成功返回对方设备名。
#[tauri::command]
pub async fn pair_lan_device(
    app: AppHandle,
    service: State<'_, LanSyncService>,
    device_id: Option<String>,
    address: Option<String>,
    code: String,
) -> Result<String> {
    let target = match (device_id, address) {
        (Some(device_id), None) => PairTarget::Device(device_id),
        (None, Some(address)) => PairTarget::Address(address),
        _ => {
            return Err(AppError::Other(anyhow::anyhow!(
                "exactly one of deviceId and address is required"
            )))
        }
    };

    service.pair(&app, target, &code).await
}

/// 取消与一台设备的配对；对方在线时一并通知它。
#[tauri::command]
pub async fn remove_lan_device(
    app: AppHandle,
    service: State<'_, LanSyncService>,
    device_id: String,
) -> Result<()> {
    service.remove_device(&device_id).await;
    crate::sync::emit_state(&app);
    Ok(())
}
