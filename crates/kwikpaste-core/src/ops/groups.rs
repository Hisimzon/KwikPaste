//! 自定义分组：增删改、排序与显隐、SVG 图标导入。

use std::path::Path;

use chrono::Utc;
use serde::Deserialize;

use crate::db::groups;
use crate::db::models::ClipboardGroup;
use crate::error::{AppError, Result};
use crate::events::CoreEvent;
use crate::root::Core;

/// 分组没有指定图标时用的预设图标。
pub const DEFAULT_CLIPBOARD_GROUP_ICON: &str = "i-lets-icons:folder";
const MAX_GROUP_NAME_CHARS: usize = 32;
const MAX_GROUP_ICON_BYTES: usize = 256 * 1024;

/// 新建或更新自定义分组的输入。`icon` 是预设图标名或自定义 SVG 源码，空串用默认图标。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardGroupInput {
    pub name: String,
    pub icon: String,
    #[serde(default)]
    pub is_hidden: bool,
}

/// 批量保存分组排序和主界面显隐：`order` 是全部分组的新顺序，`visible_ids` 是显示在主界面的分组。
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardGroupLayoutInput {
    pub order: Vec<String>,
    pub visible_ids: Vec<String>,
}

/// 归一化分组名称，保证持久化前去除首尾空白并限制长度。
fn normalize_group_name(name: &str) -> Result<String> {
    let normalized = name.trim();
    if normalized.is_empty() {
        return Err(AppError::Clipboard("分组名称不能为空".to_owned()));
    }

    if normalized.chars().count() > MAX_GROUP_NAME_CHARS {
        return Err(AppError::Clipboard("分组名称不能超过 32 个字符".to_owned()));
    }

    Ok(normalized.to_owned())
}

/// 归一化分组图标；空值回退到默认预设图标，自定义 SVG 做基础校验。
fn normalize_group_icon(icon: &str) -> Result<String> {
    let normalized = icon.trim();
    if normalized.is_empty() {
        return Ok(DEFAULT_CLIPBOARD_GROUP_ICON.to_owned());
    }

    if normalized.len() > MAX_GROUP_ICON_BYTES {
        return Err(AppError::Clipboard("SVG 图标不能超过 256 KB".to_owned()));
    }

    if normalized.starts_with("<svg") {
        normalize_group_svg(normalized)?;
    }

    Ok(normalized.to_owned())
}

/// 校验自定义 SVG 图标的基础形态：以 `<svg` 开头，不含脚本与 foreignObject。
fn normalize_group_svg(icon: &str) -> Result<()> {
    let normalized = icon.trim_start();
    if !normalized.starts_with("<svg") {
        return Err(AppError::Clipboard("请选择有效的 SVG 图标".to_owned()));
    }

    let lower = normalized.to_ascii_lowercase();
    if lower.contains("<script") || lower.contains("<foreignobject") {
        return Err(AppError::Clipboard(
            "SVG 图标不能包含脚本或 foreignObject".to_owned(),
        ));
    }

    Ok(())
}

impl Core {
    /// 全部分组，按 `sort_order` 升序，同序按创建时间。
    pub async fn list_groups(&self) -> Result<Vec<ClipboardGroup>> {
        let core = self.clone();
        self.hop(async move { groups::list_groups(&core.0.db.pool().await).await })
            .await
    }

    /// 新建分组（排在最后），发 [`CoreEvent::GroupsUpdated`]。
    pub async fn create_group(&self, input: ClipboardGroupInput) -> Result<ClipboardGroup> {
        let core = self.clone();
        self.hop(async move {
            let pool = core.0.db.pool().await;
            let name = normalize_group_name(&input.name)?;
            let icon = normalize_group_icon(&input.icon)?;
            let now = Utc::now();
            let group = ClipboardGroup {
                id: uuid::Uuid::new_v4().to_string(),
                name,
                icon,
                is_hidden: input.is_hidden,
                sort_order: groups::next_group_sort_order(&pool).await?,
                created_at: now,
                updated_at: now,
            };

            groups::insert_group(&pool, &group).await?;
            core.0.events.emit(CoreEvent::GroupsUpdated);
            Ok(group)
        })
        .await
    }

