//! 面板命令的出口与编辑态的归属。
//!
//! 搜索框和备注框都要先让面板进入编辑态（Windows 上面板临时成为前台窗口），收到
//! `PanelEvent::EditingStarted` 后再聚焦自己的输入框。两者共用面板的同一组事件，所以请求时先记下
//! 是谁要的（[`EditTarget`]），事件来了由对应的那个视图接手。
//!
//! 界面发给面板的命令都经 [`request_panel`]：交互自测装上 [`RequestLog`] 后命令只记录不执行，
//! 自测再自己发出相应的面板事件，不会真的取前台、隐藏面板。

use gpui::{App, Global};

use crate::platform::{self, EditTrigger, PanelCommand};

/// 请求编辑态的一方。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditTarget {
    Search,
    Note,
}

#[derive(Default)]
struct Editing {
    target: Option<EditTarget>,
}

impl Global for Editing {}

/// 记录界面发给面板的命令（交互自测用）；装上之后命令不再送到面板。
#[derive(Default)]
pub struct RequestLog {
    pub commands: Vec<PanelCommand>,
}

impl Global for RequestLog {}

/// 发给面板的命令（显示、隐藏、进出编辑态）。
pub fn request_panel(cx: &mut App, command: PanelCommand) {
    if cx.has_global::<RequestLog>() {
        cx.global_mut::<RequestLog>().commands.push(command);
        return;
    }

    platform::request(cx, command);
}

/// 为 `target` 请求编辑态。
pub fn begin(target: EditTarget, trigger: EditTrigger, cx: &mut App) {
    log::debug!("editing requested for {target:?} by {}", trigger.name());
    cx.default_global::<Editing>().target = Some(target);
    request_panel(cx, PanelCommand::BeginEditing(trigger));
}

/// 退出编辑态（焦点由收到 `EditingEnded` 的列表拿回）。
pub fn end(cx: &mut App) {
    request_panel(cx, PanelCommand::EndEditing);
}

/// 当前编辑态（或正在请求的编辑态）属于谁。
pub fn target(cx: &App) -> Option<EditTarget> {
    cx.try_global::<Editing>()
        .and_then(|editing| editing.target)
}

/// 编辑态结束或被拒绝后清掉归属。
pub fn clear(cx: &mut App) {
    cx.default_global::<Editing>().target = None;
}
