//! 记录与线上格式互转、按水位线补齐，以及收到记录后的入库 / 写剪贴板。
//!
//! 收到的内容先还原成剪贴板载荷，再走本机采集同一条 `build_item_with_settings`：
//! 接收方自己的采集类型、大小上限和敏感内容规则照样生效，去重指纹也与本机复制一致。

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context};
use chrono::Utc;

use super::protocol::{WireItem, MAX_HEADER_BYTES};
use crate::clipboard::{
    build_item_with_settings, png_dimensions, write_to_clipboard, ClipboardPayload, ImagePayload,
    TextPayload,
};
use crate::db::models::{ClipboardItem, ClipboardKind, ClipboardSubKind, Platform};
use crate::root::CoreInner;
use crate::settings::LanSync;

/// 补齐最多回看多久、总共最多补多少条、每页多少条。
const CATCH_UP_DAYS: i64 = 7;
const CATCH_UP_TOTAL: i64 = 500;
const CATCH_UP_PAGE: i64 = 50;
/// 文本放在 JSON 头里，给其余字段留出余量。
const MAX_TEXT_BYTES: usize = MAX_HEADER_BYTES - 64 * 1024;
/// 最近一次同步过的内容在这段时间内视为回声。
const ECHO_WINDOW: Duration = Duration::from_secs(60);

/// 回声过滤：记住最近一次同步过（发出或实时收到）的内容。
///
/// 和它相同的内容既不再发出，也不再写回剪贴板。一次复制触发好几次剪贴板事件、
/// 本机写回又被监听到、远程桌面 / 虚拟机的剪贴板共享把内容带回来，都会产生这种回声；
/// 只按时间窗口去重挡不住延迟较长的共享，两台设备会一直互相写。
/// 期间同步过别的内容就不再算回声，再复制回原来的内容照常同步。
#[derive(Default)]
pub struct EchoFilter {
    last: Option<(String, Instant)>,
}

impl EchoFilter {
    /// 本机这次复制要不要发出去；要发时记为最近同步的内容。
    pub fn should_send(&mut self, content_hash: &str) -> bool {
        if self.is_echo(content_hash) {
            return false;
        }

        self.remember(content_hash);
        true
    }

    /// 实时收到一条记录并记为最近同步的内容；返回要不要写入剪贴板。
    pub fn accept_received(&mut self, content_hash: &str) -> bool {
        let fresh = !self.is_echo(content_hash);
        self.remember(content_hash);
        fresh
    }

    fn is_echo(&self, content_hash: &str) -> bool {
        self.last
            .as_ref()
            .is_some_and(|(hash, at)| hash == content_hash && at.elapsed() <= ECHO_WINDOW)
    }

    fn remember(&mut self, content_hash: &str) {
        self.last = Some((content_hash.to_owned(), Instant::now()));
    }
}

/// 这条记录按当前设置是否参与同步。文件和敏感内容不同步。
pub fn is_syncable(item: &ClipboardItem, lan: &LanSync) -> bool {
    if item.is_sensitive {
        return false;
    }

    match item.kind {
        ClipboardKind::Text => lan.text,
        ClipboardKind::Image => lan.image,
        ClipboardKind::Files => false,
    }
}

/// 把本机记录转成线上格式；图片读原图作为附件。附件超过 `attachment_limit`（本机设置与
/// 对方申报的上限中较小的那个）或文本超过 JSON 头上限时返回 `None`。
pub async fn wire_from_item(
    core: &CoreInner,
    item: &ClipboardItem,
    attachment_limit: u64,
) -> anyhow::Result<Option<(WireItem, Vec<u8>)>> {
    match item.kind {
        ClipboardKind::Text => {
            let text_bytes = item.content.len()
                + item.search_text.as_ref().map_or(0, String::len)
                + item.summary.as_ref().map_or(0, String::len);
            if text_bytes > MAX_TEXT_BYTES {
                log::info!("lan sync skips text of {text_bytes} bytes over the message limit");
                return Ok(None);
            }
            Ok(Some((
                WireItem {
                    kind: item.kind,
                    sub_kind: item.sub_kind,
                    content: item.content.clone(),
                    search_text: item.search_text.clone(),
                    summary: item.summary.clone(),
                    width: None,
                    height: None,
                },
                Vec::new(),
            )))
        }
        ClipboardKind::Image => {
            let path = core.images.origin_path(&item.content);
            let bytes = tokio::task::spawn_blocking(move || std::fs::read(&path))
                .await
                .context("image read task failed")?
                .context("failed to read image for sync")?;
            if bytes.len() as u64 > attachment_limit {
                log::info!(
                    "lan sync skips image of {} bytes over the size limit",
                    bytes.len()
                );
                return Ok(None);
            }

            Ok(Some((
                WireItem {
                    kind: item.kind,
                    sub_kind: None,
                    content: String::new(),
                    search_text: None,
                    summary: None,
                    width: item.width,
                    height: item.height,
                },
                bytes,
            )))
        }
        ClipboardKind::Files => Ok(None),
    }
}

