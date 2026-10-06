//! 列表卡片与占位骨架，按 1.x `ClipboardCard.tsx` / `cards/*` 的样子画（卡片风格、标准密度为默认）。
//!
//! 尺寸全部来自 [`LayoutSpec`] 的设计 px，经 [`dp`] 换成 rem，随文本缩放；颜色全部取 `KpTokens`。
//! 交互（悬停、点击）由列表视图挂在返回的元素上；悬停快捷动作、多选复选框这类带回调的部件也由
//! 列表画好后经 [`CardState`] 交进来，这里只管摆放。

use std::{rc::Rc, sync::Arc, time::Duration};

use chrono::{DateTime, Local};
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Div, ElementId, Image, ImageSource,
    InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, Rems, RenderImage,
    SharedString, StatefulInteractiveElement as _, Styled, Window, div, img,
    prelude::FluentBuilder as _, pulsating_between, relative, rems,
};
use kwikpaste_ui::{
    Icon, IconName, KeyHint, KpStyled as _, TooltipExt as _,
    theme::{KpTokens, TextSize, css_color, radius},
};

use crate::{
    clipboard::model::{
        item::{FileRow, FilesPreview, ItemKind, ListItem, Platform, SubKind, TypeKey},
        layout::{ImageBox, LayoutSpec, predict_image_box},
        time_label::time_label,
    },
    i18n::{t, t_args},
};

/// 1.x 设计 px 换成 rem（基准 16 px）。
pub fn dp(value: f32) -> Rems {
    rems(value / 16.)
}

/// 骨架脉动的帧率上限（附录 D §2.1：循环动画限 10 fps）。
const PULSE_MAX_FPS: f32 = 10.;
/// Tailwind `animate-pulse` 的周期。
const PULSE_PERIOD: Duration = Duration::from_secs(2);

/// 渲染卡片需要的环境。
pub struct CardEnv<'a> {
    pub tokens: &'static KpTokens,
    pub layout: &'a LayoutSpec,
    pub now: DateTime<Local>,
    pub reduce_motion: bool,
}

/// 图片区的状态（缩略图或单图文件）。
#[derive(Clone)]
pub enum Visual {
    Ready(Arc<RenderImage>),
    Loading,
    Failed,
}

/// 卡片的状态标记。
#[derive(Default)]
pub struct CardState {
    /// 当前项：中性灰底。
    pub active: bool,
    /// 指针在卡片上：铺一层悬停底色。
    pub hovered: bool,
    pub image: Option<Visual>,
    /// 指针在卡片上：有备注且开了“悬停显示原文”时显示原文。
    pub show_original: bool,
    /// 按住修饰键时来源图标上的数字角标（1–9、0）。
    pub hint: Option<char>,
    /// 多选时已勾上：卡片铺一层淡主色。
    pub checked: bool,
    /// 上一行铺了底色（当前项、悬停或勾选）：这一行上方的分隔线不画，免得贴着色块多出一道线。
    pub after_highlight: bool,
    /// 悬停快捷动作（列表画好的按钮行）；有它时头部行不显示时间。
    pub actions: Option<AnyElement>,
    /// 多选的复选框。
    pub checkbox: Option<AnyElement>,
    /// 点卡片下方的快捷信息；多选时为空，快捷信息不响应点击。
    pub on_snippet: Option<SnippetHandler>,
    /// 按住修饰键时链接、邮箱卡片的正文是可点的链接（1.x `isLinkActive`）：点了打开。
    pub on_link: Option<LinkHandler>,
    /// 当前条目在无障碍列表中的 1-based 位置。
    pub position: usize,
    /// 无障碍列表的总条数。
    pub set_size: usize,
}

/// 快捷信息被点了：参数是那段文字。
pub type SnippetHandler = Rc<dyn Fn(Arc<str>, &mut Window, &mut App)>;

/// 链接正文被点了。
pub type LinkHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// 这一行会不会铺底色（当前项、悬停或勾选）。
pub fn is_highlighted(active: bool, hovered: bool, checked: bool) -> bool {
    active || hovered || checked
}

/// 超过这个长度的片段（多为链接）大概率会被截断，悬停时补一个完整内容的提示（1.x 同）。
const SNIPPET_TOOLTIP_MIN_CHARS: usize = 32;

