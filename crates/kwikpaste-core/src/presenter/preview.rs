//! 预览面板的展示数据与内容度量。

use std::path::Path;

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::SqlitePool;

use super::files::{
    count_file_paths, is_image_path, path_to_string, resolve_file_icon_path,
    resolve_preview_file_is_dir, resolve_preview_file_size,
};
use super::text::{
    count_preview_text_rows, preview_sub_kind, preview_text, preview_text_rows, preview_text_source,
};
use crate::clipboard::{
    split_words, validate_image_file_name, word_spans, FileIconStore, ImageStore, WordSpan,
};
use crate::db::models::{ClipboardItem, ClipboardKind, ClipboardSubKind};
use crate::error::Result;
use crate::settings::{Clipboard, PreviewTextView};

/// 预览面板最多列出的文件条数，控制图标抽取成本。
pub const PREVIEW_FILE_ENTRY_LIMIT: usize = 64;

// 拆词视图的词块尺寸，与 1.x 前端 `WordChipsViewer` 的样式一一对应：
// text-sm / leading-5 / px-1.5 / py-0.5 / min-w-6。
const PREVIEW_WORD_FONT_SIZE: f64 = 14.0;
const PREVIEW_WORD_NARROW_CHAR_EM: f64 = 0.5;
const PREVIEW_WORD_CHIP_PADDING_X: f64 = 12.0;
const PREVIEW_WORD_CHIP_MIN_WIDTH: f64 = 24.0;

/// 预览面板专用 payload：只暴露内容视图渲染所需的归一化字段。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardPreviewPayload {
    pub id: String,
    pub kind: ClipboardKind,
    pub sub_kind: Option<ClipboardSubKind>,
    pub updated_at: DateTime<Utc>,
    /// 预览展示用纯文本。HTML / RTF 条目返回 `search_text`，不返回富文本源；敏感内容按设置遮罩。
    pub text: Option<String>,
    pub image_path: Option<String>,
    pub image_width: Option<i64>,
    pub image_height: Option<i64>,
    pub size: Option<i64>,
    pub is_sensitive: bool,
    pub image_exists: bool,
    pub files: Vec<ClipboardPreviewFileEntry>,
    pub total_files: usize,
    /// 文本里可点选的词（UTF-16 区间，序号即拆词序号）；脱敏展示的敏感内容为空。
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<WordSpan>,
    /// 原文过长、词区间只覆盖开头：选词视图在词块末尾提示只拆了开头。
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub words_truncated: bool,
}

/// 预览面板里的单个文件条目，比列表卡片保留更多文件并带上 size。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardPreviewFileEntry {
    pub path: String,
    pub name: String,
    pub is_dir: bool,
    pub is_image: bool,
    pub exists: bool,
    pub size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon_path: Option<String>,
}

/// 预览面板尺寸所需的内容度量。面板要在显示前定好尺寸，所以由记录直接算出。
#[derive(Clone, Debug, PartialEq)]
pub enum PreviewContentMetrics {
    /// 预览文本软切后的行数。
    Text { rows: u32 },
    /// 拆词视图的每个词块；面板宽度定下来后再按宽度折行算高度。
    Words { chips: Vec<PreviewWordChip> },
    /// 图片原始尺寸；记录缺尺寸时为 `None`，退回兜底面板大小。
    Image {
        width: Option<f64>,
        height: Option<f64>,
    },
    /// 文件条目：实际渲染条数与总条数（总数更多时底部多一行提示）。
    Files { shown: u32, total: u32 },
}

/// 拆词视图里的一个词块：按字符估出的宽度（px），以及它前面是否换段。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PreviewWordChip {
    pub width: f64,
    pub line_break: bool,
}

impl PreviewWordChip {
    /// 按 1.x 词块样式估宽度：中日韩、全角与 emoji 记 1em，其余记 0.5em
    /// （界面字体实测西文平均约 0.47em），再加左右内边距。
    pub fn new(text: &str, line_break: bool) -> Self {
        let ems: f64 = text
            .chars()
            .map(|c| {
                if is_wide_char(c) {
                    1.0
                } else {
                    PREVIEW_WORD_NARROW_CHAR_EM
                }
            })
            .sum();
        let width = ems * PREVIEW_WORD_FONT_SIZE + PREVIEW_WORD_CHIP_PADDING_X;

        Self {
            width: width.max(PREVIEW_WORD_CHIP_MIN_WIDTH),
            line_break,
        }
    }
}

