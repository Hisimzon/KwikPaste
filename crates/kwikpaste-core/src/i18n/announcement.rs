//! 应用内公告对话框的按钮文案；标题、正文和主按钮文字由公告本身提供。

use crate::settings::Language;

pub struct AnnouncementWords {
    /// 只提醒一次、带链接的公告：不打开链接直接关掉。
    pub close: &'static str,
    /// 只提醒一次、没有链接的公告的唯一按钮。
    pub got_it: &'static str,
    /// 每天提醒的公告：以后都不再弹出。
    pub dismiss: &'static str,
    /// 每天提醒的公告：今天先关掉，明天还会提醒（按 Esc 或点关闭也是这个）。
    pub later: &'static str,
}

pub fn words(lang: Language) -> AnnouncementWords {
    match lang {
        Language::ZhCN => AnnouncementWords {
            close: "关闭",
            got_it: "知道了",
            dismiss: "不再提醒",
            later: "稍后",
        },
        Language::EnUS => AnnouncementWords {
            close: "Close",
            got_it: "Got it",
            dismiss: "Don't remind me",
            later: "Later",
        },
    }
}
