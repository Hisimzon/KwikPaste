//! 端到端测试用的环境变量覆盖。
//!
//! 只有编进 `e2e-overrides` 特性时才读环境变量；发布产物不开这个特性，环境变量一律忽略，
//! 从旧进程继承来的也一样。开了特性的构建里留有哨兵字符串 [`SENTINEL`]，发布流水线据此确认没混进来。

/// 稳定版渠道的清单地址（只用这一个，不再读默认镜像）。
pub(crate) const STABLE_ENDPOINT: &str = "KWIKPASTE_UPDATE_ENDPOINT";
/// 测试版渠道的清单地址。
pub(crate) const BETA_ENDPOINT: &str = "KWIKPASTE_UPDATE_BETA_ENDPOINT";
/// 验签公钥（与 `tauri.conf.json` 里 `plugins.updater.pubkey` 同格式），测试用一次性密钥。
pub(crate) const PUBLIC_KEY: &str = "KWIKPASTE_UPDATE_PUBLIC_KEY";
/// 使用统计的上报地址；设置后开发构建也上报。
pub(crate) const USAGE_ENDPOINT: &str = "KWIKPASTE_USAGE_ENDPOINT";
/// 公告接口地址；设置后只请求这个地址（不读快照），开发构建也拉取。
pub(crate) const ANNOUNCEMENT_ENDPOINT: &str = "KWIKPASTE_ANNOUNCEMENT_ENDPOINT";

/// 编进 `e2e-overrides` 的构建里才有的字符串。
#[cfg(feature = "e2e-overrides")]
pub const SENTINEL: &str = "KWIKPASTE_E2E_OVERRIDES_ENABLED";

/// 读一个覆盖项；没开特性、没设置或为空都返回 `None`。
#[cfg(feature = "e2e-overrides")]
pub(crate) fn var(name: &str) -> Option<String> {
    std::hint::black_box(SENTINEL);
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// 读一个覆盖项；没开特性、没设置或为空都返回 `None`。
#[cfg(not(feature = "e2e-overrides"))]
pub(crate) fn var(_name: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `PATH` 总是有值：没开特性时照样读不到，开了特性才读得到。
    #[test]
    fn overrides_follow_the_feature() {
        assert_eq!(var("PATH").is_some(), cfg!(feature = "e2e-overrides"));
        assert_eq!(var("KWIKPASTE_UPDATE_SURELY_UNSET_FOR_TESTS"), None);
    }
}
