//! 图标。
//!
//! 暂时借用 gpui-kit-assets 随组件库打包的 lucide 图标（[`crate::Assets`] 提供）；附录 D §1.8 的
//! 图标管线（从 1.x 的 iconify 包导出 lucide / lets-icons / ph 并内嵌）落地后，枚举换成自带 SVG，
//! 调用方不用改。

use gpui::{App, Hsla, IntoElement, Rems, RenderOnce, Styled, Window, prelude::FluentBuilder as _};

/// 应用可用的图标。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconName {
    Check,
    ChevronDown,
    Close,
    Copy,
    Delete,
    Ellipsis,
    ExternalLink,
    Eye,
    Folder,
    Globe,
    Heart,
    Inbox,
    Info,
    Moon,
    Palette,
    Plus,
    Search,
    Settings,
    Star,
    Sun,
    TextSize,
}

impl IconName {
    pub(crate) fn kit(self) -> gpui_component::IconName {
        use gpui_component::IconName as Kit;

        match self {
            Self::Check => Kit::Check,
            Self::ChevronDown => Kit::ChevronDown,
            Self::Close => Kit::Close,
            Self::Copy => Kit::Copy,
            Self::Delete => Kit::Delete,
            Self::Ellipsis => Kit::Ellipsis,
            Self::ExternalLink => Kit::ExternalLink,
            Self::Eye => Kit::Eye,
            Self::Folder => Kit::Folder,
            Self::Globe => Kit::Globe,
            Self::Heart => Kit::Heart,
            Self::Inbox => Kit::Inbox,
            Self::Info => Kit::Info,
            Self::Moon => Kit::Moon,
            Self::Palette => Kit::Palette,
            Self::Plus => Kit::Plus,
            Self::Search => Kit::Search,
            Self::Settings => Kit::Settings,
            Self::Star => Kit::Star,
            Self::Sun => Kit::Sun,
            Self::TextSize => Kit::ALargeSmall,
        }
    }

    pub(crate) fn kit_icon(self) -> gpui_component::Icon {
        gpui_component::Icon::new(self.kit())
    }
}

/// 单色图标，颜色默认继承文字色。
#[derive(IntoElement)]
pub struct Icon {
    name: IconName,
    size: Option<Rems>,
    color: Option<Hsla>,
}

impl Icon {
    pub fn new(name: IconName) -> Self {
        Self {
            name,
            size: None,
            color: None,
        }
    }

    pub fn size(mut self, size: Rems) -> Self {
        self.size = Some(size);
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.name
            .kit_icon()
            .when_some(self.size, |icon, size| icon.size(size))
            .when_some(self.color, |icon, color| icon.text_color(color))
    }
}
