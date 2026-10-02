//! 主窗口列表的模型层：不依赖 GPUI，全部可以 headless 跑单测（附录 D §3.1）。
//!
//! - [`item`]：列表条目的视图模型（core 列表载荷的形状）；
//! - [`list_model`]：稀疏分页缓存；
//! - [`controller`]：当前项、键盘移动、数字提示、刷新策略；
//! - [`layout`]：密度换算、图片显示尺寸预测；
//! - [`time_label`]：卡片时间标签。

pub mod controller;
pub mod item;
pub mod layout;
pub mod list_model;
pub mod time_label;
