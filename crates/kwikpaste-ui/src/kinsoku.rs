//! 中文避头：全角的句末标点（？！。，等）不放在行首。
//!
//! GPUI 的折行在每个 CJK 字符和全角标点前都允许断开，`……记录吗？` 刚好排满一行时 `？` 会单独
//! 落到下一行；浏览器按 UAX #14 不在这些标点前断行，1.x 的确认框里是 `吗？` 一起换行。这里先按
//! 实际宽度预排一遍，断点落在这类标点前时往前挪一个字，再用显式换行交给 GPUI，或直接按行区间逐行画。

use std::ops::Range;

use gpui::{App, FontWeight, LineFragment, LineWrapperHandle, Pixels, SharedString, font, px};

use crate::theme::fonts;

/// 不放在行首的标点（UAX #14 的 CL / CP / EX / IS / NS 中常见的全角字符）。
fn no_break_before(c: char) -> bool {
    matches!(
        c,
        '，' | '。'
            | '、'
            | '；'
            | '：'
            | '？'
            | '！'
            | '）'
            | '」'
            | '』'
            | '】'
            | '》'
            | '〉'
            | '〕'
            | '］'
            | '｝'
            | '”'
            | '’'
            | '…'
            | '％'
            | '·'
    )
}

/// 按 `first_break`（给出一段文字第一个折行点的字节下标）逐行折开，断点落在避头标点前时连同前一个
/// 字一起移到下一行。已有的换行保留（不计入任何一行）。返回每行在 `text` 里的字节区间。
fn wrap_with(text: &str, mut first_break: impl FnMut(&str) -> Option<usize>) -> Vec<Range<usize>> {
    let mut rows = Vec::new();
    let mut paragraph_start = 0;

    for paragraph in text.split('\n') {
        let mut start = paragraph_start;
        let mut rest = paragraph;
        while let Some(mut at) = first_break(rest).filter(|at| *at > 0 && *at < rest.len()) {
            let breaks_before_mark = rest
                .get(at..)
                .and_then(|tail| tail.chars().next())
                .is_some_and(no_break_before);
            if breaks_before_mark
                && let Some((previous, _)) =
                    rest.get(..at).and_then(|head| head.char_indices().last())
                && previous > 0
            {
                at = previous;
            }
            let Some(tail) = rest.get(at..) else {
                break;
            };
            rows.push(start..start + at);
            start += at;
            rest = tail;
        }
        rows.push(start..start + rest.len());
        paragraph_start += paragraph.len() + 1;
    }

    rows
}

/// 正文字体（`font_size`、常规字重）的折行器。
fn body_wrapper(font_size: Pixels, cx: &App) -> LineWrapperHandle {
    let mut ui_font = font(fonts::UI_FAMILY);
    ui_font.fallbacks = Some(fonts::ui_fallbacks());
    ui_font.weight = FontWeight::NORMAL;
    cx.text_system().line_wrapper(ui_font, font_size)
}

/// 按正文字体在 `width` 内把 `text` 折成行（避开行首标点，已有换行保留），返回每行的字节区间。
pub fn wrap_rows(text: &str, width: Pixels, font_size: Pixels, cx: &App) -> Vec<Range<usize>> {
    let mut wrapper = body_wrapper(font_size, cx);
    // 留 1 px 余量：预排的行比 GPUI 实排时略宽的话，行尾会被裁掉或再折一次。
    let width = (width - px(1.)).max(px(1.));

    wrap_with(text, |rest| {
        wrapper
            .wrap_line(&[LineFragment::text(rest)], width)
            .next()
            .map(|boundary| boundary.ix)
    })
}

/// 按正文字体（`font_size`、常规字重）在 `width` 内预排 `text`，返回加好换行、避开行首标点的文字。
/// 文字里没有避头标点时原样返回，不做排版。
pub fn kinsoku_wrap(
    text: &SharedString,
    width: Pixels,
    font_size: Pixels,
    cx: &App,
) -> SharedString {
    if !text.chars().any(no_break_before) || width <= px(0.) {
        return text.clone();
    }

    joined(text, &wrap_rows(text, width, font_size, cx)).into()
}

/// 把行区间用换行接回一段文字。
fn joined(text: &str, rows: &[Range<usize>]) -> String {
    rows.iter()
        .filter_map(|row| text.get(row.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每 `n` 个字符折一次，再接回文字。
    fn every(text: &str, n: usize) -> String {
        let rows = wrap_with(text, |rest: &str| {
            rest.char_indices().nth(n).map(|(ix, _)| ix)
        });
        joined(text, &rows)
    }

    #[test]
    fn a_closing_mark_takes_the_previous_character_along() {
        assert_eq!(every("确定删除吗？", 5), "确定删除\n吗？");
        assert_eq!(every("一二三，四五六", 3), "一二\n三，四\n五六");
    }

    #[test]
    fn ordinary_breaks_stay_where_they_are() {
        assert_eq!(every("一二三四五六", 3), "一二三\n四五六");
        assert_eq!(every("短句", 5), "短句");
    }

    #[test]
    fn explicit_newlines_are_kept() {
        assert_eq!(every("一二\n三四五？", 3), "一二\n三四\n五？");
    }

    #[test]
    fn a_line_of_one_character_keeps_its_break() {
        // 行里只有一个字时没法再往前挪。
        assert_eq!(every("一？", 1), "一\n？");
    }

    #[test]
    fn rows_are_byte_ranges_without_the_newlines() {
        let text = "ab中\n\ncdef";
        let rows = wrap_with(text, |rest: &str| {
            rest.char_indices().nth(2).map(|(ix, _)| ix)
        });
        assert_eq!(rows, vec![0..2, 2..5, 6..6, 7..9, 9..11]);
    }
}
