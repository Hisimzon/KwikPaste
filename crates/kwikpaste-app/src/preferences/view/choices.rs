//! 取代下拉框的两种单选控件：短选项排成分段控件，有视觉含义的选项画成带示意图的磁贴。
//!
//! 都是受控的：选中值由调用方从设置里读，点击交给回调写补丁。示意图只用主题 token 画，
//! 主题磁贴按浅色、深色各自的 token 画，其余跟随当前主题。

use std::rc::Rc;

use gpui::{
    AnyElement, App, BoxShadow, Div, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Role, SharedString, StatefulInteractiveElement as _, Styled as _, Toggled,
    Window, div, point, prelude::FluentBuilder as _, px, relative, rems,
};
use kwikpaste_ui::{
    Icon, IconName, KpStyled as _,
    theme::{self, Appearance, SemanticTokens, TextSize, components::PreferencesTokens, space},
};

use super::super::{icons::PrefIcon, schema::TilesKind};
use super::segment;
use crate::i18n;

/// 选中后的回调，参数是选项的存储值。
pub(in crate::preferences) type OnSelect = Rc<dyn Fn(&str, &mut Window, &mut App)>;

/// 磁贴等分整行，只定高度：扁一点省空间。
const TILE_HEIGHT: f32 = 4.;

/// 分段控件：与导出对话框的分段同一个样子，放在设置行右侧。
pub(in crate::preferences) fn segmented(
    id: &'static str,
    label: SharedString,
    options: Vec<(SharedString, SharedString)>,
    selected: Option<&str>,
    disabled: bool,
    on_select: OnSelect,
    cx: &App,
) -> AnyElement {
    let tokens = theme::semantic(cx);
    let count = options.len();

    div()
        .id(SharedString::from(format!("{id}-choices")))
        .role(Role::RadioGroup)
        .aria_label(label)
        .flex()
        .flex_none()
        .gap(space(0.5))
        .p(space(0.5))
        .rounded(theme::radius::MD)
        .bg(tokens.fill.subtle)
        .when(disabled, |track| {
            track.opacity(theme::components(cx).preferences.choice_disabled_opacity)
        })
        .children(
            options
                .into_iter()
                .enumerate()
                .map(|(index, (value, text))| {
                    let checked = selected == Some(value.as_ref());
                    let on_select = on_select.clone();
                    segment(
                        SharedString::from(format!("{id}-{value}")),
                        text,
                        checked,
                        cx,
                    )
                    .role(Role::RadioButton)
                    .aria_toggled(toggled(checked))
                    .aria_position_in_set(index + 1)
                    .aria_size_of_set(count)
                    .when(!disabled, |segment| {
                        segment.on_click(move |_, window, cx| on_select(&value, window, cx))
                    })
                }),
        )
        .into_any_element()
}

/// 一排磁贴：等分整行，上面是扁平的示意图，下面是名称；选中的描强调色边并带勾。
pub(in crate::preferences) fn tiles(
    kind: TilesKind,
    setting_id: &'static str,
    label: SharedString,
    selected: Option<&str>,
    on_select: OnSelect,
    cx: &App,
) -> AnyElement {
    let tokens = theme::semantic(cx);
    let values = kind.values();

    div()
        .id(SharedString::from(format!("{setting_id}-tiles")))
        .role(Role::RadioGroup)
        .aria_label(label)
        .flex()
        .w_full()
        .gap(space(3.))
        .children(values.iter().enumerate().map(|(index, value)| {
            let value = *value;
            let checked = selected == Some(value);
            let name = i18n::t(&format!(
                "preferences:schema.settings.{setting_id}.options.{value}"
            ));
            let group = SharedString::from(format!("{setting_id}-{value}"));
            let on_select = on_select.clone();

            div()
                .id(group.clone())
                .group(group.clone())
                .role(Role::RadioButton)
                .aria_label(name.clone())
                .aria_toggled(toggled(checked))
                .aria_position_in_set(index + 1)
                .aria_size_of_set(values.len())
                .flex()
                .flex_col()
                .items_center()
                .gap(space(1.))
                .flex_1()
                .min_w_0()
                .cursor_pointer()
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(rems(TILE_HEIGHT))
                        .rounded(theme::radius::LG)
                        .overflow_hidden()
                        .border_2()
                        .map(|frame| {
                            if checked {
                                frame.border_color(tokens.accent.solid)
                            } else {
                                frame
                                    .border_color(tokens.border.subtle)
                                    .group_hover(group.clone(), |style| {
                                        style.border_color(tokens.border.strong)
                                    })
                            }
                        })
                        .child(illustration(kind, value, cx))
                        .when(checked, |frame| frame.child(check_badge(tokens))),
                )
                .child(
                    div()
                        .kp_text(TextSize::Sm)
                        .text_center()
                        .text_color(if checked {
                            tokens.text.primary
                        } else {
                            tokens.text.secondary
                        })
                        .child(name),
                )
                .on_click(move |_, window, cx| on_select(value, window, cx))
        }))
        .into_any_element()
}

