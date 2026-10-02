//! 一次设置变更改到了哪些部分。

use std::sync::Arc;

use serde_json::Value;

/// 一次设置变更的范围：局部更新记下 patch，恢复默认、导入备份这类整份替换算全部改动。
///
/// 与 1.x 一样按 patch 里出现的键判断，而不是比较新旧取值：用户显式保存了某一节，
/// 即使取值没变也算改动（例如保存一次历史设置就恢复被暂停的自动清理）。
#[derive(Debug, Clone)]
pub struct SettingsDelta {
    patch: Option<Arc<Value>>,
}

impl SettingsDelta {
    pub fn from_patch(patch: &Value) -> Self {
        Self {
            patch: Some(Arc::new(patch.clone())),
        }
    }

    pub fn replaced() -> Self {
        Self { patch: None }
    }

    pub fn is_replaced(&self) -> bool {
        self.patch.is_none()
    }

    /// 这次变更是否改到了 `path`（点分隔的 camelCase 键，如 `clipboard.history`、`general.trayIcon`）。
    /// patch 改了 `path` 本身、它下面的任意字段，或者把它的某一级父节点整个换掉，都算改到。
    pub fn touches(&self, path: &str) -> bool {
        let Some(patch) = &self.patch else {
            return true;
        };

        let mut node = patch.as_ref();
        for key in path.split('.') {
            let Value::Object(fields) = node else {
                return true;
            };
            let Some(next) = fields.get(key) else {
                return false;
            };
            node = next;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn touches_follows_patch_keys() {
        let delta = SettingsDelta::from_patch(&json!({
            "clipboard": {"history": {"maxCount": 500}},
            "general": {"trayIcon": false}
        }));

        assert!(delta.touches("clipboard"));
        assert!(delta.touches("clipboard.history"));
        assert!(delta.touches("clipboard.history.maxCount"));
        assert!(!delta.touches("clipboard.history.rules"));
        assert!(!delta.touches("clipboard.capture"));
        assert!(delta.touches("general.trayIcon"));
        assert!(!delta.touches("general.autoStart"));
        assert!(!delta.touches("shortcuts"));
        assert!(!delta.is_replaced());
    }

    #[test]
    fn replacing_a_parent_or_everything_touches_children() {
        let delta = SettingsDelta::from_patch(&json!({"clipboard": {"history": null}}));
        assert!(delta.touches("clipboard.history.maxCount"));

        let replaced = SettingsDelta::replaced();
        assert!(replaced.is_replaced());
        assert!(replaced.touches("shortcuts.openClipboard"));
    }
}
