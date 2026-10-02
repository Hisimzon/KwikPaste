//! 应用身份：单实例的名字、数据目录都由 identifier 和运行环境派生。
//!
//! 开发、测试构建绝不能用正式版的身份：本机装着、正在运行的 1.x 会把它当成第二实例
//! （或反过来把参数转交给它），数据也会写进 1.x 的 `prod` 目录。正式版只在发布流水线里打开
//! `production-identity` 特性。

use std::sync::OnceLock;

use kwikpaste_core::AppEnv;

const DEVELOPMENT: &str = "com.fastthree.kwikpaste.native-dev";

/// 当前进程的身份。
#[derive(Debug, Clone, Copy)]
pub struct Identity {
    pub identifier: &'static str,
    pub env: AppEnv,
}

/// 正式版：`com.fastthree.kwikpaste` 加 `prod`，与 1.x 互认、共用数据。开发版：独立的
/// identifier 加 `dev`。自测进程另加 `.selftest` 后缀，不会和手动开着的开发实例互认，也不共用
/// 数据；平台探针用 `.selftest-platform`，和同时跑的其它自测进程互不转交。
pub fn current() -> Identity {
    static IDENTITY: OnceLock<Identity> = OnceLock::new();

    *IDENTITY.get_or_init(|| {
        let (base, env) = if cfg!(feature = "production-identity") {
            (kwikpaste_core::APP_IDENTIFIER, AppEnv::Prod)
        } else {
            (DEVELOPMENT, AppEnv::Dev)
        };
        let identifier = if crate::selftest::platform_probe() {
            Box::leak(format!("{base}.selftest-platform").into_boxed_str())
        } else if crate::selftest::active() {
            Box::leak(format!("{base}.selftest").into_boxed_str())
        } else {
            base
        };

        Identity { identifier, env }
    })
}

/// 当前进程的 identifier。
pub fn identifier() -> &'static str {
    current().identifier
}

/// 托盘提示等处显示的名字；开发构建标出来，免得和本机的 1.x 混淆。
pub fn display_name() -> &'static str {
    if cfg!(feature = "production-identity") {
        kwikpaste_core::APP_NAME
    } else {
        "KwikPaste (native dev)"
    }
}