fn toggled(checked: bool) -> Toggled {
    if checked {
        Toggled::True
    } else {
        Toggled::False
    }
}

fn check_badge(tokens: &SemanticTokens) -> Div {
    div()
        .absolute()
        .top(space(1.))
        .right(space(1.))
        .flex()
        .items_center()
        .justify_center()
        .size(space(4.))
        .rounded_full()
        .bg(tokens.accent.solid)
        .child(
            Icon::new(IconName::Check)
                .size(rems(0.625))
                .color(tokens.text.on_accent),
        )
}

/// 示意图用不透明的 token：磨砂材质下当前 token 整体半透明，画出来会发灰、透出底下的色块。
fn illustration(kind: TilesKind, value: &str, cx: &App) -> AnyElement {
    let tokens = theme::semantic_for(theme::appearance(cx));
    match kind {
        TilesKind::Theme => theme_scene(value),
        TilesKind::Material => material_scene(
            value,
            tokens,
            &theme::components_for(theme::appearance(cx)).preferences,
        ),
        TilesKind::ListStyle => list_style_scene(value, tokens),
        TilesKind::PanelLayout => panel_layout_scene(value, tokens),
        TilesKind::WindowPosition => window_position_scene(value, tokens),
    }
}

/// 示意图的底：铺满磁贴，子元素按比例绝对定位。
fn canvas(background: Hsla) -> Div {
    div().absolute().inset_0().bg(background)
}

/// 按磁贴宽高的比例摆放一块。
fn place(left: f32, top: f32, width: f32, height: f32) -> Div {
    div()
        .absolute()
        .left(relative(left))
        .top(relative(top))
        .w(relative(width))
        .h(relative(height))
}

/// 一行文字的示意：一条圆角短横。
fn text_bar(width: f32, color: Hsla) -> Div {
    div()
        .flex_none()
        .h(rems(0.25))
        .w(relative(width))
        .rounded_full()
        .bg(color)
}

/// 示意图里的窗口：细边框的圆角面板。
fn frame(tokens: &SemanticTokens, background: Hsla) -> Div {
    div()
        .size_full()
        .rounded(theme::radius::SM)
        .overflow_hidden()
        .border_1()
        .border_color(tokens.border.default)
        .bg(background)
}

/// 窗口里的两行文字。
fn text_lines(tokens: &SemanticTokens) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(rems(0.375))
        .p(rems(0.5))
        .child(text_bar(0.85, tokens.fill.strong))
        .child(text_bar(0.55, tokens.fill.strong))
}

fn theme_scene(value: &str) -> AnyElement {
    let scene = |appearance| {
        let tokens = theme::semantic_for(appearance);
        canvas(tokens.surface.window).child(
            place(0.1, 0.22, 0.8, 0.56)
                .child(frame(tokens, tokens.surface.panel).child(text_lines(tokens))),
        )
    };
    let half = |appearance, right: bool| {
        div()
            .relative()
            .w(relative(0.5))
            .h_full()
            .overflow_hidden()
            .child(
                scene(appearance)
                    .w(relative(2.))
                    .when(right, |scene| scene.left(relative(-1.))),
            )
    };

    match value {
        "light" => scene(Appearance::Light).into_any_element(),
        "dark" => scene(Appearance::Dark).into_any_element(),
        _ => div()
            .absolute()
            .inset_0()
            .flex()
            .child(half(Appearance::Light, false))
            .child(half(Appearance::Dark, true))
            .into_any_element(),
    }
}

