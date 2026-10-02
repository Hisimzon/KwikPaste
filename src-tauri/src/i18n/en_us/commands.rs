use crate::i18n::keys::CommandKey as Key;

/// 返回美式英文 Tauri 命令错误根因文案。
pub fn label(key: Key) -> &'static str {
    match key {
        Key::ExportNoGroups => "Select at least one group or Ungrouped",
        Key::ExportPreviewChanged => "Export data changed. Preview again before confirming",
        Key::ExportEmpty => "There are no records to export in this scope",
        Key::ExportInvalidTarget => {
            "Choose a valid export folder or a file path with the matching extension"
        }
        Key::ExportExcelLimit => {
            "Content exceeds Excel cell or row limits. Use Markdown or a smaller scope"
        }

        Key::DragSourceFilesMissing => "The dragged source files no longer exist",
        Key::DragImageMissing => "The image file no longer exists",
        Key::DragTextEmpty => "Text content is empty",
        Key::ExternalUrlUnsupported => "Only links starting with http or https can be opened",
        Key::FragmentUnavailable => "The selected text is no longer in this record",
        Key::SplitTextOnly => "Only text records can be split into words",
        Key::SplitSensitiveRedacted => "Sensitive content is redacted and can't be split",
        Key::PortableStorageFixed => {
            "KwikPaste Portable always keeps its data in the data folder next to the app"
        }
    }
}
