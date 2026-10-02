//! 下拉菜单与右键菜单（antd `Dropdown`）。弹层画在窗口内（gpui-component 的 PopupMenu），
//! 不开原生菜单，不抢前台。打开时菜单拿焦点，钩子转来的 ↑/↓/Enter/Esc 由它处理。

use std::rc::Rc;

use gpui::{
    AnyElement, App, Div, ElementId, InteractiveElement, Interactivity, IntoElement, ParentElement,
    SharedString, Stateful, StyleRefinement, Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_base::Selectable;
use gpui_component::menu::{ContextMenuExt as _, DropdownMenu, PopupMenu, PopupMenuItem};

use crate::{
    icon::IconName,
    styled::KpStyled as _,
    theme::{self, TextSize},
};

type ClickHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// 菜单项前面的图标。
#[derive(Clone, Debug)]
pub enum MenuIcon {
    Name(IconName),
    /// 资源路径（见 [`crate::Assets`]），例如分组图标。
    Path(SharedString),
}

/// 一个菜单项。
#[derive(Clone)]
pub struct MenuItem {
    label: SharedString,
    icon: Option<MenuIcon>,
    danger: bool,
    checked: bool,
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
            danger: false,
            checked: false,
            on_click: Rc::new(on_click),
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(MenuIcon::Name(icon));
        self
    }

    pub fn icon_path(mut self, path: impl Into<SharedString>) -> Self {
        self.icon = Some(MenuIcon::Path(path.into()));
        self
    }

    /// 危险操作：文字为错误色（antd `danger: true`）。
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

/// 菜单里的一行。
#[derive(Clone)]
pub enum MenuEntry {
    Item(MenuItem),
    Separator,
}

impl From<MenuItem> for MenuEntry {
    fn from(item: MenuItem) -> Self {
        Self::Item(item)
    }
}

fn build_menu(menu: PopupMenu, entries: Vec<MenuEntry>, cx: &App) -> PopupMenu {
    let tokens = theme::tokens(cx);

    entries.into_iter().fold(menu, |menu, entry| match entry {
        MenuEntry::Separator => menu.separator(),
        MenuEntry::Item(item) => {
            let label = item.label.clone();
            let color = if item.danger {
                tokens.error
            } else {
                tokens.text
            };
            let handler = item.on_click.clone();
            let entry = PopupMenuItem::element(move |_, _| {
                div()
                    .kp_text(TextSize::Sm)
                    .text_color(color)
                    .child(label.clone())
            })
            .checked(item.checked)
            .on_click(move |_, window, cx| handler(window, cx))
            .when_some(item.icon, |entry, icon| match icon {
                MenuIcon::Name(name) => entry.icon(name.kit_icon()),
                MenuIcon::Path(path) => entry.icon(gpui_component::Icon::empty().path(path)),
            });

            menu.item(entry)
        }
    })
}

/// 给元素挂右键菜单。`build` 在每次打开时调用。
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
            build_menu(menu, entries, cx)
        })
        .into_any_element()
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
            build_menu(menu, entries, cx)
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