    /// 更新分组名称、图标和显隐，发 [`CoreEvent::GroupsUpdated`]。
    pub async fn update_group(&self, id: &str, input: ClipboardGroupInput) -> Result<()> {
        let core = self.clone();
        let id = id.to_owned();
        self.hop(async move {
            let pool = core.0.db.pool().await;
            let name = normalize_group_name(&input.name)?;
            let icon = normalize_group_icon(&input.icon)?;

            groups::update_group(&pool, &id, &name, &icon, input.is_hidden).await?;
            core.0.events.emit(CoreEvent::GroupsUpdated);
            Ok(())
        })
        .await
    }

    /// 批量保存分组排序和主界面显隐，发 [`CoreEvent::GroupsUpdated`]。
    pub async fn update_groups_layout(&self, input: ClipboardGroupLayoutInput) -> Result<()> {
        let core = self.clone();
        self.hop(async move {
            groups::update_group_layout(&core.0.db.pool().await, &input.order, &input.visible_ids)
                .await?;
            core.0.events.emit(CoreEvent::GroupsUpdated);
            Ok(())
        })
        .await
    }

    /// 删除分组（组内记录回到未分组），发 [`CoreEvent::GroupsUpdated`]。
    pub async fn delete_group(&self, id: &str) -> Result<()> {
        let core = self.clone();
        let id = id.to_owned();
        self.hop(async move {
            groups::delete_group(&core.0.db.pool().await, &id).await?;
            core.0.events.emit(CoreEvent::GroupsUpdated);
            Ok(())
        })
        .await
    }

    /// 读取用户选择的 SVG 文件作为分组图标：必须是 `.svg`、不超过 256 KB、UTF-8、通过安全校验。
    pub fn import_group_svg(&self, path: &Path) -> Result<String> {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if extension != "svg" {
            return Err(AppError::Clipboard("请选择 SVG 文件".to_owned()));
        }

        let bytes = std::fs::read(path)
            .map_err(|err| AppError::Clipboard(format!("无法读取 SVG 文件：{err}")))?;
        if bytes.len() > MAX_GROUP_ICON_BYTES {
            return Err(AppError::Clipboard("SVG 文件不能超过 256 KB".to_owned()));
        }

        let icon = String::from_utf8(bytes)
            .map_err(|_| AppError::Clipboard("SVG 文件必须是 UTF-8 文本".to_owned()))?;
        normalize_group_svg(&icon)?;

        Ok(icon)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_names_are_trimmed_and_limited() {
        assert_eq!(normalize_group_name("  工作 ").unwrap(), "工作");
        assert!(normalize_group_name("   ").is_err());
        assert!(normalize_group_name(&"字".repeat(32)).is_ok());
        assert!(normalize_group_name(&"字".repeat(33)).is_err());
    }

    #[test]
    fn group_icons_default_and_reject_unsafe_svg() {
        assert_eq!(
            normalize_group_icon("  ").unwrap(),
            DEFAULT_CLIPBOARD_GROUP_ICON
        );
        assert_eq!(
            normalize_group_icon("i-lets-icons:star").unwrap(),
            "i-lets-icons:star"
        );
        assert!(normalize_group_icon("<svg><path d='M0 0'/></svg>").is_ok());
        assert!(normalize_group_icon("<svg><script>alert(1)</script></svg>").is_err());
        assert!(normalize_group_icon("<svg><foreignObject/></svg>").is_err());
        assert!(normalize_group_icon(&format!("<svg>{}</svg>", "a".repeat(256 * 1024))).is_err());
        assert!(normalize_group_svg("<div/>").is_err());
    }
}