/// 一行卡片（含外层间距）。返回的元素带 id，调用方再挂交互。
pub fn card(
    env: &CardEnv<'_>,
    item: &ListItem,
    index: usize,
    state: CardState,
) -> gpui::Stateful<Div> {
    let tokens = env.tokens;
    let layout = env.layout;
    let body = content(
        env,
        item,
        index,
        state.image,
        state.show_original,
        state.on_link,
    );
    let pinned = item.is_pinned;
    let sensitive = item.shows_sensitive_mark();
    let hint = state.hint;
    let actions = state.actions;
    let checkbox = state.checkbox;
    let on_snippet = state.on_snippet;
    let highlighted = is_highlighted(state.active, state.hovered, state.checked);
    let divider = !layout.seamless && state.position > 1 && !highlighted && !state.after_highlight;
    // 分隔线左端与正文对齐：紧凑密度的正文在 16 px 来源图标右侧。
    let divider_left = layout.item_padding_x
        + layout.card_padding_x
        + if layout.header_row {
            0.
        } else {
            16. + layout.body_gap
        };

    // 条目是扁平的行：平时没有底色和描边，悬停、当前项（中性灰）、勾选（淡主色）时才铺一层
    // 底色。卡片风格是圆角块、行间画细分隔线；无间风格贴边，用底边线分隔。
    let highlight = if state.checked {
        Some(tokens.primary_bg)
    } else if state.active {
        Some(tokens.fill_secondary)
    } else if state.hovered {
        // 比当前项浅一档，指针扫过时不会和当前项混淆。
        Some(tokens.fill_quaternary)
    } else {
        None
    };
    let mut frame = div()
        .relative()
        .flex()
        .overflow_hidden()
        .py(dp(layout.card_padding_y))
        .px(dp(layout.card_padding_x))
        .when(layout.seamless, |frame| {
            frame.border_b_1().border_color(tokens.split)
        })
        // 卡片风格保留 1 px 的透明描边：行高估算里算了这圈描边。
        .when(!layout.seamless, |frame| {
            frame
                .border_1()
                .border_color(kwikpaste_ui::theme::transparent())
                .rounded(radius::LG)
        })
        .when_some(highlight, |frame, highlight| frame.bg(highlight));

    frame = if layout.header_row {
        // 来源、类型、时间这行小号灰字在正文上方。
        frame
            .flex_col()
            .gap(dp(layout.body_gap))
            .child(meta(
                env,
                item,
                hint,
                actions,
                checkbox,
                status_marks(tokens, pinned, sensitive, true),
            ))
            .child(body)
            .children(snippets(env, item, 0, on_snippet))
    } else {
        frame
            .items_start()
            .gap(dp(layout.body_gap))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .h(dp(20.))
                    .items_center()
                    .child(hinted_icon(env, item, hint)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .gap(dp(2.))
                    .child(body)
                    .children(snippets(env, item, 0, on_snippet)),
            )
            .when(pinned || sensitive, |frame| {
                frame.child(status_marks(tokens, pinned, sensitive, true))
            })
            .when_some(checkbox, |frame, checkbox| {
                frame.child(
                    div()
                        .flex()
                        .flex_none()
                        .h(dp(20.))
                        .items_center()
                        .child(checkbox),
                )
            })
            // 没有头部行时动作按钮浮在卡片右上角，不占行高（1.x `floating`）。
            .when_some(actions, |frame, actions| {
                frame.child(
                    div()
                        .absolute()
                        .top(dp(4.))
                        .right(dp(4.))
                        .p(dp(2.))
                        .rounded(radius::MD)
                        .border_1()
                        .border_color(tokens.border_secondary)
                        .bg(tokens.bg_elevated)
                        .shadow(tokens.shadow_card.to_vec())
                        .child(actions),
                )
            })
    };

    div()
        // 按记录 id 而不是行号标识：刷新后新记录挪到光标下时，悬停、点击状态不会继承给它。
        .id(ElementId::Name(SharedString::from(item.id.clone())))
        .role(gpui::Role::ListBoxOption)
        .aria_label(accessibility_name(item, &env.now))
        .aria_selected(state.active)
        .aria_position_in_set(state.position)
        .aria_size_of_set(state.set_size)
        .relative()
        .px(dp(layout.item_padding_x))
        .pt(dp(layout.item_gap))
        // 卡片风格的行间分隔线画在行上方的间距里，左右与正文对齐；第一行和挨着色块的不画。
        .when(divider, |row| {
            row.child(
                div()
                    .absolute()
                    .top(dp(layout.item_gap / 2.))
                    .left(dp(divider_left))
                    .right(dp(layout.item_padding_x + layout.card_padding_x))
                    .h(gpui::px(1.))
                    .bg(tokens.split),
            )
        })
        .child(frame)
        .when(state.active, |row| row.aria_active_descendant())
}

