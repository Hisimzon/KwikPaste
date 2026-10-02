//! 给任意 GPUI 元素用的排版辅助：标准语义字号、等宽字体。

use gpui::Styled;

use crate::theme::{TextSize, fonts};

pub trait KpStyled: Styled + Sized {
    /// 标准语义字号：同时设置字号和行高（1.x 的 `text-xs` / `text-sm` / `text-base` / `text-lg`）。
    fn kp_text(mut self, size: TextSize) -> Self {
        let text = self.text_style();
        text.font_size = Some(size.font_size().into());
        text.line_height = Some(size.line_height().into());
        self
    }

    /// 等宽字体（1.x 的 `font-mono`），中文回退到系统中文字体。
    fn kp_mono(mut self) -> Self {
        let text = self.text_style();
        text.font_family = Some(fonts::MONO_FAMILY.into());
        text.font_fallbacks = Some(fonts::mono_fallbacks());
        self
    }
}

impl<T: Styled> KpStyled for T {}
