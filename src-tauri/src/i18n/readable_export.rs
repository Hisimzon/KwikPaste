//! 可读格式导出文档的短文案。

use crate::settings::Language;

pub struct ExportWords {
    pub headers: [&'static str; 6],
    pub ungrouped: &'static str,
    pub title: &'static str,
    pub warning: &'static str,
    pub yes: &'static str,
    pub no: &'static str,
}

/// 导出文件里的「类型」列：子类型优先，认不出的取值原样输出。
pub fn type_label<'a>(lang: Language, kind: &'a str, sub_kind: Option<&'a str>) -> &'a str {
    let value = sub_kind.unwrap_or(kind);
    let (zh, en) = match value {
        "text" => ("文本", "Text"),
        "image" => ("图片", "Image"),
        "files" => ("文件", "Files"),
        "rtf" => ("RTF", "RTF"),
        "html" => ("HTML", "HTML"),
        "url" => ("链接", "Link"),
        "email" => ("邮箱", "Email"),
        "color" => ("颜色", "Color"),
        "path" => ("路径", "Path"),
        _ => return value,
    };
    match lang {
        Language::ZhCN => zh,
        Language::EnUS => en,
    }
}

/// 返回导出文件中的本地化表头、标题和资源引用说明。
pub fn words(lang: Language) -> ExportWords {
    match lang {
        Language::ZhCN => ExportWords {
            headers: ["分组", "类型", "内容", "备注", "创建时间", "收藏"],
            ungrouped: "未分组",
            title: "快贴数据导出",
            warning: "这是未加密的可读内容，不是可恢复备份。图片和文件仅保留原始引用，不包含实际资源；换电脑后可能无法访问。HTML / RTF 保留原始标记。",
            yes: "是",
            no: "否",
        },
        Language::EnUS => ExportWords {
            headers: ["Group", "Type", "Content", "Note", "Created at", "Favorite"],
            ungrouped: "Ungrouped",
            title: "KwikPaste data export",
            warning: "This is an unencrypted readable export, not a restorable backup. Images and files retain references only; resources are not included and may be unavailable on another computer. HTML / RTF retain their original markup.",
            yes: "Yes",
            no: "No",
        },
    }
}
