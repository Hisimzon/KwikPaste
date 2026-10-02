//! 剪贴板内容的纯逻辑：文本子类型识别、密钥 / token 识别、列表快捷信息与拆词。
//!
//! 剪贴板监听、读取与写回、图片落盘随后续批次加入。

mod detect;
mod fragment;
mod ingest;
mod secrets;

pub use detect::{detect_text_sub_kind, sanitize_css_color};
pub use fragment::{
    fragment_source, quick_snippets, resolve_fragment, split_words, word_spans, ClipboardFragment,
    WordSpan, WordSplit, WordToken, MAX_SPLIT_CHARS,
};
pub use ingest::SUMMARY_MAX_CHARS;
pub use secrets::contains_secret;
