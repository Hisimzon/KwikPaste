//! 可读格式导出：只读数据库，预览确认后生成 Excel / Markdown，不参与备份恢复。
//!
//! 宿主先 [`Core::preview_readable_export`] 拿数量、分组与指纹给用户确认，再带着指纹
//! [`Core::export_readable_data`]；两次之间数据变了指纹就对不上，导出报错而不是静默导出新数据。

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use rust_xlsxwriter::{Format, Workbook};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use tempfile::NamedTempFile;

use crate::error::Result;
use crate::i18n::commands::{label, Key};
use crate::i18n::readable_export::{type_label, words, ExportWords as Words};
use crate::root::Core;
use crate::settings::Language;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    Xlsx,
    Markdown,
}

impl ExportFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Xlsx => "xlsx",
            Self::Markdown => "md",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportOptions {
    pub format: ExportFormat,
    pub favorites_only: bool,
    /// None 为全部分组；Some 为指定分组（空数组仅在选择未分组时有效）。
    pub group_ids: Option<Vec<String>>,
    pub include_ungrouped: bool,
    pub split_by_group: bool,
    pub include_sensitive: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSummary {
    pub id: String,
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportPreview {
    pub fingerprint: String,
    pub item_count: usize,
    pub file_count: usize,
    pub excluded_sensitive: usize,
    pub reference_count: usize,
    pub groups: Vec<GroupSummary>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportResult {
    pub path: String,
    pub item_count: usize,
    pub file_count: usize,
}

impl Core {
    /// 按导出选项统计记录数、分组与输出文件数，并给出本次预览的指纹。只读，不改任何记录。
    pub async fn preview_readable_export(&self, options: ExportOptions) -> Result<ExportPreview> {
        let core = self.clone();
        self.hop(async move {
            let pool = core.0.db.pool().await;
            preview(&pool, &options, core.0.language()).await
        })
        .await
    }

    /// 指纹与预览一致时生成 Excel / Markdown。`target` 是单个文件路径（扩展名与格式一致），
    /// 按分组拆分时是目标目录（在其中新建 `KwikPaste-Export-<时间>-<uuid>` 子目录）；必须是绝对路径。
    pub async fn export_readable_data(
        &self,
        options: ExportOptions,
        fingerprint: String,
        target: PathBuf,
    ) -> Result<ExportResult> {
        let core = self.clone();
        self.hop(async move {
            let pool = core.0.db.pool().await;
            export(
                &pool,
                options,
                fingerprint,
                target.to_string_lossy().into_owned(),
                core.0.language(),
            )
            .await
        })
        .await
    }
}

#[derive(Debug, Clone, Serialize, FromRow)]
struct ExportRow {
    id: String,
    group_id: Option<String>,
    group_name: Option<String>,
    group_order: i64,
    kind: String,
    sub_kind: Option<String>,
    content: String,
    note: Option<String>,
    created_at: String,
    is_favorite: bool,
    is_sensitive: bool,
}

struct Snapshot {
    preview: ExportPreview,
    groups: BTreeMap<(i64, String), (String, Vec<ExportRow>)>,
}

/// 用相同筛选读取稳定快照，预览指纹包含选项、内容及元数据，避免确认后静默导出变化的数据。
async fn snapshot(pool: &SqlitePool, options: &ExportOptions, lang: Language) -> Result<Snapshot> {
    if options.group_ids.as_ref().is_some_and(|ids| ids.is_empty()) && !options.include_ungrouped {
        return Err(anyhow!(label(lang, Key::ExportNoGroups)).into());
    }

    let rows = sqlx::query_as::<_, ExportRow>(
        "SELECT i.id, i.group_id, g.name AS group_name, COALESCE(g.sort_order, -1) AS group_order, i.kind, i.sub_kind, \
         i.content, i.note, i.created_at, i.is_favorite, i.is_sensitive \
         FROM clipboard_items i LEFT JOIN clipboard_groups g ON g.id = i.group_id \
         WHERE (? = 0 OR i.is_favorite = 1) \
         ORDER BY g.sort_order, i.group_id, i.created_at, i.id",
    )
    .bind(options.favorites_only)
    .fetch_all(pool)
    .await
    .context("read readable-export snapshot")?;

    let mut groups: BTreeMap<(i64, String), (String, Vec<ExportRow>)> = BTreeMap::new();
    let mut excluded_sensitive: usize = 0;
    let mut reference_count = 0;
    let mut hasher = blake3::Hasher::new();
    hasher.update(&serde_json::to_vec(options).context("serialize export options")?);
    hasher.update(match lang {
        Language::ZhCN => b"zh-CN",
        Language::EnUS => b"en-US",
    });

    for row in rows {
        if let Some(ids) = &options.group_ids {
            let included = match &row.group_id {
                Some(id) => ids.contains(id),
                None => options.include_ungrouped,
            };
            if !included {
                continue;
            }
        }
        if row.is_sensitive && !options.include_sensitive {
            excluded_sensitive += 1;
            continue;
        }
        reference_count += usize::from(row.kind != "text");
        hasher.update(&serde_json::to_vec(&row).context("serialize export row")?);
        let name = row
            .group_name
            .clone()
            .unwrap_or_else(|| words(lang).ungrouped.to_owned());
        groups
            .entry((row.group_order, row.group_id.clone().unwrap_or_default()))
            .or_insert_with(|| (name, Vec::new()))
            .1
            .push(row);
    }

    hasher.update(&excluded_sensitive.to_le_bytes());
    let item_count = groups.values().map(|(_, rows)| rows.len()).sum();
    let file_count = if item_count == 0 {
        0
    } else if options.split_by_group {
        groups.len()
    } else {
        1
    };
    let summary = groups
        .iter()
        .map(|((_, id), (name, rows))| GroupSummary {
            id: id.clone(),
            name: name.clone(),
            count: rows.len(),
        })
        .collect();
    Ok(Snapshot {
        preview: ExportPreview {
            fingerprint: hasher.finalize().to_hex().to_string(),
            item_count,
            file_count,
            excluded_sensitive,
            reference_count,
            groups: summary,
        },
        groups,
    })
}

/// 只返回数量与分组摘要，不向前端传输正文或修改任何原始记录。
pub(crate) async fn preview(
    pool: &SqlitePool,
    options: &ExportOptions,
    lang: Language,
) -> Result<ExportPreview> {
    Ok(snapshot(pool, options, lang).await?.preview)
}

/// 重新校验预览，在线程池生成文件；单文件原子写入，拆分文件在临时目录完整生成后交付。
pub(crate) async fn export(
    pool: &SqlitePool,
    options: ExportOptions,
    fingerprint: String,
    target: String,
    lang: Language,
) -> Result<ExportResult> {
    let snapshot = snapshot(pool, &options, lang).await?;
    if snapshot.preview.fingerprint != fingerprint {
        return Err(anyhow!(label(lang, Key::ExportPreviewChanged)).into());
    }
    if snapshot.preview.item_count == 0 {
        return Err(anyhow!(label(lang, Key::ExportEmpty)).into());
    }
    tokio::task::spawn_blocking(move || {
        write_snapshot(snapshot, options, PathBuf::from(target), lang)
    })
    .await
    .context("join readable export worker")?
}

/// 文件写入只作用于用户选择的导出位置，始终不访问或复制记录引用的图片 / 文件资源。
fn write_snapshot(
    snapshot: Snapshot,
    options: ExportOptions,
    target: PathBuf,
    lang: Language,
) -> Result<ExportResult> {
    if !target.is_absolute() {
        return Err(anyhow!(label(lang, Key::ExportInvalidTarget)).into());
    }
    let output = if options.split_by_group {
        if !target.is_dir() {
            return Err(anyhow!(label(lang, Key::ExportInvalidTarget)).into());
        }
        let staging = tempfile::Builder::new()
            .prefix(".kwikpaste-export-")
            .tempdir_in(&target)
            .context("create export staging directory")?;
        for (index, (name, rows)) in snapshot.groups.values().enumerate() {
            let filename = format!(
                "{:03}-{}.{}",
                index + 1,
                safe_filename(name),
                options.format.extension()
            );
            write_atomic(
                &staging.path().join(filename),
                &render(rows, options.format, lang)?,
            )?;
        }
        let folder = format!(
            "KwikPaste-Export-{}-{}",
            chrono::Local::now().format("%Y%m%d-%H%M%S"),
            uuid::Uuid::new_v4()
        );
        let output = target.join(folder);
        fs::rename(staging.path(), &output)
            .context("publish complete readable export directory")?;
        output
    } else {
        if target.extension().and_then(|s| s.to_str()) != Some(options.format.extension())
            || !target.parent().is_some_and(Path::is_dir)
        {
            return Err(anyhow!(label(lang, Key::ExportInvalidTarget)).into());
        }
        let rows: Vec<_> = snapshot
            .groups
            .values()
            .flat_map(|(_, rows)| rows.iter().cloned())
            .collect();
        write_atomic(&target, &render(&rows, options.format, lang)?)?;
        target
    };
    Ok(ExportResult {
        path: output.to_string_lossy().into_owned(),
        item_count: snapshot.preview.item_count,
        file_count: snapshot.preview.file_count,
    })
}

/// 与目标同盘创建临时文件，内容生成 / 写入失败时不覆盖已有输出。
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = NamedTempFile::new_in(path.parent().context("missing export parent")?)
        .context("create export temporary file")?;
    file.write_all(bytes).context("write readable export")?;
    file.as_file().sync_all().context("flush readable export")?;
    file.persist(path)
        .map_err(|e| anyhow!(e.error))
        .context("publish readable export file")?;
    Ok(())
}

/// 编号保证分组同名 / 清洗后同名仍唯一；限制长度并屏蔽 Windows 文件名与路径控制字符。
fn safe_filename(name: &str) -> String {
    let name: String = name
        .chars()
        .take(60)
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let name = name.trim_matches([' ', '.']);
    if name.is_empty() {
        "group".to_owned()
    } else {
        name.to_owned()
    }
}

/// 所有单元格显式写为字符串，公式样式内容不会执行；Excel 超限会明确失败而非截断。
fn render(rows: &[ExportRow], format: ExportFormat, lang: Language) -> Result<Vec<u8>> {
    let w = words(lang);
    if format == ExportFormat::Markdown {
        return Ok(render_markdown(rows, &w, lang).into_bytes());
    }
    if rows.len() >= 1_048_576 {
        return Err(anyhow!(label(lang, Key::ExportExcelLimit)).into());
    }
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    sheet
        .set_name("KwikPaste")
        .context("name readable-export worksheet")?;
    let header = Format::new().set_bold();
    let body = Format::new().set_text_wrap();
    for (col, title) in w.headers.iter().enumerate() {
        sheet
            .write_string_with_format(0, col as u16, *title, &header)
            .context("write export headers")?;
    }
    sheet
        .set_column_width(0, 20.0)
        .context("set group column width")?;
    sheet
        .set_column_width(1, 12.0)
        .context("set type column width")?;
    sheet
        .set_column_width(2, 70.0)
        .context("set content column width")?;
    sheet
        .set_column_width(3, 30.0)
        .context("set note column width")?;
    sheet
        .set_column_width(4, 28.0)
        .context("set timestamp column width")?;
    sheet
        .set_freeze_panes(1, 0)
        .context("freeze export headers")?;
    for (index, row) in rows.iter().enumerate() {
        let created_at = display_time(&row.created_at);
        let cells = [
            row.group_name.as_deref().unwrap_or(w.ungrouped),
            type_label(lang, &row.kind, row.sub_kind.as_deref()),
            &row.content,
            row.note.as_deref().unwrap_or(""),
            &created_at,
            if row.is_favorite { w.yes } else { w.no },
        ];
        for (col, value) in cells.iter().enumerate() {
            if value.encode_utf16().count() > 32_767 {
                return Err(anyhow!(label(lang, Key::ExportExcelLimit)).into());
            }
            sheet
                .write_string_with_format((index + 1) as u32, col as u16, *value, &body)
                .context("write readable-export cell")?;
        }
    }
    sheet
        .autofilter(0, 0, rows.len() as u32, 5)
        .context("filter readable-export worksheet")?;
    let info = workbook.add_worksheet();
    info.set_name("Info")
        .context("name export information sheet")?;
    info.write_string(0, 0, w.warning)
        .context("write export information")?;
    workbook
        .save_to_buffer()
        .context("encode readable-export workbook")
        .map_err(Into::into)
}

/// 数据库里的 RFC 3339 时间转成本机时区的 `YYYY-MM-DD HH:MM:SS`；解析不了的原样输出。
fn display_time(raw: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
        .unwrap_or_else(|_| raw.to_owned())
}

/// 使用比正文中任何反引号串更长的围栏，保留空白、换行、代码和 HTML，避免意外渲染成链接或脚本。
fn fenced(value: &str) -> String {
    let mut run = 0;
    let mut longest = 0;
    for c in value.chars() {
        run = if c == '`' { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    let fence = "`".repeat(3.max(longest + 1));
    format!("{fence}text\n{value}\n{fence}\n")
}

/// 分组用标题组织，正文和备注用字面量围栏呈现；图片 / 文件仅输出数据库原有引用，不读取资源。
fn render_markdown(rows: &[ExportRow], w: &Words, lang: Language) -> String {
    let mut out = format!("# {}\n\n{}\n\n", w.title, w.warning);
    let mut previous: Option<Option<&str>> = None;
    for (index, row) in rows.iter().enumerate() {
        let group = row.group_name.as_deref().unwrap_or(w.ungrouped);
        if previous != Some(row.group_id.as_deref()) {
            out.push_str(&format!("## {}\n\n", markdown_heading(group)));
            previous = Some(row.group_id.as_deref());
        }
        out.push_str(&format!(
            "### {}\n\n{}: {} · {}: {} · {}: {}\n\n",
            index + 1,
            w.headers[1],
            type_label(lang, &row.kind, row.sub_kind.as_deref()),
            w.headers[4],
            display_time(&row.created_at),
            w.headers[5],
            if row.is_favorite { w.yes } else { w.no }
        ));
        out.push_str(&fenced(&row.content));
        out.push('\n');
        if let Some(note) = &row.note {
            out.push_str(&format!("{}:\n\n{}\n", w.headers[3], fenced(note)));
        }
    }
    out
}

/// 分组名称只能作为单行标题文本，转义 Markdown 控制字符而不影响正文。
fn markdown_heading(value: &str) -> String {
    value
        .chars()
        .flat_map(|c| {
            if c.is_control() {
                vec![' ']
            } else if "\\`*_{}[]<>!#|".contains(c) {
                vec!['\\', c]
            } else {
                vec![c]
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::test_support::memory_pool;

    fn options(format: ExportFormat, split: bool) -> ExportOptions {
        ExportOptions {
            format,
            favorites_only: false,
            group_ids: None,
            include_ungrouped: true,
            split_by_group: split,
            include_sensitive: false,
        }
    }

    async fn fixture() -> SqlitePool {
        let pool = memory_pool().await;
        for (id, name) in [("a", "工作/代码"), ("b", "工作\\代码"), ("empty", "空分组")]
        {
            sqlx::query("INSERT INTO clipboard_groups (id,name,icon,sort_order,created_at,updated_at) VALUES (?,?,'folder',0,'2026-01-01','2026-01-01')").bind(id).bind(name).execute(&pool).await.unwrap();
        }
        for (id, group, kind, content, favorite, sensitive) in [
            (
                "one",
                Some("a"),
                "text",
                "  =SUM(1,2)\r\n```\n  原样  ",
                true,
                false,
            ),
            (
                "two",
                Some("b"),
                "text",
                "=HYPERLINK(\"https://example.invalid\", \"链接\")\n正文\n\n保留换行",
                false,
                false,
            ),
            ("secret", Some("a"), "text", "fixture-sensitive", true, true),
            ("image", None, "image", "images/missing.png", true, false),
        ] {
            sqlx::query("INSERT INTO clipboard_items (id,group_id,kind,content,content_hash,is_favorite,is_sensitive,platform,note,created_at,updated_at) VALUES (?,?,?,?,?,?,?,'windows','  备注  ','2026-01-01','2026-01-02')").bind(id).bind(group).bind(kind).bind(content).bind(id).bind(favorite).bind(sensitive).execute(&pool).await.unwrap();
        }
        pool
    }

    /// 同一数据覆盖格式、范围、分组与输出方式的全部组合，导出前后数据库逐字一致。
    #[tokio::test]
    async fn export_matrix_preserves_database() {
        let pool = fixture().await;
        let before: Vec<(String, String, String, bool, String)> = sqlx::query_as(
            "SELECT id,content,note,is_favorite,updated_at FROM clipboard_items ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let root = std::env::var("KWIKPASTE_EXPORT_FIXTURE_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|_| temp.path().to_path_buf());
        fs::create_dir_all(&root).unwrap();
        for format in [ExportFormat::Xlsx, ExportFormat::Markdown] {
            for split in [false, true] {
                for favorites in [false, true] {
                    for selected in [false, true] {
                        let mut o = options(format, split);
                        o.favorites_only = favorites;
                        if selected {
                            o.group_ids = Some(vec!["a".into()]);
                            o.include_ungrouped = false;
                        }
                        let p = preview(&pool, &o, Language::ZhCN).await.unwrap();
                        let expected = if selected {
                            1
                        } else if favorites {
                            2
                        } else {
                            3
                        };
                        assert_eq!(p.item_count, expected);
                        assert_eq!(p.excluded_sensitive, 1);
                        assert_eq!(p.file_count, if split { expected } else { 1 });
                        let label = format!(
                            "{}-split{split}-favorites{favorites}-selected{selected}",
                            format.extension()
                        );
                        let target = if split {
                            root.clone()
                        } else {
                            root.join(format!("{label}.{}", format.extension()))
                        };
                        let result = export(
                            &pool,
                            o,
                            p.fingerprint,
                            target.to_string_lossy().into_owned(),
                            Language::ZhCN,
                        )
                        .await
                        .unwrap();
                        assert!(Path::new(&result.path).exists());
                        println!(
                            "EXPORTED {label}: records={} files={} path={}",
                            result.item_count, result.file_count, result.path
                        );
                        if split {
                            let files: Vec<_> = fs::read_dir(&result.path)
                                .unwrap()
                                .map(|e| e.unwrap().file_name())
                                .collect();
                            assert_eq!(files.len(), expected);
                            assert!(files
                                .iter()
                                .all(|name| !name.to_string_lossy().contains("空分组")));
                        }
                    }
                }
            }
        }
        let after: Vec<(String, String, String, bool, String)> = sqlx::query_as(
            "SELECT id,content,note,is_favorite,updated_at FROM clipboard_items ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(before, after);
        println!("DATABASE_UNCHANGED: content/note/favorite/updated_at");
    }

    #[tokio::test]
    async fn sensitive_opt_in_and_ungrouped_only() {
        let pool = fixture().await;
        let mut o = options(ExportFormat::Markdown, true);
        o.include_sensitive = true;
        assert_eq!(
            preview(&pool, &o, Language::EnUS).await.unwrap().item_count,
            4
        );
        o.group_ids = Some(vec![]);
        let p = preview(&pool, &o, Language::EnUS).await.unwrap();
        assert_eq!(p.item_count, 1);
        assert_eq!(p.groups[0].name, "Ungrouped");
        o.include_ungrouped = false;
        assert!(preview(&pool, &o, Language::EnUS).await.is_err());
    }

    #[tokio::test]
    async fn stale_preview_empty_and_invalid_target_do_not_write() {
        let pool = fixture().await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unchanged.md");
        fs::write(&path, "existing-file").unwrap();
        let o = options(ExportFormat::Markdown, false);
        let p = preview(&pool, &o, Language::ZhCN).await.unwrap();
        sqlx::query("UPDATE clipboard_items SET note='changed' WHERE id='one'")
            .execute(&pool)
            .await
            .unwrap();
        assert!(export(
            &pool,
            o.clone(),
            p.fingerprint,
            path.to_string_lossy().into_owned(),
            Language::ZhCN
        )
        .await
        .is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "existing-file");
        let p = preview(&pool, &o, Language::ZhCN).await.unwrap();
        assert!(export(
            &pool,
            o.clone(),
            p.fingerprint,
            "relative.md".into(),
            Language::ZhCN
        )
        .await
        .is_err());
        let mut empty = o;
        empty.group_ids = Some(vec!["empty".into()]);
        empty.include_ungrouped = false;
        let p = preview(&pool, &empty, Language::ZhCN).await.unwrap();
        assert_eq!(p.file_count, 0);
        assert!(export(
            &pool,
            empty,
            p.fingerprint,
            path.to_string_lossy().into_owned(),
            Language::ZhCN
        )
        .await
        .is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "existing-file");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        println!("ERROR_PATHS: stale/empty/relative rejected; existing output unchanged");
    }

    #[tokio::test]
    async fn excel_limit_failure_keeps_destination_and_cleans_split_staging() {
        let pool = fixture().await;
        sqlx::query("UPDATE clipboard_items SET content=? WHERE id='two'")
            .bind("长".repeat(32_768))
            .execute(&pool)
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unchanged.xlsx");
        fs::write(&path, "existing-file").unwrap();
        for split in [false, true] {
            let o = options(ExportFormat::Xlsx, split);
            let p = preview(&pool, &o, Language::ZhCN).await.unwrap();
            let target = if split { dir.path() } else { &path };
            let error = export(
                &pool,
                o,
                p.fingerprint,
                target.to_string_lossy().into_owned(),
                Language::ZhCN,
            )
            .await
            .unwrap_err();
            assert!(error.to_string().contains("Excel"));
            assert_eq!(fs::read(&path).unwrap(), b"existing-file");
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        }
        println!(
            "EXCEL_LIMIT: rejected without truncation; staging cleaned; destination unchanged"
        );
    }

    #[test]
    fn literal_markdown_and_safe_names() {
        let value = "  =1+1\r\n```\n````\n尾部  ";
        assert_eq!(fenced(value), format!("`````text\n{value}\n`````\n"));
        assert_eq!(safe_filename("../CON:*<>|?\\/\n"), "_CON_________");
        assert!(!markdown_heading("<script>\n#x").contains('\n'));
    }

    #[tokio::test]
    async fn type_column_uses_localized_labels() {
        let pool = fixture().await;
        let o = options(ExportFormat::Markdown, false);
        let snapshot = snapshot(&pool, &o, Language::ZhCN).await.unwrap();
        let rows: Vec<_> = snapshot
            .groups
            .values()
            .flat_map(|(_, rows)| rows.iter().cloned())
            .collect();
        let zh = render_markdown(&rows, &words(Language::ZhCN), Language::ZhCN);
        assert!(zh.contains("类型: 图片") && zh.contains("类型: 文本"));
        let en = render_markdown(&rows, &words(Language::EnUS), Language::EnUS);
        assert!(en.contains("Type: Image") && en.contains("Type: Text"));
        assert_eq!(type_label(Language::ZhCN, "text", Some("url")), "链接");
        assert_eq!(type_label(Language::EnUS, "future", None), "future");
    }

    /// 宿主经 `Core` 调用：语言取当前设置，指纹在两次调用之间保持一致。
    #[test]
    fn core_facade_previews_and_exports_in_the_settings_language() {
        use crate::testing::{block_on, Fixture};

        let fixture = Fixture::new();
        let core = fixture.start();
        block_on(core.update_settings(serde_json::json!({"appearance": {"language": "en-US"}})))
            .unwrap();
        let clipboard =
            crate::clipboard::MemoryClipboard::with_state(crate::clipboard::MemoryState {
                text: Some("exported through the facade".to_owned()),
                ..Default::default()
            });
        let item = core
            .build_item(&core.read_payload(&clipboard).unwrap().unwrap())
            .unwrap()
            .unwrap();
        block_on(core.store_item(item, None)).unwrap();

        let o = options(ExportFormat::Markdown, false);
        let p = block_on(core.preview_readable_export(o.clone())).unwrap();
        assert_eq!(p.item_count, 1);
        assert_eq!(p.groups[0].name, "Ungrouped");

        let target = fixture.root().join("history.md");
        let result =
            block_on(core.export_readable_data(o.clone(), p.fingerprint, target.clone())).unwrap();
        assert_eq!(result.item_count, 1);
        let markdown = fs::read_to_string(&target).unwrap();
        assert!(markdown.contains("exported through the facade"));
        assert!(markdown.contains("Type: Text"));

        let stale = block_on(core.export_readable_data(o, "stale".to_owned(), target)).unwrap_err();
        assert_eq!(
            stale.to_string(),
            label(Language::EnUS, Key::ExportPreviewChanged)
        );
        block_on(core.shutdown()).unwrap();
    }

    #[test]
    fn created_time_is_local_and_readable() {
        let raw = "2026-10-02T02:00:00.000000000+00:00";
        let expected = chrono::DateTime::parse_from_rfc3339(raw)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M:%S")
            .to_string();
        assert_eq!(display_time(raw), expected);
        assert_eq!(display_time(raw).len(), 19);
        assert_eq!(display_time("2026-01-01"), "2026-01-01");
    }
}
