//! 复选框与开关。都是受控组件：回调收到想要的新值，由调用方保存后重新渲染。
//!
//! 形态接受 gpui-component 的样子（附录 D §1.3），颜色来自 antd token；文字统一为正文 14 px。

use std::rc::Rc;

use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_base::h_flex;
use gpui_component::{
    Disableable as _, Sizable as _, checkbox::Checkbox as KitCheckbox, switch::Switch as KitSwitch,
};

use crate::{
    styled::KpStyled as _,
    theme::{self, TextSize, space},
};

type ChangeHandler = Rc<dyn Fn(bool, &mut Window, &mut App)>;

/// 复选框（antd Checkbox：16 px 方框，文字 14 px）。
#[derive(IntoElement)]
pub struct Checkbox {
    id: ElementId,
    label: Option<SharedString>,
    accessibility_label: Option<SharedString>,
    checked: bool,
    disabled: bool,
    on_change: Option<ChangeHandler>,
}

impl Checkbox {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            accessibility_label: None,
            checked: false,
            disabled: false,
            on_change: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// 没有可见文字时必须给读屏名称（例如多选列表里每行的勾选框）。
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Checkbox {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        KitCheckbox::new(self.id)
            .checked(self.checked)
            .disabled(self.disabled)
            .kp_text(TextSize::Sm)
            .when_some(self.label, |checkbox, label| checkbox.label(label))
            .when_some(self.accessibility_label, |checkbox, label| {
                checkbox.accessibility_label(label)
            })
            .when_some(self.on_change, |checkbox, on_change| {
                checkbox.on_change(move |checked, window, cx| on_change(*checked, window, cx))
            })
    }
}

/// 开关（antd Switch；尺寸接受 gpui-component 的 36×20 / 28×16）。
#[derive(IntoElement)]
pub struct Switch {
    id: ElementId,
    label: Option<SharedString>,
    accessibility_label: Option<SharedString>,
    checked: bool,
    disabled: bool,
    small: bool,
    on_change: Option<ChangeHandler>,
}

impl Switch {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: None,
            accessibility_label: None,
            checked: false,
            disabled: false,
            small: false,
            on_change: None,
        }
    }

    /// 开关右侧的文字，点它也会切换。
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// 没有可见文字时必须给读屏名称（偏好窗里用设置行标题）。
    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = Some(label.into());
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn small(mut self) -> Self {
        self.small = true;
        self
    }

    pub fn on_change(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Switch {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let checked = self.checked;
        let accessibility_label = self.accessibility_label.or_else(|| self.label.clone());
        let switch = KitSwitch::new(self.id.clone())
            .checked(checked)
            .disabled(self.disabled)
            .when(self.small, |switch| switch.small())
            .when_some(accessibility_label, |switch, label| {
                switch.accessibility_label(label)
            })
            .when_some(self.on_change.clone(), |switch, on_change| {
                switch.on_change(move |checked, window, cx| on_change(*checked, window, cx))
            });

        // gpui-component 的开关文字在默认尺寸下是 16 px，这里自己画 14 px 的文字。
        let Some(label) = self.label else {
            return switch.into_any_element();
        };
        let tokens = theme::tokens(cx);

        h_flex()
            .gap(space(2.))
            .items_center()
            .child(switch)
            .child(
                div()
                    .id((self.id, "label"))
                    .kp_text(TextSize::Sm)
                    .text_color(if self.disabled {
                        tokens.disabled
                    } else {
                        tokens.text
                    })
                    .when(!self.disabled, |label| {
                        label.when_some(self.on_change, |label, on_change| {
                            label.on_click(move |_, window, cx| on_change(!checked, window, cx))
                        })
                    })
                    .child(label),
            )
            .into_any_element()
    }
}
