//! KwikPaste 的数据契约与业务逻辑，不依赖 Tauri 与 GPUI。
//!
//! 代码从 1.4.0 的 `src-tauri/src` 复制而来，1.x 继续用自己那份，两边不共享。2.0 直接打开 1.x
//! 留下的数据目录、数据库、设置和图片，所以这里的路径布局、迁移文件和序列化格式都是已发布的契约。
//!
//! 运行环境（dev / prod、数据根、系统语言）一律由宿主传入，core 里不出现 `cfg!(dev)`。

pub mod clipboard;
pub mod db;
pub mod disk;
pub mod env;
pub mod error;
pub mod i18n;
pub mod paths;
pub mod portable;
pub mod settings;

pub use env::{AppEnv, AppInfo, APP_IDENTIFIER, APP_NAME};
pub use error::{AppError, Result};
pub use paths::{CorePaths, StorageLocation};