/// 未加载行的骨架：照两行文本卡片画（1.x `renderPlaceholderItem`），标准档高 84 px。
pub fn placeholder(env: &CardEnv<'_>) -> AnyElement {
    let tokens = env.tokens;
    let layout = env.layout;
    let bar = |width: gpui::DefiniteLength| {
        div()
            .h(dp(12.))
            .w(width)
            .rounded(radius::SM)
            .bg(tokens.fill_secondary)
    };
    let line = |width: f32| {
        div()
            .flex()
            .h(dp(20.))
            .items_center()
            .child(bar(relative(width)))
    };
    let lines = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .child(line(0.75))
        .child(line(0.5));
    let icon = div()
        .size(dp(16.))
        .flex_none()
        .rounded(radius::SM)
        .bg(tokens.fill_secondary);

    let frame = div()
        .flex()
        .py(dp(layout.card_padding_y))
        .px(dp(layout.card_padding_x))
        .border_color(if layout.seamless {
            tokens.split
        } else {
            kwikpaste_ui::theme::transparent()
        })
        .when(layout.seamless, |frame| frame.border_b_1())
        .when(!layout.seamless, |frame| {
            frame.border_1().rounded(radius::LG)
        });
    let frame = if layout.header_row {
        frame.flex_col().gap(dp(layout.body_gap)).child(
            div()
                .flex()
                .h(dp(layout.header_height))
                .items_center()
                .gap(dp(4.))
                .child(icon)
                .child(bar(dp(64.).into())),
        )
    } else {
        frame.items_start().gap(dp(layout.body_gap)).child(
            div()
                .flex()
                .flex_none()
                .h(dp(20.))
                .items_center()
                .child(icon),
        )
    };

    div()
        .px(dp(layout.item_padding_x))
        .pt(dp(layout.item_gap))
        .child(frame.child(lines))
        .into_any_element()
}

/// 卡片要显示的图片：缩略图（图片记录）或单图文件记录的原图；返回（路径，显示框）。
pub fn image_target(item: &ListItem, layout: &LayoutSpec) -> Option<ImageTarget> {
    match item.kind {
        ItemKind::Image => Some(ImageTarget {
            path: item.image_thumbnail_path.clone(),
            file_name: Some(item.content.clone()),
            display: item.image_display.unwrap_or_else(|| {
                predict_image_box(item.width, item.height, layout.image_max_height)
            }),
        }),
        ItemKind::Files if item.files_preview_kind == Some(FilesPreview::ImagePreview) => {
            let row = item.file_rows().first()?;
            Some(ImageTarget {
                path: Some(row.path.clone()),
                file_name: None,
                display: predict_image_box(row.width, row.height, layout.image_max_height),
            })
        }
        _ => None,
    }
}

/// 卡片图片区的数据来源。
pub struct ImageTarget {
    /// 已知的图片文件路径；图片记录还没有缩略图时为空。
    pub path: Option<Arc<str>>,
    /// 缺缩略图时向数据源要缩略图用的文件名（图片记录的 `content`）。
    pub file_name: Option<Arc<str>>,
    pub display: ImageBox,
}

fn meta(
    env: &CardEnv<'_>,
    item: &ListItem,
    hint: Option<char>,
    actions: Option<AnyElement>,
    checkbox: Option<AnyElement>,
    marks: Div,
) -> Div {
    let tokens = env.tokens;
    let origin = item
        .origin_device_name
        .as_deref()
        .map(|name| t_args("clipboard:origin.fromDevice", &[("name", name)]));

    div()
        .flex()
        .h(dp(env.layout.header_height))
        .items_center()
        .justify_between()
        .gap(dp(6.))
        .kp_text(TextSize::Xs)
        .text_color(tokens.tertiary)
        .child(
            div()
                .flex()
                .min_w_0()
                .items_center()
                .gap(dp(6.))
                .overflow_hidden()
                .child(hinted_icon(env, item, hint))
                .child(
                    div()
                        .truncate()
                        .text_color(tokens.secondary)
                        .child(type_label(item.type_key())),
                )
                .children(origin.map(|origin| {
                    div()
                        .truncate()
                        .child(SharedString::from(format!("· {origin}")))
                }))
                .child(div().flex_none().child(SharedString::from(format!(
                    "· {}",
                    time_label(item.created_at, &env.now)
                )))),
        )
        // 右侧：置顶、敏感标记；悬停时换成快捷动作（置顶动作本身带状态，标记不再重复），多选时跟复选框。
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(dp(6.))
                .when(actions.is_none(), |side| side.child(marks))
                .children(actions)
                .children(checkbox),
        )
}

