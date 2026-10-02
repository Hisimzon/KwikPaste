//! 列表的数据接口（适配层）。视图只认 [`ClipboardSource`]；数据来自哪里由这里的实现决定：
//!
//! - [`core_source::CoreSource`]：真正的 core（`Core::list_items` / `Core::ensure_thumbnail`），把展示层的
//!   `ClipboardItemView` 转成列表的 `ListItem`；
//! - [`FixtureSource`]：合成夹具（JSON 或生成器），不碰任何真实数据库，供自测、跑分和 core
//!   接进入口之前的默认启动使用。

pub mod core_source;
mod fixture;
pub mod synthetic;

use std::{path::PathBuf, sync::Arc};

use futures::future::BoxFuture;

pub use self::core_source::start_selftest_core;
pub use self::fixture::{FixtureSource, FixtureStore};
use super::model::list_model::Page;

/// 一次列表查询。U1 只分页；分组栏、搜索、排序（U2）在这里加过滤字段，[`core_source::CoreSource`] 负责映射。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListQuery {
    pub offset: usize,
    pub limit: usize,
}

/// 列表的数据来源。实现必须能在任意线程上被调用，返回的 future 在 UI 线程上 await。
pub trait ClipboardSource: Send + Sync + 'static {
    /// 取一页列表载荷（core 已经裁剪、脱敏，并算好文件条目、可用动作等）。
    fn list(&self, query: ListQuery) -> BoxFuture<'static, anyhow::Result<Page>>;

    /// 确保图片记录的缩略图存在并返回路径（1.x `get_clipboard_image_path(fileName, thumbnail: true)`）。
    /// 列表载荷只带已生成的缩略图，没有时卡片先画骨架再调这里。
    fn thumbnail(&self, file_name: Arc<str>) -> BoxFuture<'static, anyhow::Result<PathBuf>>;
}