/// 按一个字宽排版的字符：中日韩文字、全角符号与 emoji。
fn is_wide_char(c: char) -> bool {
    matches!(
        u32::from(c),
        0x1100..=0x115F
            | 0x2E80..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE30..=0xFE4F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1FAFF
            | 0x20000..=0x3FFFD
    )
}

/// 一条记录的预览内容度量。脱敏设置会改变实际渲染的文本，所以行数按脱敏后的文本统计；
/// 文本按原文还是词块排版取决于预览方式设置。
pub(crate) fn preview_content_metrics(
    item: &ClipboardItem,
    clipboard: &Clipboard,
) -> PreviewContentMetrics {
    let redact_sensitive = clipboard.sensitive.redact_secrets;

    match item.kind {
        ClipboardKind::Text => {
            preview_text_metrics(item, redact_sensitive, clipboard.preview.text_view)
        }
        ClipboardKind::Image => PreviewContentMetrics::Image {
            width: item.width.map(|value| value as f64),
            height: item.height.map(|value| value as f64),
        },
        ClipboardKind::Files => {
            let total = count_file_paths(&item.content);

            PreviewContentMetrics::Files {
                shown: total.min(PREVIEW_FILE_ENTRY_LIMIT) as u32,
                total: total as u32,
            }
        }
    }
}

/// 文本预览的面板度量：选词方式下有词可拆时按词块排版，否则（原文方式、脱敏展示、无词可拆）按文本行。
/// 判断条件与 [`build_preview_payload`] 给出词区间的条件一致，界面据此选同一种视图。
///
/// 原文过长时只拆开头，词块按拆出的部分排；拆到上限的文本几乎总能撑满面板最大高度，
/// 所以词块末尾那行「只拆了开头」的提示不计入高度。
fn preview_text_metrics(
    item: &ClipboardItem,
    redact_sensitive: bool,
    text_view: PreviewTextView,
) -> PreviewContentMetrics {
    if redact_sensitive && item.is_sensitive {
        return PreviewContentMetrics::Text {
            rows: preview_text_rows(item, redact_sensitive),
        };
    }

    preview_plain_text_metrics(preview_text_source(item), text_view)
}

/// 普通文本与图片识别文本共用的度量：有词可拆时按词块，否则按软切后的文本行。
fn preview_plain_text_metrics(text: &str, text_view: PreviewTextView) -> PreviewContentMetrics {
    if text_view == PreviewTextView::Words {
        let split = split_words(text);
        if !split.tokens.is_empty() {
            return PreviewContentMetrics::Words {
                chips: split
                    .tokens
                    .iter()
                    .map(|token| PreviewWordChip::new(&token.text, token.line_break))
                    .collect(),
            };
        }
    }

    PreviewContentMetrics::Text {
        rows: count_preview_text_rows(text, false),
    }
}

/// 图片识别文本按普通文本预览展示，保留图片记录的身份和时间，不带图片或文件字段。
pub(crate) fn build_image_text_preview(
    item: &ClipboardItem,
    text: String,
    text_view: PreviewTextView,
) -> (ClipboardPreviewPayload, PreviewContentMetrics) {
    let metrics = preview_plain_text_metrics(&text, text_view);
    let (words, words_truncated) = word_spans(&text);
    let payload = ClipboardPreviewPayload {
        id: item.id.clone(),
        kind: ClipboardKind::Text,
        sub_kind: None,
        updated_at: item.updated_at,
        text: Some(text),
        image_path: None,
        image_width: None,
        image_height: None,
        size: None,
        is_sensitive: false,
        image_exists: false,
        files: Vec::new(),
        total_files: 0,
        words,
        words_truncated,
    };

    (payload, metrics)
}

