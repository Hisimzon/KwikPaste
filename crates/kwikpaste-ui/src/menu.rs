//! 下拉菜单与右键菜单（antd `Dropdown`）。弹层画在窗口内（gpui-component 的 PopupMenu），
//! 不开原生菜单，不抢前台。打开时菜单拿焦点，钩子转来的 ↑/↓/←/→/Enter/Esc 由它处理。

use std::rc::Rc;

use gpui::{
    AnyElement, App, Context, Div, ElementId, InteractiveElement, Interactivity, IntoElement,
    ParentElement, SharedString, Stateful, StyleRefinement, Styled, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_base::Selectable;
use gpui_component::menu::{ContextMenuExt as _, DropdownMenu, PopupMenu, PopupMenuItem};

use crate::{
    icon::IconName,
    styled::KpStyled as _,
    theme::{self, TextSize, space},
};

type ClickHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// gpui-component 菜单的 key context。
const POPUP_MENU_CONTEXT: &str = "PopupMenu";

/// 菜单项前面的图标。
#[derive(Clone, Debug)]
pub enum MenuIcon {
    Name(IconName),
    /// 资源路径（见 [`crate::Assets`]），例如分组图标。
    Path(SharedString),
}

impl MenuIcon {
    fn kit_icon(self) -> gpui_component::Icon {
        match self {
            Self::Name(name) => name.kit_icon(),
            Self::Path(path) => gpui_component::Icon::empty().path(path),
        }
    }
}

/// 一个菜单项。
#[derive(Clone)]
pub struct MenuItem {
    label: SharedString,
    icon: Option<MenuIcon>,
    shortcut: Option<SharedString>,
    danger: bool,
    checked: bool,
    disabled: bool,
    on_click: ClickHandler,
}

impl MenuItem {
    pub fn new(
        label: impl Into<SharedString>,
        on_click: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        Self {
            label: label.into(),
            icon: None,
            shortcut: None,
            danger: false,
            checked: false,
            disabled: false,
            on_click: Rc::new(on_click),
        }
    }

    /// 右侧的快捷键提示（1.x 右键菜单的 accelerator，`text-ant-description text-xs`）。
    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(MenuIcon::Name(icon));
        self
    }

    pub fn icon_path(mut self, path: impl Into<SharedString>) -> Self {
        self.icon = Some(MenuIcon::Path(path.into()));
        self
    }

    /// 危险操作：文字和图标为错误色（antd `danger: true`）。
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    /// 选中项（antd `selectedKeys`）。
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }
}

/// 子菜单：悬停或 → 展开（1.x 右键菜单的“移动到分组”）。
#[derive(Clone)]
pub struct Submenu {
    label: SharedString,
    icon: Option<MenuIcon>,
    entries: Vec<MenuEntry>,
}

impl Submenu {
    pub fn new(label: impl Into<SharedString>, entries: Vec<MenuEntry>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            entries,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(MenuIcon::Name(icon));
        self
    }
}

/// 菜单里的一行。
#[derive(Clone)]
pub enum MenuEntry {
    Item(MenuItem),
    Submenu(Submenu),
    Separator,
}

impl From<MenuItem> for MenuEntry {
    fn from(item: MenuItem) -> Self {
        Self::Item(item)
    }
}

impl From<Submenu> for MenuEntry {
    fn from(submenu: Submenu) -> Self {
        Self::Submenu(submenu)
    }
}

fn build_menu(
    menu: PopupMenu,
    entries: Vec<MenuEntry>,
    window: &mut Window,
    cx: &mut Context<PopupMenu>,
) -> PopupMenu {
    entries.into_iter().fold(menu, |menu, entry| match entry {
        MenuEntry::Separator => menu.separator(),
        MenuEntry::Submenu(submenu) => {
            let entries = submenu.entries;
            menu.submenu_with_icon(
                submenu.icon.map(MenuIcon::kit_icon),
                submenu.label,
                window,
                cx,
                move |menu, window, cx| build_menu(menu, entries.clone(), window, cx),
            )
        }
        MenuEntry::Item(item) => menu.item(menu_item(item, cx)),
    })
}