/// 来源图标；按住修饰键时叠一个数字角标，图标本身隐去（1.x `KeyHint` 包着来源图标）。
fn hinted_icon(env: &CardEnv<'_>, item: &ListItem, hint: Option<char>) -> AnyElement {
    let icon = app_icon(env, item);
    let Some(key) = hint else {
        return icon;
    };

    div()
        .relative()
        .flex()
        .flex_none()
        .child(div().flex().opacity(0.).child(icon))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(KeyHint::new(SharedString::from(key.to_string()))),
        )
        .into_any_element()
}

fn type_label(key: TypeKey) -> SharedString {
    match key {
        TypeKey::Text => t("clipboard:types.text"),
        TypeKey::Html => t("clipboard:types.html"),
        TypeKey::Rtf => t("clipboard:types.rtf"),
        TypeKey::Url => t("clipboard:types.url"),
        TypeKey::Email => t("clipboard:types.email"),
        TypeKey::Color => t("clipboard:types.color"),
        TypeKey::Path => t("clipboard:types.path"),
        TypeKey::Image => t("clipboard:types.image"),
        TypeKey::Files => t("clipboard:types.files"),
    }
}

/// 生成条目在 UIA/AccessKit 中使用的可读名称。
pub fn accessibility_name(item: &ListItem, now: &DateTime<Local>) -> SharedString {
    let summary = item
        .summary
        .as_deref()
        .or_else(|| match item.kind {
            ItemKind::Image => Some(&*item.content),
            ItemKind::Files => item.file_rows().first().map(|row| &*row.name),
            ItemKind::Text => None,
        })
        .and_then(|value| value.lines().next())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let kind = type_label(item.type_key());
    let time = time_label(item.created_at, now);
    match summary {
        Some(summary) => format!("{kind} · {summary} · {time}").into(),
        None => format!("{kind} · {time}").into(),
    }
}

/// 来源应用图标；同步来的记录用设备平台图标；都没有时是快贴的图标（1.x 的顺序）。
/// 头部行里配 12 px 的小字用 14 px，紧凑密度里和正文并排用 16 px，三种来源同一尺寸。
fn app_icon(env: &CardEnv<'_>, item: &ListItem) -> AnyElement {
    let size = if env.layout.header_row {
        dp(14.)
    } else {
        dp(16.)
    };
    if item.source_app_id.is_some()
        && let Some(path) = &item.source_app_icon_path
    {
        return img(std::path::PathBuf::from(&**path))
            .size(size)
            .flex_none()
            .into_any_element();
    }

    if item.origin_device_id.is_some() {
        let icon = match item.platform {
            Platform::Macos => IconName::Laptop,
            Platform::Windows => IconName::Monitor,
        };
        return div()
            .flex_none()
            .child(Icon::new(icon).size(size).color(env.tokens.secondary))
            .into_any_element();
    }

    img(ImageSource::Image(logo()))
        .size(size)
        .flex_none()
        .into_any_element()
}

/// 快贴图标（1.x `public/logo.png`，macOS 用 `logo-mac.png`）。
pub fn logo() -> Arc<Image> {
    use std::sync::LazyLock;

    static LOGO: LazyLock<Arc<Image>> = LazyLock::new(|| {
        let bytes: &[u8] = if cfg!(target_os = "macos") {
            include_bytes!("../../../assets/logo-mac.png")
        } else {
            include_bytes!("../../../assets/logo.png")
        };
        Arc::new(Image::from_bytes(gpui::ImageFormat::Png, bytes.to_vec()))
    });

    LOGO.clone()
}