/// 将完整记录转为预览面板的轻量数据模型。必须在 core runtime 里调用（文件图标可能要抽取）。
pub(crate) async fn build_preview_payload(
    pool: &SqlitePool,
    image_store: &ImageStore,
    file_icon_store: &FileIconStore,
    item: ClipboardItem,
    redact_sensitive: bool,
) -> Result<ClipboardPreviewPayload> {
    let mut text = None;
    let mut image_path = None;
    let mut image_exists = false;
    let mut files = Vec::new();
    let mut total_files = 0;
    let mut words = Vec::new();
    let mut words_truncated = false;

    match item.kind {
        ClipboardKind::Text => {
            text = Some(preview_text(&item, redact_sensitive));
            if !(redact_sensitive && item.is_sensitive) {
                (words, words_truncated) = word_spans(preview_text_source(&item));
            }
        }
        ClipboardKind::Image => {
            validate_image_file_name(&item.content)?;
            let path = image_store.origin_path(&item.content);
            image_exists = path.exists();
            image_path = Some(path_to_string(&path, "image path")?);
        }
        ClipboardKind::Files => {
            total_files = count_file_paths(&item.content);
            files = build_preview_file_entries(pool, file_icon_store, &item).await?;
        }
    }
    let preview_sub_kind = preview_sub_kind(&item, redact_sensitive);

    Ok(ClipboardPreviewPayload {
        id: item.id,
        kind: item.kind,
        sub_kind: preview_sub_kind,
        updated_at: item.updated_at,
        text,
        image_path,
        image_width: item.width,
        image_height: item.height,
        size: item.size,
        is_sensitive: item.is_sensitive,
        image_exists,
        files,
        total_files,
        words,
        words_truncated,
    })
}

/// 解析 files 类型记录中的路径列表，最多返回前 64 项以控制 icon 抽取成本。
async fn build_preview_file_entries(
    pool: &SqlitePool,
    store: &FileIconStore,
    item: &ClipboardItem,
) -> Result<Vec<ClipboardPreviewFileEntry>> {
    let paths: Vec<&str> = item
        .content
        .split('\n')
        .filter(|path| !path.is_empty())
        .take(PREVIEW_FILE_ENTRY_LIMIT)
        .collect();

    let mut entries = Vec::with_capacity(paths.len());
    for (index, path) in paths.iter().enumerate() {
        let (icon_path, exists) =
            resolve_file_icon_path(pool, store, path, item.file_types.as_deref(), index).await?;
        let path_obj = Path::new(path);
        let is_dir = resolve_preview_file_is_dir(path_obj, item.file_types.as_deref(), index);
        let is_image = !is_dir && is_image_path(path);
        let size = resolve_preview_file_size(path_obj, is_dir);
        let name = path_obj
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
            .unwrap_or_else(|| (*path).to_owned());

        entries.push(ClipboardPreviewFileEntry {
            path: (*path).to_owned(),
            name,
            is_dir,
            is_image,
            exists,
            size,
            icon_path,
        });
    }

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presenter::tests::text_item;

    // 面板按预览方式开窗：原文方式按文本行，选词方式按词块；脱敏展示的敏感内容没有词可选，始终按文本行。
    #[test]
    fn preview_text_metrics_follow_the_text_view() {
        let is_words = |item: &ClipboardItem, redact: bool, view: PreviewTextView| {
            matches!(
                preview_text_metrics(item, redact, view),
                PreviewContentMetrics::Words { .. }
            )
        };
        let mut item = text_item(None, false);
        item.content = "9140BT 19mm".to_owned();

        assert!(!is_words(&item, false, PreviewTextView::Plain));
        assert!(is_words(&item, false, PreviewTextView::Words));

        item.is_sensitive = true;
        assert!(!is_words(&item, true, PreviewTextView::Words));
        assert!(is_words(&item, false, PreviewTextView::Words));

        // 只拆了开头的长文本照样按词块排。
        item.is_sensitive = false;
        item.content = "字".repeat(3_000);
        assert!(is_words(&item, false, PreviewTextView::Words));
    }

    #[test]
    fn word_chip_width_counts_wide_characters_as_one_em() {
        assert_eq!(PreviewWordChip::new("a", false).width, 24.0);
        assert_eq!(PreviewWordChip::new("钢化", true).width, 2.0 * 14.0 + 12.0);
        assert_eq!(
            PreviewWordChip::new("W2000", false).width,
            2.5 * 14.0 + 12.0
        );
    }
}
