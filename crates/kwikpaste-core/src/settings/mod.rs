//! 应用设置：单一真相源放在 `SettingsStore`，宿主经它读取与修改。
//!
//! `settings.json` 是 1.x 已发布的格式，2.0 原样读写：字段名、取值和默认值都与 1.4.0 一致。

mod delta;
mod lenient;
mod model;
mod store;

pub use delta::SettingsDelta;
pub use lenient::SettingsLoadReport;
pub use model::*;
pub use store::SettingsStore;