fn content(
    env: &CardEnv<'_>,
    item: &ListItem,
    index: usize,
    image: Option<Visual>,
    show_original: bool,
    on_link: Option<LinkHandler>,
) -> AnyElement {
    // 有备注时显示备注；悬停且开了“显示原文”时换回原内容（1.x `NoteContentSwitcher`）。
    if let Some(note) = &item.note
        && !show_original
    {
        return note_annotation(env, note);
    }

    match item.kind {
        ItemKind::Text => text_body(env, item, on_link),
        ItemKind::Image => image_body(env, item, index, image),
        ItemKind::Files if item.files_preview_kind == Some(FilesPreview::ImagePreview) => {
            image_body(env, item, index, image)
        }
        ItemKind::Files => files_body(env, item.file_rows()),
    }
}

fn shared(text: &Arc<str>) -> SharedString {
    // 制表符在 GPUI 里宽度为 0，换成空格免得两边的字粘在一起。
    if text.contains('\t') {
        return SharedString::from(text.replace('\t', "    "));
    }

    SharedString::from(text)
}

fn text_body(env: &CardEnv<'_>, item: &ListItem, on_link: Option<LinkHandler>) -> AnyElement {
    let tokens = env.tokens;
    if item.sub_kind == Some(SubKind::Color)
        && let Some(value) = &item.color_preview
        && let Some(color) = css_color(value)
    {
        return div()
            .flex()
            .items_center()
            .gap(dp(8.))
            .kp_text(TextSize::Sm)
            .child(
                div()
                    .size(dp(16.))
                    .flex_none()
                    .rounded(radius::SM)
                    .border_1()
                    .border_color(tokens.border_secondary)
                    .bg(color),
            )
            .child(div().kp_mono().child(shared(value)))
            .into_any_element();
    }

    let summary = item.summary.as_ref().map(shared).unwrap_or_default();
    let link = on_link.filter(|_| matches!(item.sub_kind, Some(SubKind::Url | SubKind::Email)));
    let Some(on_link) = link else {
        return div()
            .w_full()
            .kp_text(TextSize::Sm)
            .line_clamp(env.layout.text_max_lines)
            .text_ellipsis()
            .child(summary)
            .into_any_element();
    };

    // 按住修饰键：主色、下划线，点了打开；按下不冒泡给卡片（不选中、不触发单击粘贴）。
    div()
        .id("card-link")
        .w_full()
        .kp_text(TextSize::Sm)
        .line_clamp(env.layout.text_max_lines)
        .text_ellipsis()
        .text_color(tokens.primary)
        .underline()
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_link(window, cx);
        })
        .child(summary)
        .into_any_element()
}

fn note_annotation(env: &CardEnv<'_>, note: &Arc<str>) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap(dp(4.))
        .kp_text(TextSize::Sm)
        .child(
            div().flex_none().pt(dp(3.)).child(
                Icon::new(IconName::NotebookPen)
                    .size(dp(14.))
                    .color(env.tokens.primary),
            ),
        )
        .child(div().flex_1().min_w_0().child(shared(note)))
        .into_any_element()
}

fn image_body(
    env: &CardEnv<'_>,
    item: &ListItem,
    index: usize,
    image: Option<Visual>,
) -> AnyElement {
    let tokens = env.tokens;
    let Some(target) = image_target(item, env.layout) else {
        return div().into_any_element();
    };
    let (width, height) = (dp(target.display.width), dp(target.display.height));

    match image.unwrap_or(Visual::Loading) {
        // 圆角缩略图，上面叠一圈不占位置的细描边：白底截图、浅色图片也有边界，行高不变。
        Visual::Ready(image) => div()
            .flex()
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(width)
                    .h(height)
                    .child(
                        img(ImageSource::Render(image))
                            .size_full()
                            .rounded(radius::SM),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .rounded(radius::SM)
                            .border_1()
                            .border_color(tokens.split),
                    ),
            )
            .into_any_element(),
        Visual::Failed => div()
            .flex()
            .child(
                div()
                    .flex()
                    .flex_none()
                    .w(width)
                    .h(height)
                    .items_center()
                    .justify_center()
                    .rounded(radius::SM)
                    .bg(tokens.fill_tertiary)
                    .child(
                        Icon::new(IconName::ImageOff)
                            .size(dp(target.display.height.min(16.)))
                            .color(tokens.quaternary),
                    ),
            )
            .into_any_element(),
        Visual::Loading => {
            let skeleton = div()
                .flex_none()
                .w(width)
                .h(height)
                .rounded(radius::SM)
                .bg(tokens.fill_tertiary);
            let skeleton = if env.reduce_motion {
                skeleton.into_any_element()
            } else {
                skeleton
                    .with_animation(
                        ElementId::NamedInteger("thumbnail-pulse".into(), index as u64),
                        Animation::new(PULSE_PERIOD)
                            .repeat()
                            .with_easing(pulsating_between(0.5, 1.))
                            .with_max_fps(PULSE_MAX_FPS),
                        |skeleton, delta| skeleton.opacity(delta),
                    )
                    .into_any_element()
            };

            div().flex().child(skeleton).into_any_element()
        }
    }
}

