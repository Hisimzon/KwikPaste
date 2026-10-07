#![allow(dead_code)]

mod onboarding;
mod sortable;
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
            log::info!(
                "backup import requested from {:?}: {}",
                source,
                path.display()
            );
            view::open_import(path, cx)
        }
    }
}

pub fn open(cx: &mut App) -> Result<()> {
    view::open(cx)
}

pub fn open_onboarding(cx: &mut App) -> Result<()> {
    onboarding::open(cx)
}
