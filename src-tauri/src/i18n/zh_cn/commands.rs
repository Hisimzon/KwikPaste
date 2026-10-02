use crate::i18n::keys::CommandKey as Key;

/// 返回简体中文 Tauri 命令错误根因文案。
pub fn label(key: Key) -> &'static str {
    match key {
        Key::ExportNoGroups => "请选择至少一个分组或未分组",
        Key::ExportPreviewChanged => "导出内容已变化，请重新预览后确认",
        Key::ExportEmpty => "所选范围没有可导出的记录",
        Key::ExportInvalidTarget => "请选择有效的导出目录或对应格式的文件路径",
        Key::ExportExcelLimit => {
            "内容超过 Excel 的单元格或行数限制，请改用 Markdown 或缩小导出范围"
        }

        Key::DragSourceFilesMissing => "拖拽源文件已不存在",
        Key::DragImageMissing => "图片文件已不存在",
        Key::DragTextEmpty => "文本内容为空",
        Key::ExternalUrlUnsupported => "只能打开 http 或 https 开头的链接",
        Key::FragmentUnavailable => "所选内容已不在这条记录中",
        Key::SplitTextOnly => "只有文本记录可以拆词",
        Key::SplitSensitiveRedacted => "敏感内容已脱敏显示，不能拆词",
        Key::PortableStorageFixed => "便携版的数据固定保存在程序文件夹的 data 目录里",
    }
}