fn files_body(env: &CardEnv<'_>, rows: &[FileRow]) -> AnyElement {
    let tokens = env.tokens;

    div()
        .flex()
        .flex_col()
        .gap(dp(4.))
        .kp_text(TextSize::Sm)
        .children(rows.iter().map(|row| {
            div()
                .flex()
                .items_center()
                .gap(dp(4.))
                .min_w_0()
                .children(row.icon_path.as_ref().map(|path| {
                    img(std::path::PathBuf::from(&**path))
                        .size(dp(20.))
                        .flex_none()
                }))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .when(!row.exists, |name| {
                            name.line_through().text_decoration_color(tokens.text)
                        })
                        .child(shared(&row.name)),
                )
        }))
        .into_any_element()
}

/// 卡片下方的快捷信息：一行放得下的片段，放不下的整个隐藏（1.x `QuickSnippets`）。
/// `marks` 是右下角水印的个数，给水印留出位置。
fn snippets(
    env: &CardEnv<'_>,
    item: &ListItem,
    marks: usize,
    on_pick: Option<SnippetHandler>,
) -> Option<AnyElement> {
    if item.quick_snippets.is_empty() {
        return None;
    }
    let tokens = env.tokens;
    let reserve = match marks {
        0 => 0.,
        1 => 24.,
        _ => 44.,
    };

    Some(
        div()
            .flex()
            .flex_wrap()
            .h(dp(24.))
            .gap(dp(4.))
            .overflow_hidden()
            .pr(dp(reserve))
            .children(
                item.quick_snippets
                    .iter()
                    .enumerate()
                    .map(|(index, snippet)| {
                        let chip = div()
                            .id(ElementId::NamedInteger("snippet".into(), index as u64))
                            .flex()
                            .h(dp(24.))
                            .max_w_full()
                            .items_center()
                            .px(dp(8.))
                            .rounded(radius::MD)
                            .bg(tokens.fill_tertiary)
                            .kp_text(TextSize::Xs)
                            .text_color(tokens.secondary)
                            .child(div().truncate().child(shared(snippet)));
                        let Some(on_pick) = on_pick.clone() else {
                            return chip.into_any_element();
                        };
                        let text = snippet.clone();
                        let long = snippet.chars().count() >= SNIPPET_TOOLTIP_MIN_CHARS;

                        // 按下时拦住事件，不触发卡片的选中、单击粘贴和双击粘贴（1.x `SnippetChip`）。
                        chip.cursor_pointer()
                            .hover(|style| style.bg(tokens.fill_secondary).text_color(tokens.text))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, window, cx| {
                                cx.stop_propagation();
                                on_pick(text.clone(), window, cx);
                            })
                            .when(long, |chip| chip.kp_tooltip(shared(snippet)))
                            .into_any_element()
                    }),
            )
            .into_any_element(),
    )
}

/// 置顶、敏感标记：有头部行时是右下角 20 px 的水印，没有时是正文右侧 16 px 的小图标。
fn status_marks(tokens: &KpTokens, pinned: bool, sensitive: bool, inline: bool) -> Div {
    let size = dp(14.);
    let marks = div()
        .flex()
        .gap(dp(4.))
        .when(pinned, |marks| {
            marks.child(
                Icon::new(IconName::PushPin)
                    .size(size)
                    .color(tokens.warning),
            )
        })
        .when(sensitive, |marks| {
            marks.child(
                Icon::new(IconName::KeyRound)
                    .size(size)
                    .color(tokens.quaternary),
            )
        });

    if inline {
        marks.flex_none().h(dp(20.)).items_center()
    } else {
        marks.absolute().right(dp(8.)).bottom(dp(8.)).items_end()
    }
}