/// 一页补齐。
pub struct CatchUpPage {
    pub items: Vec<(WireItem, Vec<u8>)>,
    /// 这一页覆盖到的本机序号：对方收齐这页后把水位线推进到这里。
    pub until: u64,
    pub more: bool,
    /// 本机序号的代次。
    pub epoch: String,
}

/// 对方请求本机序号大于 `after` 的记录：最近 7 天、最新的 500 条以内，按序号从小到大分页。
/// 对方的水位线属于另一代序号（本机数据库重建过）或还没有水位线时从头发。
/// 最后一页的 `until` 是本机当前的计数器，不参与同步的记录（敏感、文件）也一并跳过。
pub async fn collect_catch_up(
    core: &CoreInner,
    after: u64,
    epoch: Option<&str>,
    attachment_limit: u64,
) -> anyhow::Result<CatchUpPage> {
    let lan = core.settings.snapshot().sync.lan;
    let pool = core.db.pool().await;
    let own_epoch = crate::db::sync::sync_epoch(&pool).await?;
    let after = if epoch == Some(own_epoch.as_str()) {
        after
    } else {
        0
    };
    // 先读计数器再取这一页：取页期间新复制的记录序号比它大，留到下一次补齐，不会被跳过。
    let counter = crate::db::sync::sync_counter(&pool).await?.max(0) as u64;
    let cutoff = Utc::now() - chrono::Duration::days(CATCH_UP_DAYS);
    let after_signed = i64::try_from(after).unwrap_or(i64::MAX);
    let (rows, more) =
        crate::db::sync::catch_up_page(&pool, after_signed, cutoff, CATCH_UP_TOTAL, CATCH_UP_PAGE)
            .await?;

    let last = rows.last().map_or(after, |(_, seq)| (*seq).max(0) as u64);
    let until = if more { last } else { counter.max(last) };
    let mut items = Vec::with_capacity(rows.len());
    for (id, _) in rows {
        let Some(item) = crate::db::items::find_item_by_id(&pool, &id).await? else {
            continue;
        };
        if !is_syncable(&item, &lan) {
            continue;
        }
        let limit = attachment_limit.min(lan.max_image_bytes());
        match wire_from_item(core, &item, limit).await {
            Ok(Some(wire)) => items.push(wire),
            Ok(None) => {}
            Err(err) => log::warn!("lan sync catch-up skips item {id}: {err:#}"),
        }
    }

    Ok(CatchUpPage {
        items,
        until,
        more,
        epoch: own_epoch,
    })
}

