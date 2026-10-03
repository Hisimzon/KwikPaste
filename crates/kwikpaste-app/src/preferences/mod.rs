#![allow(dead_code)]

pub(crate) mod view;

pub mod icons;
pub mod schema;
pub mod text;
pub mod values;

use anyhow::Result;
use gpui::App;

use crate::platform::host::HostRequest;

/// 处理平台层转交的偏好设置与备份请求。
pub fn open_request(cx: &mut App, request: HostRequest) -> Result<()> {
    match request {
        HostRequest::OpenPreferences { .. } => view::open(cx),
        HostRequest::ImportBackup { path, source } => {
            let mode = kwikpaste_core::backup::inspect_backup_file(&path).map_err(|error| {
                anyhow::anyhow!("invalid backup file {}: {error}", path.display())
            })?;
            log::info!(
                "backup import requested from {:?}: {} ({mode:?}); awaiting import confirmation",
                source,
                path.display()
            );
            view::open(cx)
        }
    }
}

pub fn open(cx: &mut App) -> Result<()> {
    view::open(cx)
}
