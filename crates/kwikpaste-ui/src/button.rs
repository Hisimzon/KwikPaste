//! 按钮与图标按钮。颜色来自冻结的 antd token（经 gpui-component 主题映射），高度走 rem。

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _, rems,
};
use gpui_component::{
    Disableable as _, Selectable as _, Sizable as _,
    button::{Button as KitButton, ButtonVariants as _},
};

use crate::{icon::IconName, theme::control_height, tooltip::TooltipExt as _};

/// 按钮类型，对应 antd 的 `type` / `danger`。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonKind {
    /// antd 默认按钮：容器底色加 `colorBorder` 描边。
    #[default]
    Default,
    /// antd `type="primary"`。
    Primary,
    /// antd `type="primary" danger`：危险操作的确认按钮。
    Danger,
    /// antd `type="text"`：无底无边，悬停出现填充色；图标按钮多用它。
    Ghost,
    /// antd `type="link"`。
    Link,
}

/// 按钮高度。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    /// 20 px：卡片上的快捷动作。
    XSmall,
    /// 24 px：antd `size="small"`。
    Small,
    /// 32 px：antd 默认（`controlHeight`）。
    #[default]
    Medium,
}

type ClickHandler = Rc<dyn Fn(&ClickEvent, &mut Window, &mut App)>;

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<IconName>,
    icon_only: bool,
    kind: ButtonKind,
    size: ButtonSize,
    loading: bool,
    disabled: bool,
    selected: Option<bool>,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
}

impl Button {
    /// 文字按钮。
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: Some(label.into()),
            icon: None,
            icon_only: false,
            kind: ButtonKind::Default,
            size: ButtonSize::Medium,
            loading: false,
            disabled: false,
            selected: None,
            tooltip: None,
            on_click: None,
        }
    }

    /// 图标按钮。`label` 必填：它既是悬停提示，也是读屏名称。
    pub fn icon(id: impl Into<ElementId>, icon: IconName, label: impl Into<SharedString>) -> Self {
        let label = label.into();
        Self {
            icon: Some(icon),
            icon_only: true,
            tooltip: Some(label.clone()),
            kind: ButtonKind::Ghost,
            ..Self::new(id, label)
        }
    }

    /// 文字前加图标。
    pub fn with_icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn kind(mut self, kind: ButtonKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn primary(self) -> Self {
        self.kind(ButtonKind::Primary)
    }

    pub fn danger(self) -> Self {
        self.kind(ButtonKind::Danger)
    }

    pub fn ghost(self) -> Self {
        self.kind(ButtonKind::Ghost)
    }

    pub fn link(self) -> Self {
        self.kind(ButtonKind::Link)
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    pub fn small(self) -> Self {
        self.size(ButtonSize::Small)
    }

    pub fn xsmall(self) -> Self {
        self.size(ButtonSize::XSmall)
    }

    /// 加载中：显示转圈并忽略点击，外观保持原样。
    pub fn loading(mut self, loading: bool) -> Self {
        self.loading = loading;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// 选中态（切换类按钮），同时以 toggled 状态暴露给读屏。
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = Some(selected);
        self
    }

    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let label = self.label.unwrap_or_default();
        let button = KitButton::new(self.id.clone())
            .map(|button| match self.kind {
                ButtonKind::Default => button,
                ButtonKind::Primary => button.primary(),
                ButtonKind::Danger => button.danger(),
                ButtonKind::Ghost => button.ghost(),
                ButtonKind::Link => button.link(),
            })
            .map(|button| match self.size {
                ButtonSize::XSmall => button.xsmall(),
                ButtonSize::Small => button.small(),
                // gpui-component 的默认尺寸是 16 px 字；antd 默认按钮是 14 px 字、32 px 高、左右 15 px，
                // 所以借小号的字号和图标，再把高度和内边距撑回 antd 的值。
                ButtonSize::Medium => button.small().h(control_height::MD).map(|button| {
                    if self.icon_only {
                        button.w(control_height::MD)
                    } else {
                        button.px(rems(0.9375))
                    }
                }),
            })
            .when_some(self.icon, |button, icon| button.icon(icon.kit_icon()))
            // 转圈图标替换的是按钮自己的图标，纯文字按钮加载时要先给一个占位图标。
            .when(self.loading && self.icon.is_none(), |button| {
                button.icon(gpui_component::Icon::new(
                    gpui_component::IconName::LoaderCircle,
                ))
            })
            .map(|button| {
                if self.icon_only {
                    button.accessibility_label(label.clone())
                } else {
                    button.label(label.clone())
                }
            })
            .loading(self.loading)
            .disabled(self.disabled)
            .when_some(self.selected, |button, selected| {
                button.selected(selected).toggled(selected)
            })
            .when_some(self.on_click, |button, on_click| {
                button.on_click(move |event, window, cx| on_click(event, window, cx))
            });

        let Some(tooltip) = self.tooltip else {
            return button.into_any_element();
        };

        tooltip_host(self.id, button.into_any_element(), tooltip)
    }
}

/// 给不带 Tooltip 接口的子元素包一层只负责悬停提示的容器。
fn tooltip_host(id: ElementId, child: AnyElement, tooltip: SharedString) -> AnyElement {
    div()
        .id((id, "tooltip"))
        .flex_none()
        .child(child)
        .kp_tooltip(tooltip)
        .into_any_element()
}
