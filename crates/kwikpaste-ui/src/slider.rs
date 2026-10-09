//! 整数滑块：拖动时只更新显示，松手时提交，避免每次鼠标移动都写设置文件。

use gpui::{
    App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Rems, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, Window, div,
};
use gpui_component::slider::{Slider as KitSlider, SliderEvent, SliderState as KitSliderState};

/// 由视图长期持有的整数滑块状态；上游组件类型不穿透隔离层。
#[derive(Clone)]
pub struct SliderState {
    state: Entity<KitSliderState>,
    min: u8,
    max: u8,
}

impl SliderState {
    pub fn new(value: u8, min: u8, max: u8, cx: &mut App) -> Self {
        let max = max.max(min);
        let state = cx.new(|_| {
            KitSliderState::new()
                .min(f32::from(min))
                .max(f32::from(max))
                .step(1.0)
                .default_value(f32::from(value.clamp(min, max)))
        });
        Self { state, min, max }
    }

    pub fn value(&self, cx: &App) -> u8 {
        integer_value(self.state.read(cx).value().end(), self.min, self.max)
    }

    /// 拖动事件只用于更新显示；返回的订阅须由视图保存。
    pub fn on_change<V: 'static>(
        &self,
        cx: &mut Context<V>,
        handler: impl Fn(&mut V, &mut Context<V>) + 'static,
    ) -> Subscription {
        cx.subscribe(&self.state, move |view, _, event: &SliderEvent, cx| {
            if matches!(event, SliderEvent::Change(_)) {
                handler(view, cx);
            }
        })
    }

    /// 松手时只提交一次最终整数值，包含拖到控件外再松手的路径。
    pub fn on_commit<V: 'static>(
        &self,
        cx: &mut Context<V>,
        handler: impl Fn(&mut V, u8, &mut Context<V>) + 'static,
    ) -> Subscription {
        let (min, max) = (self.min, self.max);
        cx.subscribe(&self.state, move |view, _, event: &SliderEvent, cx| {
            if let SliderEvent::Release(value) = event {
                handler(view, integer_value(value.end(), min, max), cx);
            }
        })
    }
}

/// 只允许范围内的整数；异常浮点值也不能传到设置模型。
fn integer_value(value: f32, min: u8, max: u8) -> u8 {
    (value.round() as u8).clamp(min, max.max(min))
}

/// 水平滑块，使用组件主题的轨道与手柄颜色。
#[derive(IntoElement)]
pub struct Slider {
    state: SliderState,
    width: Rems,
    disabled: bool,
    accessibility_label: SharedString,
}

impl Slider {
    pub fn new(state: &SliderState) -> Self {
        Self {
            state: state.clone(),
            width: gpui::rems(10.),
            disabled: false,
            accessibility_label: SharedString::default(),
        }
    }

    pub fn width(mut self, width: Rems) -> Self {
        self.width = width;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn accessibility_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accessibility_label = label.into();
        self
    }
}

impl RenderOnce for Slider {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        div()
            .id(("integer-slider", self.state.state.entity_id()))
            .role(Role::Group)
            .aria_label(self.accessibility_label)
            .w(self.width)
            .max_w_full()
            .child(KitSlider::new(&self.state.state).disabled(self.disabled))
    }
}

#[cfg(test)]
mod tests {
    use super::integer_value;

    #[test]
    fn slider_values_are_rounded_and_clamped() {
        assert_eq!(integer_value(80.1, 0, 100), 80);
        assert_eq!(integer_value(80.6, 0, 100), 81);
        assert_eq!(integer_value(-1.0, 0, 100), 0);
        assert_eq!(integer_value(255.0, 0, 100), 100);
        assert_eq!(integer_value(f32::NAN, 0, 100), 0);
    }
}
