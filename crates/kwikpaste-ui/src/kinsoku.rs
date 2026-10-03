//! 中文避头：全角的句末标点（？！。，等）不放在行首。
//!
//! GPUI 的折行在每个 CJK 字符和全角标点前都允许断开，`……记录吗？` 刚好排满一行时 `？` 会单独
//! 落到下一行；浏览器按 UAX #14 不在这些标点前断行，1.x 的确认框里是 `吗？` 一起换行。这里先按
//! 实际宽度预排一遍，断点落在这类标点前时往前挪一个字，再用显式换行交给 GPUI。

use gpui::{App, FontWeight, LineFragment, Pixels, SharedString, font, px};

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
/// 字一起移到下一行。已有的换行保留。
fn wrap_with(text: &str, mut first_break: impl FnMut(&str) -> Option<usize>) -> String {
    let mut out = String::with_capacity(text.len() + 8);

    for (index, paragraph) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
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
            let (line, tail) = rest.split_at(at);
            out.push_str(line);
            out.push('\n');
            rest = tail;
        }
        out.push_str(rest);
    }

    out
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

    let mut ui_font = font(fonts::UI_FAMILY);
    ui_font.fallbacks = Some(fonts::ui_fallbacks());
    ui_font.weight = FontWeight::NORMAL;
    let mut wrapper = cx.text_system().line_wrapper(ui_font, font_size);
    // 留 1 px 余量：预排的行比 GPUI 实排时略宽的话，GPUI 会在行尾再折一次。
    let width = width - px(1.);

    wrap_with(text, |rest| {
        wrapper
            .wrap_line(&[LineFragment::text(rest)], width)
            .next()
            .map(|boundary| boundary.ix)
    })
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 每 `n` 个字符折一次。
    fn every(n: usize) -> impl FnMut(&str) -> Option<usize> {
        move |text: &str| text.char_indices().nth(n).map(|(ix, _)| ix)
    }

    #[test]
    fn a_closing_mark_takes_the_previous_character_along() {
        assert_eq!(wrap_with("确定删除吗？", every(5)), "确定删除\n吗？");
        assert_eq!(wrap_with("一二三，四五六", every(3)), "一二\n三，四\n五六");
    }

    #[test]
    fn ordinary_breaks_stay_where_they_are() {
        assert_eq!(wrap_with("一二三四五六", every(3)), "一二三\n四五六");
        assert_eq!(wrap_with("短句", every(5)), "短句");
    }

    #[test]
    fn explicit_newlines_are_kept() {
        assert_eq!(wrap_with("一二\n三四五？", every(3)), "一二\n三四\n五？");
    }

    #[test]
    fn a_line_of_one_character_keeps_its_break() {
        // 行里只有一个字时没法再往前挪。
        assert_eq!(wrap_with("一？", every(1)), "一\n？");
    }
}
