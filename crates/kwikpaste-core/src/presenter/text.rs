//! 文本展示：敏感内容遮罩、预览文本与预览行数。

use std::ops::Range;

use crate::clipboard::fragment_source;
use crate::db::models::{ClipboardItem, ClipboardKind, ClipboardSubKind};

/// 与 1.x 前端 `PREVIEW_TEXT_SOFT_WRAP_CHARS` 保持一致：超长单行按这个字符数软切。
const PREVIEW_TEXT_SOFT_WRAP_CHARS: usize = 32;

/// 行数统计的上限。面板最高 480、减去 header 与上下内边距后约能放 18 行，
/// 超过上限的行数对尺寸没有任何影响，数到这里就可以收手。
pub(crate) const PREVIEW_TEXT_ROW_COUNT_CAP: u32 = 64;

/// 生成敏感文本遮罩：每行保留头尾少量字符，中间以星号替换。
pub fn mask_sensitive_text(text: &str) -> String {
    text.split('\n')
        .map(mask_sensitive_line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// 只遮内置密钥与自定义敏感规则的命中区间，外加凭据请求头的值，其余文字保留以便辨认；
/// 一处都定位不到（标记它的自定义规则已删除）时退回整段遮罩。
pub(crate) fn mask_with_rules(
    text: &str,
    rules: &crate::clipboard::rules::CompiledRules,
) -> String {
    use crate::clipboard::{credential_header_spans, rules::RuleAction, secret_spans};

    let mut spans: Vec<_> = secret_spans(text)
        .chain(
            rules
                .spans(RuleAction::Sensitive, text)
                .map(|(range, _)| range),
        )
        .collect();
    if spans.is_empty() {
        return mask_sensitive_text(text);
    }
    spans.extend(credential_header_spans(text));

    mask_spans(text, spans)
}

/// 合并重叠区间后逐段遮罩：短区间全遮，长区间保留前 3 后 4 个字符。
fn mask_spans(text: &str, mut spans: Vec<Range<usize>>) -> String {
    spans.sort_by_key(|span| span.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for span in spans {
        match merged.last_mut() {
            Some(last) if span.start < last.end => last.end = last.end.max(span.end),
            _ => merged.push(span),
        }
    }

    let mut result = String::new();
    let mut cursor = 0;
    for span in merged {
        result.push_str(&text[cursor..span.start]);
        let chars: Vec<_> = text[span.clone()].chars().collect();
        if chars.len() <= 8 {
            result.push_str(&"*".repeat(chars.len()));
        } else {
            result.extend(&chars[..3]);
            result.push_str(&"*".repeat((chars.len() - 7).min(8)));
            result.extend(&chars[chars.len() - 4..]);
        }
        cursor = span.end;
    }
    result.push_str(&text[cursor..]);
    result
}

/// 遮罩单行敏感文本；短文本全量替换，长文本保留前后 4 个字符。
pub fn mask_sensitive_line(line: &str) -> String {
    const VISIBLE_EDGE_CHARS: usize = 4;
    const MAX_MASK_CHARS: usize = 8;

    let chars: Vec<char> = line.chars().collect();
    let len = chars.len();
    if len == 0 {
        return String::new();
    }
    if len <= VISIBLE_EDGE_CHARS * 2 {
        return "*".repeat(len);
    }

    let head = chars[..VISIBLE_EDGE_CHARS].iter().collect::<String>();
    let tail = chars[len - VISIBLE_EDGE_CHARS..].iter().collect::<String>();
    let mask_len = (len - VISIBLE_EDGE_CHARS * 2).min(MAX_MASK_CHARS);

    [head, "*".repeat(mask_len), tail].concat()
}

/// 返回预览 payload 的文本子类型；脱敏展示时敏感内容强制按纯文本展示。
pub(crate) fn preview_sub_kind(
    item: &ClipboardItem,
    redact_sensitive: bool,
) -> Option<ClipboardSubKind> {
    if redact_sensitive && item.is_sensitive && item.kind == ClipboardKind::Text {
        return None;
    }

    item.sub_kind
}

/// 返回预览窗口展示用文本；HTML / RTF 使用 OS 提供的纯文本表示，避免渲染富文本源。
pub(crate) fn preview_text(item: &ClipboardItem, redact_sensitive: bool) -> String {
    let source = preview_text_source(item);

    if redact_sensitive && item.is_sensitive {
        return mask_sensitive_text(source);
    }

    source.to_owned()
}

/// 预览文本的原始来源，不做脱敏也不复制；只量尺寸时用它避免整段文本再克隆一次。
/// 与拆词取同一份原文，预览面板上的词序号才能直接拿去粘贴。
pub(crate) fn preview_text_source(item: &ClipboardItem) -> &str {
    fragment_source(item)
}

/// 预览面板要给文本留的行数。
///
/// 遮罩条件必须和 [`preview_text`] 一字不差：遮罩会把一整行缩成十几个字符，这里多遮一次，
/// 面板就按一行开窗，把实际渲染出来的几十行内容截掉。
#[cfg(test)]
pub(crate) fn preview_text_rows(item: &ClipboardItem, redact_sensitive: bool) -> u32 {
    count_preview_text_rows(
        preview_text_source(item),
        redact_sensitive && item.is_sensitive,
    )
}

/// 统计软切后的虚拟行数，规则与 1.x 前端 `buildTextPreviewRows` 一致；`masked` 为真时按
/// 遮罩后的文本统计。
///
/// 长度按 UTF-16 码元计，因为前端切行用的是 JS 字符串长度；用 Rust 的字符数会在
/// emoji、部分 CJK 扩展区上与前端分歧，面板高度就会差出几行。放不下的整字符挪到
/// 下一行，不从 emoji 的代理对中间切开，否则两半各自显示成乱码。
///
/// 数到 `PREVIEW_TEXT_ROW_COUNT_CAP` 就停：面板高度早在此之前就撑到上限了，
/// 而剪贴板里的文本可以有几 MB，一路数到底纯属白烧 CPU。遮罩会复制整段文本，
/// 所以放在这里按需做，普通条目一次复制都没有。
pub(crate) fn count_preview_text_rows(source: &str, masked: bool) -> u32 {
    let masked_text;
    let text = if masked {
        masked_text = mask_sensitive_text(source);
        masked_text.as_str()
    } else {
        source
    };

    if text.is_empty() {
        return 0;
    }

    let mut rows: u32 = 0;
    for line in text.split('\n') {
        rows += 1;
        let mut units = 0;

        for c in line.chars() {
            let width = c.len_utf16();
            if units + width > PREVIEW_TEXT_SOFT_WRAP_CHARS {
                rows += 1;
                units = 0;
            }
            units += width;

            if rows >= PREVIEW_TEXT_ROW_COUNT_CAP {
                return PREVIEW_TEXT_ROW_COUNT_CAP;
            }
        }

        if rows >= PREVIEW_TEXT_ROW_COUNT_CAP {
            return PREVIEW_TEXT_ROW_COUNT_CAP;
        }
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presenter::tests::text_item;

    // 行数统计要和前端的软切规则一致，并且在面板撑满后停手。
    #[test]
    fn masking_preserves_context_but_deleted_rules_use_legacy_mask() {
        use crate::clipboard::rules::{tests::rule, CompiledRules, RuleAction};
        let rules = CompiledRules::new(&[rule("[0-9]+", RuleAction::Sensitive)]);
        assert_eq!(
            mask_with_rules("订单123456789012已发出", &rules),
            "订单123*****9012已发出"
        );
        assert_eq!(
            mask_with_rules("key sk-abcdefghijklmnopqrstuvwxyzABCDE1234567890", &rules),
            "key sk-********7890"
        );
        let plain = "订单123456789012已发出";
        assert_eq!(
            mask_with_rules(plain, &CompiledRules::default()),
            mask_sensitive_text(plain)
        );
    }

    #[test]
    fn masking_merges_overlapping_spans_and_counts_chars() {
        use crate::clipboard::rules::{tests::rule, CompiledRules, RuleAction};
        let rules = CompiledRules::new(&[rule(r"[0-9]+|秘密中文内容", RuleAction::Sensitive)]);
        assert_eq!(
            mask_with_rules(
                "电话12345678，号码12345678901234567890。秘密中文内容。",
                &rules
            ),
            "电话********，号码123********7890。******。"
        );
        let mut second = rule("3456789", RuleAction::Sensitive);
        second.id = "second".into();
        let rules = CompiledRules::new(&[rule("123456", RuleAction::Sensitive), second]);
        assert_eq!(mask_with_rules("a123456789b", &rules), "a123**6789b");
    }

    // #51：浏览器复制的 curl 只遮令牌和 cookie，URL 与普通请求头保持可辨认。
    #[test]
    fn masking_a_curl_keeps_url_and_plain_headers() {
        let curl = [
            "curl 'https://api.example.com/v1/orders?page=1' \\\n",
            "  -H 'accept: application/json' \\\n",
            "  -H 'authorization: Bearer ",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.abcdefghijk' \\\n",
            "  -b 'sid=abc123; theme=dark'",
        ]
        .concat();

        assert_eq!(
            mask_with_rules(&curl, &Default::default()),
            [
                "curl 'https://api.example.com/v1/orders?page=1' \\\n",
                "  -H 'accept: application/json' \\\n",
                "  -H 'authorization: Bea********hijk' \\\n",
                "  -b 'sid********dark'",
            ]
            .concat()
        );
    }

    #[test]
    fn preview_text_rows_match_the_frontend_soft_wrap() {
        assert_eq!(count_preview_text_rows("", false), 0);
        // 结尾的换行在前端也会渲染成一个空行。
        assert_eq!(count_preview_text_rows("a\nbb\n", false), 3);
        // 超长单行按 32 个 UTF-16 码元切块。
        assert_eq!(
            count_preview_text_rows(&"x".repeat(70), false),
            3,
            "70 units wrap into three rows"
        );
    }

    // 行尾放不下的 emoji 整个挪到下一行，和前端切行一致。
    #[test]
    fn preview_text_rows_keep_emoji_whole() {
        assert_eq!(
            count_preview_text_rows(&format!("{}🙏", "a".repeat(30)), false),
            1
        );
        assert_eq!(
            count_preview_text_rows(&format!("{}🙏", "a".repeat(31)), false),
            2
        );
        assert_eq!(count_preview_text_rows(&"🙏".repeat(17), false), 2);
    }

    // 几 MB 的文本不需要数到底：面板高度早就到顶了。
    #[test]
    fn preview_text_rows_stop_at_the_panel_cap() {
        let long = "line\n".repeat(5_000);

        assert_eq!(
            count_preview_text_rows(&long, false),
            PREVIEW_TEXT_ROW_COUNT_CAP
        );
    }

    #[test]
    fn mask_sensitive_text_replaces_middle_with_stars() {
        assert_eq!(
            mask_sensitive_text("sk-abcdefghijklmnopqrstuvwxyzABCDE1234567890"),
            "sk-a********7890"
        );
    }

    #[test]
    fn mask_sensitive_text_replaces_short_lines_fully() {
        assert_eq!(mask_sensitive_text("secret"), "******");
    }

    #[test]
    fn mask_sensitive_text_masks_each_line() {
        assert_eq!(
            mask_sensitive_text("abcd1234xyz\nshort"),
            "abcd***4xyz\n*****"
        );
    }

    #[test]
    fn preview_sub_kind_drops_sensitive_text_subtype_when_redacted() {
        let item = text_item(Some(ClipboardSubKind::Html), true);

        assert_eq!(preview_sub_kind(&item, true), None);
    }

    #[test]
    fn preview_sub_kind_keeps_sensitive_text_subtype_when_not_redacted() {
        let item = text_item(Some(ClipboardSubKind::Html), true);

        assert_eq!(preview_sub_kind(&item, false), Some(ClipboardSubKind::Html));
    }

    #[test]
    fn preview_sub_kind_keeps_regular_text_subtype() {
        let item = text_item(Some(ClipboardSubKind::Html), false);

        assert_eq!(preview_sub_kind(&item, true), Some(ClipboardSubKind::Html));
    }

    #[test]
    fn preview_text_keeps_plain_text_content() {
        let mut item = text_item(None, false);
        item.content = "plain text".to_owned();

        assert_eq!(preview_text(&item, false), "plain text");
    }

    #[test]
    fn preview_text_uses_search_text_for_html() {
        let mut item = text_item(Some(ClipboardSubKind::Html), false);
        item.content = "<b>Hello</b> World".to_owned();
        item.search_text = Some("Hello World".to_owned());

        assert_eq!(preview_text(&item, false), "Hello World");
    }

    #[test]
    fn preview_text_uses_search_text_for_rtf() {
        let mut item = text_item(Some(ClipboardSubKind::Rtf), false);
        item.content = r"{\rtf1 Hello World}".to_owned();
        item.search_text = Some("Hello World".to_owned());

        assert_eq!(preview_text(&item, false), "Hello World");
    }

    #[test]
    fn preview_text_masks_sensitive_plain_source() {
        let mut item = text_item(Some(ClipboardSubKind::Html), true);
        item.content = "<b>sk-abcdefghijklmnopqrstuvwxyzABCDE1234567890</b>".to_owned();
        item.search_text = Some("sk-abcdefghijklmnopqrstuvwxyzABCDE1234567890".to_owned());

        assert_eq!(preview_text(&item, true), "sk-a********7890");
    }

    // 行数和渲染文本必须用同一套遮罩条件：脱敏开着但条目不敏感时，面板要按原文开窗。
    #[test]
    fn preview_text_rows_only_mask_sensitive_items() {
        let mut item = text_item(Some(ClipboardSubKind::Html), false);
        item.content = "<b>irrelevant</b>".to_owned();
        item.search_text = Some("x".repeat(70));

        assert_eq!(preview_text_rows(&item, true), 3);
        assert_eq!(preview_text_rows(&item, false), 3);
    }

    #[test]
    fn preview_text_rows_follow_the_masked_text_for_sensitive_items() {
        let mut item = text_item(None, true);
        item.content = "x".repeat(70);

        // 遮罩后只剩 "xxxx********xxxx"，一行就放得下。
        assert_eq!(preview_text_rows(&item, true), 1);
        assert_eq!(preview_text_rows(&item, false), 3);
    }
}
