//! 剪贴板管线：读取、归类、内容识别、图片与图标落盘、写回、回环抑制与历史自动清理。
//!
//! 监听线程、前台应用识别与来源应用登记随后续批次加入；入库请经 `Core::store_item`。

mod app_store;
mod backend;
pub(crate) mod cleanup;
mod detect;
mod file_icon_store;
mod fragment;
mod guard;
mod icon;
mod ingest;
mod payload;
pub(crate) mod persist;
mod read;
mod secrets;
mod storage;
mod write;

pub use app_store::AppIconStore;
pub use backend::{
    ClipboardBackend, ClipboardFormat, ClipboardWrite, DecodedImage, MemoryClipboard, MemoryState,
    SystemClipboard,
};
pub use cleanup::{CleanupPreview, CleanupReport, CleanupStatus, RulePreview, StorageCheck};
pub use detect::{detect_text_sub_kind, sanitize_css_color};
pub use file_icon_store::FileIconStore;
pub use fragment::{
    fragment_source, quick_snippets, resolve_fragment, split_words, word_spans, ClipboardFragment,
    WordSpan, WordSplit, WordToken, MAX_SPLIT_CHARS,
};
pub use guard::WritebackGuard;
pub use icon::{get_icon_cache_key, icon_png, DIR_CACHE_KEY};
#[cfg(test)]
pub use ingest::build_item;
pub use ingest::{build_item_with_settings, SUMMARY_MAX_CHARS};
pub use payload::{ClipboardPayload, ImagePayload, TextPayload};
pub use read::{png_dimensions, ClipboardReader};
pub use secrets::contains_secret;
pub use storage::{validate_image_file_name, ImageStore, StoredImage};
pub use write::{write_text_fragment, write_to_clipboard};
