//! 图标。
//!
//! 组件常用的 lucide 图标借用 gpui-kit-assets 的默认图标集；默认集里没有的，从 1.x 用的 iconify
//! 包导出到 `icons/`（见 `icons/export-icons.mjs`），由 [`crate::Assets`] 一起提供。附录 D §1.8 的
//! 图标管线（lets-icons、分组图标）落地后继续往这里加，调用方不用改。

use gpui::{App, Hsla, IntoElement, Rems, RenderOnce, Styled, Window, prelude::FluentBuilder as _};

use crate::assets::PREFIX;

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
    /// 1.x `i-lucide:image-off`：缩略图解码失败。
    ImageOff,
    Inbox,
    Info,
    /// 1.x `i-lucide:key-round`：敏感内容标记。
    KeyRound,
    /// 1.x `i-lucide:laptop`：来自 macOS 设备的同步记录。
    Laptop,
    /// 1.x `i-lucide:monitor`：来自 Windows 设备的同步记录。
    Monitor,
    Moon,
    /// 1.x `i-lucide:notebook-pen`：便签。
    NotebookPen,
    Palette,
    /// 1.x `i-ph:push-pin-bold`：置顶标记。
    PushPin,
    Plus,
    Search,
    Settings,
    Star,
    Sun,
    TextSize,
}

impl IconName {
    /// 从 1.x iconify 包导出的图标的资源文件名；其余用 gpui-kit-assets 的默认集。
    fn exported(self) -> Option<&'static str> {
        Some(match self {
            Self::ImageOff => "lucide-image-off.svg",
            Self::KeyRound => "lucide-key-round.svg",
            Self::Laptop => "lucide-laptop.svg",
            Self::Monitor => "lucide-monitor.svg",
            Self::NotebookPen => "lucide-notebook-pen.svg",
            Self::PushPin => "ph-push-pin-bold.svg",
            _ => return None,
        })
    }

    fn kit(self) -> gpui_component::IconName {
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
            // 导出的图标不经过这里（见 `kit_icon`），给一个占位以保持 match 穷尽。
            Self::ImageOff
            | Self::KeyRound
            | Self::Laptop
            | Self::Monitor
            | Self::NotebookPen
            | Self::PushPin => Kit::Info,
        }
    }

    pub(crate) fn kit_icon(self) -> gpui_component::Icon {
        match self.exported() {
            Some(file) => gpui_component::Icon::empty().path(format!("{PREFIX}{file}")),
            None => gpui_component::Icon::new(self.kit()),
        }
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
