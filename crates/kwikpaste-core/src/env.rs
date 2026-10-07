//! 宿主传入的运行环境与应用身份。

/// 应用标识，决定数据目录 `%LOCALAPPDATA%\<id>` / `~/Library/Application Support/<id>`。
/// 与 1.x 的 `tauri.conf.json` 相同，2.0 靠它找到 1.x 的数据，不能改。
pub const APP_IDENTIFIER: &str = "com.fastthree.kwikpaste";

/// 产品名，与 1.x 的 `productName` 相同。
pub const APP_NAME: &str = "KwikPaste";

/// 数据隔离用的运行环境，取代 1.x 里 Tauri 注入的 `cfg!(dev)`：dev 数据落 `dev/`，正式版落 `prod/`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AppEnv {
    Dev,
    Prod,
}

impl AppEnv {
    /// 数据根下的环境子目录名，也写进 `storage.json` 与数据目录 identity 的 `environment` 字段。
    pub const fn dir_name(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Prod => "prod",
        }
    }
}

/// 宿主的身份信息。core 的版本号不代表应用版本，由宿主填写。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppInfo {
    pub name: &'static str,
    pub identifier: &'static str,
    pub version: semver::Version,
    pub env: AppEnv,
}

/// 宿主给 core 的运行参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreOptions {
    /// SQLite 连接池上限。1.x 用 sqlx 默认的 10；原生版只有一个进程内的界面在读写，3 个足够。
    pub db_max_connections: u32,
    /// 系统语言标签（BCP 47，如 `zh-CN`），只在首次启动和恢复默认设置时决定界面语言。
    pub locale: Option<String>,
    /// 自测用：来源应用列表换成固定清单，截图不受本机正在运行的进程影响。
    pub fixture_apps: bool,
}

impl Default for CoreOptions {
    fn default() -> Self {
        Self {
            db_max_connections: 3,
            locale: None,
            fixture_apps: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_dir_names_match_released_layout() {
        assert_eq!(AppEnv::Dev.dir_name(), "dev");
        assert_eq!(AppEnv::Prod.dir_name(), "prod");
    }
}
