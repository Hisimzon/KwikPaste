//! 有界 HTML 预览投影；只保留语义，不加载资源、不执行脚本、不解释 CSS。

use html5ever::{parse_document, tendril::TendrilSink};
use markup5ever_rcdom::{Handle, NodeData, RcDom};
use serde::Serialize;

const MAX_INPUT_BYTES: usize = 256 * 1024;
const MAX_NODES: usize = 4096;
const MAX_DEPTH: usize = 32;
const MAX_TEXT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewDocument {
    pub blocks: Vec<PreviewBlock>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "content", rename_all = "camelCase")]
pub enum PreviewBlock {
    Paragraph(Vec<PreviewInline>),
    Heading {
        level: u8,
        runs: Vec<PreviewInline>,
    },
    List {
        ordered: bool,
        items: Vec<Vec<PreviewBlock>>,
    },
    Table {
        rows: Vec<Vec<Vec<PreviewInline>>>,
    },
    Blockquote(Vec<PreviewBlock>),
    CodeBlock(String),
}

/// `text` 中的换行表示显式换行；普通 HTML 空白折叠，代码块保留空白。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInline {
    pub text: String,
    pub style: PreviewInlineStyle,
}

/// 链接仅为展示数据；渲染者不得把 href 当作资源加载地址。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInlineStyle {
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikethrough: bool,
    pub code: bool,
    pub href: Option<String>,
}

/// 输入先按 UTF-8 边界截到 256 KiB，再用 HTML5 规则修复结构。
/// 投影最多访问 4096 个节点、32 层、保留 64 KiB 文本（含 href）；任一超限即停止。
/// 不支持的容器保留文字；脚本、样式、嵌入资源和表单控件丢弃整个子树。
pub fn parse_preview_html(source: &str) -> PreviewDocument {
    let input = prefix(source, MAX_INPUT_BYTES);
    let dom = parse_document(RcDom::default(), Default::default()).one(input);
    let mut parser = Projection::default();
    let mut flow = Flow::default();
    parser.children(
        &dom.document,
        0,
        &PreviewInlineStyle::default(),
        &mut flow,
        false,
    );
    flow.flush();

    PreviewDocument {
        blocks: flow.blocks,
        truncated: input.len() < source.len() || parser.stopped,
    }
}

