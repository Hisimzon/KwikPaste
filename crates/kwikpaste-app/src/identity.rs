//! 应用身份。单实例的名字（以后还有数据目录）都由 identifier 派生。
//!
//! 开发、测试构建绝不能用正式版的 identifier：本机装着、正在运行的 1.x 会把它当成第二实例，
//! 或者反过来把参数转交给它。正式版只在发布流水线里打开 `production-identity` 特性。

const PRODUCTION: &str = "com.fastthree.kwikpaste";
const DEVELOPMENT: &str = "com.fastthree.kwikpaste.native-dev";

/// 当前进程的 identifier。自测进程另加 `.selftest` 后缀，不会和手动开着的开发实例互认。
pub fn identifier() -> String {
    let base = if cfg!(feature = "production-identity") {
        PRODUCTION
    } else {
        DEVELOPMENT
    };

    if crate::selftest::active() {
        format!("{base}.selftest")
    } else {
        base.to_owned()
    }
}

/// 托盘提示等处显示的名字；开发构建标出来，免得和本机的 1.x 混淆。
pub fn display_name() -> &'static str {
    if cfg!(feature = "production-identity") {
        "KwikPaste"
    } else {
        "KwikPaste (native dev)"
    }
}