/// 入库一条收到的记录。`live` 且设置允许时写入系统剪贴板；补齐的历史只在本机还没有这条内容时
/// 插入，已有的不动（不刷新时间、不增加使用次数），重连后重复补到的记录不会把列表顺序打乱。
pub async fn receive(
    core: &Arc<CoreInner>,
    echo: &std::sync::Mutex<EchoFilter>,
    peer_id: &str,
    peer_platform: Platform,
    wire: WireItem,
    attachment: Vec<u8>,
    live: bool,
) -> anyhow::Result<()> {
    let settings = core.settings.snapshot();
    let lan = &settings.sync.lan;
    let payload = match wire.kind {
        ClipboardKind::Text if lan.text => text_payload(&wire)?,
        ClipboardKind::Image if lan.image => {
            if attachment.len() as u64 > lan.max_image_bytes() {
                log::info!("lan sync drops image over the size limit");
                return Ok(());
            }
            let (width, height) = png_dimensions(&attachment)
                .ok_or_else(|| anyhow!("received image is not a PNG"))?;
            ClipboardPayload::Image(ImagePayload {
                bytes: attachment,
                width,
                height,
            })
        }
        _ => return Ok(()),
    };

    let capture = settings.clipboard.capture.clone();
    let sensitive = settings.clipboard.sensitive;
    let plain_only = settings.clipboard.content.copy_plain;
    let built = {
        let core = core.clone();
        tokio::task::spawn_blocking(move || {
            build_item_with_settings(&core.images, &payload, &capture, &sensitive, plain_only)
        })
        .await
        .context("build received item task failed")??
    };
    let Some(mut item) = built else {
        return Ok(());
    };
    item.origin_device_id = Some(peer_id.to_owned());
    item.platform = peer_platform;

    if !live {
        let pool = core.db.pool().await;
        if crate::db::items::find_item_by_content_hash(&pool, &item.content_hash)
            .await?
            .is_some()
        {
            return Ok(());
        }
    }

    // 补齐的是历史记录，不代表对方剪贴板的现状，不参与回声判断，也不写剪贴板。
    // 实时记录先记回声再写剪贴板：写回被本机监听到时要能认出来。
    let write_clipboard = live
        && lan.write_clipboard
        && echo
            .lock()
            .expect("sync echo filter poisoned")
            .accept_received(&item.content_hash);

    crate::clipboard::persist::store_and_emit(core, &item).await?;

    if write_clipboard {
        let core = core.clone();
        tokio::task::spawn_blocking(move || {
            let written = core.clipboard().and_then(|backend| {
                write_to_clipboard(backend.as_ref(), &core.images, &core.guard, &item, false)
            });
            if let Err(err) = written {
                log::warn!("lan sync write clipboard failed: {err}");
            }
        });
    }

    Ok(())
}

/// 文本记录还原成剪贴板文本载荷：富文本带上 OS 同时提供的纯文本表示。
fn text_payload(wire: &WireItem) -> anyhow::Result<ClipboardPayload> {
    let rich = matches!(
        wire.sub_kind,
        Some(ClipboardSubKind::Html) | Some(ClipboardSubKind::Rtf)
    );
    let text = if rich {
        wire.search_text.clone().unwrap_or_default()
    } else {
        wire.content.clone()
    };
    if text.trim().is_empty() {
        return Err(anyhow!("received text is empty"));
    }

    Ok(ClipboardPayload::Text(TextPayload {
        text,
        html: (wire.sub_kind == Some(ClipboardSubKind::Html)).then(|| wire.content.clone()),
        rtf: (wire.sub_kind == Some(ClipboardSubKind::Rtf)).then(|| wire.content.clone()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(sub_kind: Option<ClipboardSubKind>, content: &str, search: Option<&str>) -> WireItem {
        WireItem {
            kind: ClipboardKind::Text,
            sub_kind,
            content: content.to_owned(),
            search_text: search.map(str::to_owned),
            summary: None,
            width: None,
            height: None,
        }
    }

    #[test]
    fn html_keeps_source_and_plain_text() {
        let payload =
            text_payload(&wire(Some(ClipboardSubKind::Html), "<b>hi</b>", Some("hi"))).unwrap();

        let ClipboardPayload::Text(text) = payload else {
            panic!("expected text payload");
        };
        assert_eq!(text.text, "hi");
        assert_eq!(text.html.as_deref(), Some("<b>hi</b>"));
        assert!(text.rtf.is_none());
    }

    #[test]
    fn plain_text_uses_content() {
        let payload =
            text_payload(&wire(Some(ClipboardSubKind::Url), "https://a.b", None)).unwrap();

        let ClipboardPayload::Text(text) = payload else {
            panic!("expected text payload");
        };
        assert_eq!(text.text, "https://a.b");
        assert!(text.html.is_none());
    }

    #[test]
    fn empty_text_is_rejected() {
        assert!(text_payload(&wire(None, "   ", None)).is_err());
    }

    #[test]
    fn received_content_is_not_sent_back() {
        let mut filter = EchoFilter::default();

        assert!(filter.accept_received("a"));
        assert!(!filter.should_send("a"));
    }

    #[test]
    fn repeated_events_for_one_copy_send_once() {
        let mut filter = EchoFilter::default();

        assert!(filter.should_send("a"));
        assert!(!filter.should_send("a"));
    }

    #[test]
    fn content_echoed_back_is_not_written_again() {
        let mut filter = EchoFilter::default();
        filter.should_send("a");

        assert!(!filter.accept_received("a"));
    }

    #[test]
    fn copying_back_after_other_content_syncs_again() {
        let mut filter = EchoFilter::default();
        filter.should_send("a");

        assert!(filter.accept_received("b"));
        assert!(filter.should_send("a"));
    }
}