fn menu_item(item: MenuItem, cx: &App) -> PopupMenuItem {
    let tokens = theme::tokens(cx);
    let label = item.label;
    let shortcut = item.shortcut;
    let danger = item.danger;
    let color = if danger { tokens.error } else { tokens.text };
    let handler = item.on_click;

    PopupMenuItem::element(move |_, cx| {
        let tokens = theme::tokens(cx);

        div()
            .flex()
            .flex_1()
            .min_w_0()
            .items_center()
            .justify_between()
            .gap(space(3.))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .kp_text(TextSize::Sm)
                    .text_color(color)
                    .child(label.clone()),
            )
            .when_some(shortcut.clone(), |row, shortcut| {
                row.child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .kp_text(TextSize::Xs)
                        .text_color(tokens.description)
                        .child(shortcut),
                )
            })
    })
    .checked(item.checked)
    .disabled(item.disabled)
    .on_click(move |_, window, cx| handler(window, cx))
    .when_some(item.icon, |entry, icon| {
        let icon = icon.kit_icon();
        entry.icon(if danger {
            icon.text_color(tokens.error)
        } else {
            icon
        })
    })
}

/// 给元素挂右键菜单。`build` 在每次打开时调用；返回空列表时不弹出。
pub fn context_menu<E>(
    element: E,
    build: impl Fn(&mut Window, &mut App) -> Vec<MenuEntry> + 'static,
) -> AnyElement
where
    E: InteractiveElement + ParentElement + Styled + IntoElement + 'static,
{
    element
        .context_menu(move |menu, window, cx| {
            let entries = build(window, cx);
            build_menu(menu, entries, window, cx)
        })
        .into_any_element()
}

/// 焦点在打开的菜单里时关掉它（连同父菜单），例如窗口隐藏时。没有打开的菜单时什么也不做。
pub fn dismiss_menu(window: &mut Window, cx: &mut App) {
    if menu_open(window) {
        window.dispatch_action(Box::new(gpui_base::actions::Cancel), cx);
    }
}

/// 焦点是否在打开的菜单里。
pub fn menu_open(window: &Window) -> bool {
    window
        .context_stack()
        .iter()
        .any(|context| context.contains(POPUP_MENU_CONTEXT))
}

/// 下拉菜单的触发元素：一个可自由排版的容器，点击时打开菜单。
pub struct MenuTrigger {
    base: Stateful<Div>,
    selected: bool,
}

impl MenuTrigger {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            base: div().id(id),
            selected: false,
        }
    }

    /// 点击打开 `build` 给出的菜单（antd `Dropdown trigger={["click"]}`），菜单左上角对齐触发元素左下角。
    pub fn dropdown(
        self,
        build: impl Fn(&mut Window, &mut App) -> Vec<MenuEntry> + 'static,
    ) -> AnyElement {
        self.dropdown_menu(move |menu, window, cx| {
            let entries = build(window, cx);
            build_menu(menu, entries, window, cx)
        })
        .into_any_element()
    }
}

impl Styled for MenuTrigger {
    fn style(&mut self) -> &mut StyleRefinement {
        self.base.style()
    }
}

impl InteractiveElement for MenuTrigger {
    fn interactivity(&mut self) -> &mut Interactivity {
        self.base.interactivity()
    }
}

impl ParentElement for MenuTrigger {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.base.extend(elements);
    }
}

impl Selectable for MenuTrigger {
    fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    fn is_selected(&self) -> bool {
        self.selected
    }

    /// 打开菜单不改触发元素的外观：选中态由调用方按业务含义设置。
    fn open(self, _: bool) -> Self {
        self
    }
}

impl IntoElement for MenuTrigger {
    type Element = Stateful<Div>;

    fn into_element(self) -> Self::Element {
        self.base.cursor_pointer()
    }
}

impl DropdownMenu for MenuTrigger {}