/// 在字节上限以内保留完整字符。
fn prefix(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[derive(Default)]
struct Flow {
    blocks: Vec<PreviewBlock>,
    runs: Vec<PreviewInline>,
}

impl Flow {
    /// 段落边界不保留折叠后的尾空格，但保留显式换行。
    fn flush(&mut self) {
        trim_runs(&mut self.runs);
        if !self.runs.is_empty() {
            self.blocks
                .push(PreviewBlock::Paragraph(std::mem::take(&mut self.runs)));
        }
    }
}

fn trim_runs(runs: &mut Vec<PreviewInline>) {
    while let Some(last) = runs.last_mut() {
        last.text.truncate(last.text.trim_end_matches(' ').len());
        if !last.text.is_empty() {
            break;
        }
        runs.pop();
    }
}

#[derive(Default)]
struct Projection {
    nodes: usize,
    text_bytes: usize,
    stopped: bool,
}

impl Projection {
    /// 所有遍历共用预算；超限后不继续处理兄弟节点。
    fn enter(&mut self, depth: usize) -> bool {
        if self.stopped || self.nodes >= MAX_NODES || depth > MAX_DEPTH {
            self.stopped = true;
            return false;
        }
        self.nodes += 1;
        true
    }

    /// 合并同样式相邻文本，跨标签折叠 HTML 空白，并按完整字符截断。
    fn text(
        &mut self,
        text: &str,
        style: &PreviewInlineStyle,
        runs: &mut Vec<PreviewInline>,
        preserve: bool,
    ) {
        if self.stopped {
            return;
        }
        let mut normalized = String::new();
        let mut space = runs
            .last()
            .is_none_or(|run| run.text.ends_with([' ', '\n']));
        for c in text.chars() {
            if !preserve && matches!(c, ' ' | '\t' | '\r' | '\n' | '\u{c}') {
                if !space {
                    normalized.push(' ');
                }
                space = true;
            } else {
                normalized.push(c);
                space = false;
            }
        }
        if normalized.is_empty() {
            return;
        }
        let merge = runs.last().is_some_and(|run| run.style == *style);
        let href_bytes = if merge {
            0
        } else {
            style.href.as_ref().map_or(0, String::len)
        };
        let remaining = MAX_TEXT_BYTES - self.text_bytes;
        if href_bytes > remaining {
            self.stopped = true;
            return;
        }
        let kept = prefix(&normalized, remaining - href_bytes);
        self.text_bytes += kept.len() + href_bytes;
        self.stopped = kept.len() < normalized.len();
        if !kept.is_empty() {
            if merge {
                if let Some(last) = runs.last_mut() {
                    last.text.push_str(kept);
                }
            } else {
                runs.push(PreviewInline {
                    text: kept.to_owned(),
                    style: style.clone(),
                });
            }
        }
    }

    /// 将子节点按原顺序投影；inline 模式用于标题、表格单元格和代码块。
    fn children(
        &mut self,
        node: &Handle,
        depth: usize,
        style: &PreviewInlineStyle,
        flow: &mut Flow,
        inline: bool,
    ) {
        for child in node.children.borrow().iter() {
            self.node(child, depth + 1, style, flow, inline, false);
            if self.stopped {
                break;
            }
        }
    }

    /// inline 模式把内部块边界转为换行；pre 模式保留原始空白。
    fn inlines(
        &mut self,
        node: &Handle,
        depth: usize,
        style: &PreviewInlineStyle,
        pre: bool,
    ) -> Vec<PreviewInline> {
        let mut flow = Flow::default();
        for child in node.children.borrow().iter() {
            self.node(child, depth + 1, style, &mut flow, true, pre);
            if self.stopped {
                break;
            }
        }
        if !pre {
            trim_runs(&mut flow.runs);
        }
        flow.runs
    }

    /// 根据有限标签集建立块；未知标签只作为透明容器。
    fn node(
        &mut self,
        node: &Handle,
        depth: usize,
        inherited: &PreviewInlineStyle,
        flow: &mut Flow,
        inline: bool,
        pre: bool,
    ) {
        if !self.enter(depth) {
            return;
        }
        let NodeData::Element { name, attrs, .. } = &node.data else {
            if let NodeData::Text { contents } = &node.data {
                self.text(&contents.borrow(), inherited, &mut flow.runs, pre);
            }
            return;
        };
        let tag = name.local.as_ref();
        if matches!(
            tag,
            "head"
                | "script"
                | "style"
                | "template"
                | "img"
                | "iframe"
                | "object"
                | "embed"
                | "input"
                | "textarea"
                | "select"
                | "button"
                | "svg"
                | "math"
        ) {
            return;
        }
        let mut style = inherited.clone();
        match tag {
            "b" | "strong" => style.bold = true,
            "i" | "em" => style.italic = true,
            "u" => style.underline = true,
            "s" | "strike" | "del" => style.strikethrough = true,
            "code" => style.code = true,
            "a" => {
                style.href = attrs
                    .borrow()
                    .iter()
                    .find(|a| a.name.local.as_ref() == "href")
                    .map(|a| a.value.to_string())
            }
            _ => {}
        }
        if tag == "br" {
            self.text("\n", &style, &mut flow.runs, true);
            return;
        }
        let block = matches!(
            tag,
            "p" | "div"
                | "section"
                | "article"
                | "header"
                | "footer"
                | "main"
                | "form"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "ul"
                | "ol"
                | "li"
                | "table"
                | "tr"
                | "blockquote"
                | "pre"
                | "hr"
        );
        if inline {
            if block
                && !flow.runs.is_empty()
                && !flow.runs.last().is_some_and(|run| run.text.ends_with('\n'))
            {
                self.text("\n", &style, &mut flow.runs, true);
            }
            for child in node.children.borrow().iter() {
                self.node(child, depth + 1, &style, flow, true, pre);
                if self.stopped {
                    break;
                }
            }
            if block && !pre {
                self.text("\n", &style, &mut flow.runs, true);
            }
            return;
        }
        if block {
            flow.flush();
        }
        match tag {
            "h1" | "h2" | "h3" => flow.blocks.push(PreviewBlock::Heading {
                level: match tag {
                    "h1" => 1,
                    "h2" => 2,
                    _ => 3,
                },
                runs: self.inlines(node, depth, &style, false),
            }),
            "ul" | "ol" => {
                let mut items = Vec::new();
                self.list_items(node, depth, &style, &mut items);
                flow.blocks.push(PreviewBlock::List {
                    ordered: tag == "ol",
                    items,
                });
            }
            "table" => {
                let mut rows = Vec::new();
                self.table_rows(node, depth, &style, &mut rows);
                flow.blocks.push(PreviewBlock::Table { rows });
            }
            "blockquote" => {
                let mut quote = Flow::default();
                self.children(node, depth, &style, &mut quote, false);
                quote.flush();
                flow.blocks.push(PreviewBlock::Blockquote(quote.blocks));
            }
            "pre" => {
                let runs = self.inlines(node, depth, &style, true);
                flow.blocks.push(PreviewBlock::CodeBlock(
                    runs.into_iter().map(|run| run.text).collect(),
                ));
            }
            _ => self.children(node, depth, &style, flow, false),
        }
        if block {
            flow.flush();
        }
    }

    /// 列表项内继续使用同一块模型，因而可以包含一层或更多有界子列表。
    fn list_items(
        &mut self,
        node: &Handle,
        depth: usize,
        style: &PreviewInlineStyle,
        items: &mut Vec<Vec<PreviewBlock>>,
    ) {
        for child in node.children.borrow().iter() {
            if !self.enter(depth + 1) {
                break;
            }
            if let NodeData::Element { name, .. } = &child.data {
                if name.local.as_ref() == "li" {
                    let mut item = Flow::default();
                    self.children(child, depth + 1, style, &mut item, false);
                    item.flush();
                    items.push(item.blocks);
                }
            }
        }
    }

    /// 只接受 HTML 表格的行组、行和单元格，不展开跨度或嵌入资源。
    fn table_rows(
        &mut self,
        node: &Handle,
        depth: usize,
        style: &PreviewInlineStyle,
        rows: &mut Vec<Vec<Vec<PreviewInline>>>,
    ) {
        for child in node.children.borrow().iter() {
            if !self.enter(depth + 1) {
                break;
            }
            let NodeData::Element { name, .. } = &child.data else {
                continue;
            };
            match name.local.as_ref() {
                "thead" | "tbody" | "tfoot" => self.table_rows(child, depth + 1, style, rows),
                "tr" => {
                    let mut cells = Vec::new();
                    for cell in child.children.borrow().iter() {
                        if !self.enter(depth + 2) {
                            break;
                        }
                        if let NodeData::Element { name, .. } = &cell.data {
                            if matches!(name.local.as_ref(), "td" | "th") {
                                cells.push(self.inlines(cell, depth + 2, style, false));
                            }
                        }
                    }
                    rows.push(cells);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> PreviewInline {
        PreviewInline {
            text: text.to_owned(),
            style: PreviewInlineStyle::default(),
        }
    }

    #[test]
    fn projects_supported_html_without_active_content() {
        let doc = parse_preview_html(concat!(
            "<h1>Title</h1><ul><li>One<ol><li>Nested</li></ol></li></ul>",
            "<table><tr><th>A</th><td>B &amp; C</td></tr></table>",
            "<p><b><a href='https://example.com'>Link</a></b><br>tail</p>",
            "<script>alert('hidden')</script><style>hidden</style><img src='file:///secret'>",
            "<iframe>hidden</iframe><!-- hidden --><form>Label<input value='hidden'></form>",
            "<blockquote><p>Quote</p></blockquote><pre><code>let x = 1;\n  x</code></pre>",
            "<h4><i><u><s><code>Other</code></s></u></i></h4>"
        ));
        assert_eq!(
            doc,
            PreviewDocument {
                truncated: false,
                blocks: vec![
                    PreviewBlock::Heading {
                        level: 1,
                        runs: vec![run("Title")]
                    },
                    PreviewBlock::List {
                        ordered: false,
                        items: vec![vec![
                            PreviewBlock::Paragraph(vec![run("One")]),
                            PreviewBlock::List {
                                ordered: true,
                                items: vec![vec![PreviewBlock::Paragraph(vec![run("Nested")])]]
                            },
                        ]]
                    },
                    PreviewBlock::Table {
                        rows: vec![vec![vec![run("A")], vec![run("B & C")]]]
                    },
                    PreviewBlock::Paragraph(vec![
                        PreviewInline {
                            text: "Link".to_owned(),
                            style: PreviewInlineStyle {
                                bold: true,
                                href: Some("https://example.com".to_owned()),
                                ..Default::default()
                            }
                        },
                        run("\ntail")
                    ]),
                    PreviewBlock::Paragraph(vec![run("Label")]),
                    PreviewBlock::Blockquote(vec![PreviewBlock::Paragraph(vec![run("Quote")])]),
                    PreviewBlock::CodeBlock("let x = 1;\n  x".to_owned()),
                    PreviewBlock::Paragraph(vec![PreviewInline {
                        text: "Other".to_owned(),
                        style: PreviewInlineStyle {
                            italic: true,
                            underline: true,
                            strikethrough: true,
                            code: true,
                            ..Default::default()
                        }
                    }]),
                ]
            }
        );
    }

    #[test]
    fn caps_truncate_without_errors() {
        for source in [
            format!(
                "<p>kept</p><!--{}--><p>lost</p>",
                "x".repeat(MAX_INPUT_BYTES)
            ),
            format!(
                "<p>kept</p>{}<p>lost</p>",
                "<span></span>".repeat(MAX_NODES)
            ),
            format!("<p>kept</p>{}<p>lost</p>", "<div>".repeat(MAX_DEPTH + 1)),
        ] {
            let doc = parse_preview_html(&source);
            assert!(doc.truncated);
            assert_eq!(doc.blocks, vec![PreviewBlock::Paragraph(vec![run("kept")])]);
        }
        let doc = parse_preview_html(&format!(
            "<p>{}</p><p>lost</p>",
            "界".repeat(MAX_TEXT_BYTES)
        ));
        assert!(doc.truncated);
        assert_eq!(
            doc.blocks,
            vec![PreviewBlock::Paragraph(vec![run(
                &"界".repeat(MAX_TEXT_BYTES / 3)
            )])]
        );
    }
}
