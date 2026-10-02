//! 编辑态：面板里的输入框要接收键盘和输入法时，面板临时变成可激活的前台窗口。
//!
//! Windows 的做法（附录 C §1 第 6 条）：
//! - 进入：记下原前台窗口 → 钩子关导航（线程保留）→ 去掉 `WS_EX_NOACTIVATE` → 取前台。
//!   鼠标触发直接 `SetForegroundWindow`（本进程刚收到点击，系统允许）；键盘或程序触发先注入一次
//!   带标记的 Alt，由自己的钩子吞掉，再 `SetForegroundWindow`。不“先直接试、失败再补 Alt”：
//!   失败那一次本身就会造成幽灵激活。钩子没确认吞掉 Alt、或前台没拿到，就整体回滚，不进编辑态；
//!   绝不用会把 Alt 漏给目标应用的兜底（GPUI `activate()`、tao 式补 Alt、`AttachThreadInput`）。
//! - 退出：恢复 `WS_EX_NOACTIVATE` 和钩子导航；面板还是前台时把前台还给原窗口。
//!   编辑中直接隐藏面板时，系统会自己把前台交还，不再调用 `SetForegroundWindow`。
//!
//! macOS 的面板是非激活 NSPanel，成为 key 窗口不激活应用，不需要这一套。TODO(macOS)：
//! 显示时 `makeKeyWindow`，编辑态直接聚焦输入框。

/// 进入编辑态的触发方式，决定取前台的路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditTrigger {
    /// 在输入框上按下了鼠标。
    Mouse,
    /// 钩子转来的快捷键（Ctrl+F 等）或程序触发（打开带输入框的弹层）。
    Keyboard,
}

impl EditTrigger {
    pub fn name(self) -> &'static str {
        match self {
            Self::Mouse => "mouse",
            Self::Keyboard => "keyboard",
        }
    }
}

/// 一次进入编辑态的记录，日志和自测用。
#[derive(Debug, Clone, Copy, Default)]
pub struct EditReport {
    /// 进入前的前台窗口（Windows 句柄；macOS 为 0）。
    pub previous_foreground: isize,
    /// 键盘触发时钩子确认吞掉了带标记的 Alt。
    pub marked_alt_swallowed: bool,
    /// 从开始到拿到前台的毫秒数。
    pub elapsed_ms: f64,
}
