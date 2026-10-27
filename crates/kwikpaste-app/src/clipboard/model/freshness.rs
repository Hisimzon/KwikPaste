//! 列表数据追没追上 core：作用于“当前项”的按键（Enter、Mod+Enter、Mod+数字）只在追上之后执行。
//!
//! 每收到一个会影响当前视图的变化记一笔；替换请求发出时记下它覆盖到第几笔，接受响应后才算追上。
//! 替换可以是第一页，也可以是复用记录后的当前范围；按键等目标范围落地，不把滚动中的列表换回第一页。

/// 挂起的“作用于当前项”的按键。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activation {
    /// Enter / Mod+Enter。
    Paste { plain: bool },
    /// Mod+数字。
    QuickPaste { key: char },
}

#[derive(Debug, Default)]
pub struct Freshness {
    /// 收到的变化笔数。
    changes: u64,
    /// 已落地的替换范围覆盖到的笔数。
    applied: u64,
    /// 在路上的替换请求：(请求令牌, 发出时的笔数)。
    in_flight: Option<(u64, u64)>,
}

impl Freshness {
    /// 收到一个影响当前视图的变化。
    pub fn changed(&mut self) {
        self.changes += 1;
    }

    /// 发出了一个整体替换缓存的请求。
    pub fn first_page_sent(&mut self, token: u64) {
        self.in_flight = Some((token, self.changes));
    }

    /// 替换请求结束；失败时沿用旧缓存，不让按键一直挂起。
    pub fn first_page_done(&mut self, token: u64) {
        if let Some((sent, covers)) = self.in_flight
            && sent == token
        {
            self.applied = self.applied.max(covers);
            self.in_flight = None;
        }
    }

    /// 缓存里的替换范围已经反映了收到的全部变化。
    pub fn fresh(&self) -> bool {
        self.applied >= self.changes
    }

    /// 已经有一个覆盖全部变化的替换请求在路上（不用再发）。
    pub fn awaiting(&self) -> bool {
        self.in_flight
            .is_some_and(|(_, covers)| covers >= self.changes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_is_caught_up_only_by_a_first_page_sent_after_it() {
        let mut freshness = Freshness::default();
        assert!(freshness.fresh());

        freshness.first_page_sent(1);
        freshness.changed();
        assert!(!freshness.fresh());
        assert!(
            !freshness.awaiting(),
            "the request in flight predates the change"
        );

        freshness.first_page_done(1);
        assert!(
            !freshness.fresh(),
            "the old request does not cover the change"
        );

        freshness.first_page_sent(2);
        assert!(freshness.awaiting());
        freshness.first_page_done(2);
        assert!(freshness.fresh());
    }

    #[test]
    fn stale_tokens_do_not_count() {
        let mut freshness = Freshness::default();
        freshness.changed();
        freshness.first_page_sent(3);
        freshness.changed();
        freshness.first_page_sent(4);

        freshness.first_page_done(3);
        assert!(!freshness.fresh(), "token 3 was replaced by token 4");
        freshness.first_page_done(4);
        assert!(freshness.fresh());
    }
}