/// 同一张壁纸上的窗口：默认不透明；云母只取壁纸的色调；磨砂透出背后模糊的色块。
fn material_scene(value: &str, tokens: &SemanticTokens, tints: &PreferencesTokens) -> AnyElement {
    const WINDOW: (f32, f32, f32, f32) = (0.15, 0.2, 0.75, 0.6);
    // 色块的左、上（磁贴比例）和直径（rem）。
    const BLOB: (f32, f32, f32) = (0.05, 0.14, 2.5);
    let (left, top, width, height) = WINDOW;
    let (blob_left, blob_top, blob_size) = BLOB;
    let blob = tints.material_blob;
    let wallpaper = tints.material_wallpaper;

    // 各平台的真实材质透明度差别很大（Windows 磨砂靠整体缩放表面），示意图用自己的固定比例。
    let mut backdrop = div().absolute().inset_0();
    let surface = match value {
        "mica" => {
            backdrop = backdrop.bg(tints.material_mica_backdrop);
            tints.material_mica_panel
        }
        "acrylic" => {
            // 盖住清晰的色块，再把色块换到窗口坐标里，用大半径阴影画成模糊的样子。
            backdrop = backdrop.bg(wallpaper).child(
                div()
                    .absolute()
                    .left(relative((blob_left - left) / width))
                    .top(relative((blob_top - top) / height))
                    .size(rems(blob_size))
                    .rounded_full()
                    .shadow(vec![BoxShadow {
                        color: blob,
                        offset: point(px(0.), px(0.)),
                        blur_radius: px(12.),
                        spread_radius: px(-2.),
                        inset: false,
                    }]),
            );
            tints.material_frosted_panel
        }
        _ => tokens.surface.panel,
    };

    canvas(wallpaper)
        .child(
            div()
                .absolute()
                .left(relative(blob_left))
                .top(relative(blob_top))
                .size(rems(blob_size))
                .rounded_full()
                .bg(blob),
        )
        .child(
            place(left, top, width, height)
                .rounded(theme::radius::SM)
                .overflow_hidden()
                .child(backdrop)
                .child(div().absolute().inset_0().child(frame(tokens, surface))),
        )
        .into_any_element()
}

fn list_style_scene(value: &str, tokens: &SemanticTokens) -> AnyElement {
    let card = value == "card";
    let rows = (0..3).map(|index| {
        div().flex_1().map(|row| {
            if card {
                row.rounded(theme::radius::SM)
                    .bg(tokens.surface.raised)
                    .border_1()
                    .border_color(tokens.border.default)
            } else if index > 0 {
                row.border_t_1().border_color(tokens.border.strong)
            } else {
                row
            }
        })
    });

    canvas(if card {
        tokens.surface.window
    } else {
        tokens.surface.panel
    })
    .child(
        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .gap(rems(if card { 0.25 } else { 0. }))
            .px(rems(0.75))
            .py(rems(0.5))
            .children(rows),
    )
    .into_any_element()
}

fn panel_layout_scene(value: &str, tokens: &SemanticTokens) -> AnyElement {
    let column = |bars: usize| {
        div()
            .flex()
            .flex_col()
            .justify_center()
            .gap(rems(0.3125))
            .px(rems(0.5))
            .children((0..bars).map(|_| text_bar(1., tokens.fill.strong)))
    };
    let window = frame(tokens, tokens.surface.panel)
        .flex()
        .child(column(3).flex_1())
        .when(value == "split", |window| {
            window.child(
                column(2)
                    .w(relative(0.34))
                    .border_l_1()
                    .border_color(tokens.border.divider)
                    .bg(tokens.fill.faint),
            )
        });

    canvas(tokens.surface.window)
        .child(place(0.1, 0.2, 0.8, 0.6).child(window))
        .into_any_element()
}

fn window_position_scene(value: &str, tokens: &SemanticTokens) -> AnyElement {
    let (left, top) = match value {
        "followCursor" => (0.5, 0.3),
        "remember" => (0.08, 0.14),
        _ => (0.36, 0.25),
    };
    let window = place(left, top, 0.28, 0.5).child(
        frame(tokens, tokens.surface.panel)
            .flex()
            .items_center()
            .justify_center()
            .when(value == "remember", |window| {
                window.child(PrefIcon::Pin.view(rems(0.75), tokens.accent.solid))
            }),
    );

    canvas(tokens.surface.window)
        .child(window)
        .when(value == "followCursor", |scene| {
            scene.child(
                place(0.42, 0.12, 0.1, 0.3)
                    .child(PrefIcon::MousePointer.view(rems(0.875), tokens.text.primary)),
            )
        })
        .into_any_element()
}
