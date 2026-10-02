//! OS 级剪贴板监听：把 clipboard-rs 的 watcher 接到「读取 → 去重入库 → 通知宿主」闭环。
//!
//! clipboard-rs 已实现 macOS（`NSPasteboard.changeCount` 轮询）/ Windows
//! （`AddClipboardFormatListener` → `WM_CLIPBOARDUPDATE`）的平台监听，这里不重复造。
//!
//! 线程模型：`ClipboardWatcherContext::start_watch()` 是阻塞调用，故整个监听跑在独立
//! `std::thread` 上。系统剪贴板句柄**在该线程内构造**，不跨线程移动；只有 `Send` 的数据
//! （记录、来源应用）会被投递到 core runtime 做入库与通知。
//!
//! 一次变化的处理拆成可测试的 [`capture_change`]：自动测试用内存剪贴板驱动它，监听线程本身只能真机验证。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use anyhow::anyhow;
use clipboard_rs::{ClipboardHandler, ClipboardWatcher, ClipboardWatcherContext, WatcherShutdown};

use super::apps_registry::materialize_source;
use super::backend::{ClipboardBackend, SystemClipboard};
use super::ingest::build_item_with_settings;
use super::read::ClipboardReader;
use crate::db::models::{ClipboardApp, ClipboardItem};
use crate::error::{AppError, Result};
use crate::root::CoreInner;

/// macOS 轮询 `changeCount` 的间隔。clipboard-rs 默认 500ms，对复制响应（尤其图片）偏慢；
/// 120ms 跟手且 CPU 开销可忽略。Windows 走事件驱动（`WM_CLIPBOARDUPDATE`），此值被忽略。
const CLIPBOARD_POLL_INTERVAL: Duration = Duration::from_millis(120);

/// 别的剪贴板监听程序可能短暂占着 Windows 剪贴板，读取失败时在有界时间内重试。
const CLIPBOARD_READ_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_millis(15),
    Duration::from_millis(35),
    Duration::from_millis(75),
];

fn read_with_retry<T, E>(
    retry_delays: &[Duration],
    mut read: impl FnMut() -> std::result::Result<Option<T>, E>,
) -> std::result::Result<Option<T>, E> {
    let mut result = read();
    for delay in retry_delays {
        if result.is_ok() {
            return result;
        }
        std::thread::sleep(*delay);
        result = read();
    }
    result
}

/// 监听暂停开关。切换存储位置、覆盖导入备份期间置位，回调早返回跳过整条入库链路。
/// 不停 watcher 线程本身，避免反复重建平台句柄。
#[derive(Debug, Default, Clone)]
pub struct WatcherPause(Arc<AtomicBool>);

impl WatcherPause {
    pub fn is_paused(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    pub fn set_paused(&self, paused: bool) {
        self.0.store(paused, Ordering::Relaxed);
    }
}

/// 处理一次剪贴板变化的同步部分：识别来源 → 过滤忽略的应用 → 读取（带重试）→ 转成记录
/// （图片在这里落盘）→ 自身写回则跳过 → 补齐来源应用。返回 `None` 表示这次不入库。
///
/// **先**抓前台应用：等异步入库再问，前台早就切回别的窗口了。自身写回的事件会在 guard 处丢弃，
/// 但顺序换不得：guard 判定依赖 content_hash，必须先把内容读出来才能判，而读取期间用户可能已经切走前台。
pub(crate) fn capture_change<B: ClipboardBackend>(
    core: &CoreInner,
    reader: &ClipboardReader<B>,
    retry_delays: &[Duration],
) -> Option<(ClipboardItem, Option<ClipboardApp>)> {
    if core.watcher_pause.is_paused() {
        return None;
    }

    let source = core.platform().frontmost_app();
    let settings = core.settings.snapshot();

    // 用户在偏好里勾选了「过滤此应用」时整条丢弃，省掉无效的读取与图片解码。
    if let Some(src) = &source {
        if settings
            .clipboard
            .filters
            .excluded_app_ids
            .iter()
            .any(|id| id == &src.id)
        {
            return None;
        }
    }

    let payload = match read_with_retry(retry_delays, || {
        reader.read_with_capture(&settings.clipboard.capture)
    }) {
        Ok(Some(payload)) => payload,
        Ok(None) => return None,
        Err(err) => {
            log::warn!("clipboard watcher: read failed: {err}");
            return None;
        }
    };

    let mut item = match build_item_with_settings(
        &core.images,
        &payload,
        &settings.clipboard.capture,
        &settings.clipboard.sensitive,
        settings.clipboard.content.copy_plain,
    ) {
        Ok(Some(item)) => item,
        Ok(None) => return None,
        Err(err) => {
            log::warn!("clipboard watcher: build item failed: {err}");
            return None;
        }
    };

    // 自身写回触发的变更：跳过入库，避免回环。
    if core.guard.should_skip(&item.content_hash) {
        return None;
    }

    let source_app = source.map(|src| materialize_source(&core.app_icons, Some(&core.apps), src));
    if let Some(src) = &source_app {
        item.source_app_id = Some(src.id.clone());
    }

    Some((item, source_app))
}

/// 把 [`capture_change`] 的结果交给 core runtime 入库并通知宿主；失败只记日志（监听场景无人接收结果）。
fn persist_captured(core: Arc<CoreInner>, item: ClipboardItem, source_app: Option<ClipboardApp>) {
    let rt = core.rt.clone();
    rt.spawn(async move {
        if let Err(err) =
            super::persist::persist_and_notify(&core, &item, source_app.as_ref()).await
        {
            log::error!("clipboard watcher: persist failed: {err}");
        }
    });
}

/// 在独立线程上启动 OS 级监听，返回停止句柄：句柄被丢弃时监听线程退出。
/// 平台句柄创建失败时返回错误，不会留下半启动的线程。
pub(crate) fn spawn(core: &Arc<CoreInner>) -> Result<WatcherShutdown> {
    let weak = Arc::downgrade(core);
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();

    std::thread::Builder::new()
        .name("clipboard-watcher".to_owned())
        .spawn(move || {
            // 平台剪贴板句柄在本线程内构造，不跨线程移动。
            let reader = match SystemClipboard::new() {
                Ok(backend) => ClipboardReader::with_backend(backend),
                Err(err) => {
                    let _ = ready_tx.send(Err(err));
                    return;
                }
            };
            let mut watcher =
                match ClipboardWatcherContext::new_with_interval(CLIPBOARD_POLL_INTERVAL) {
                    Ok(watcher) => watcher,
                    Err(err) => {
                        let _ = ready_tx.send(Err(AppError::Clipboard(err.to_string())));
                        return;
                    }
                };

            watcher.add_handler(ClipboardChangeHandler { reader, core: weak });
            if ready_tx.send(Ok(watcher.get_shutdown_channel())).is_err() {
                return;
            }

            log::info!("clipboard watcher started");
            // 阻塞直至停止句柄被丢弃。
            watcher.start_watch();
            log::info!("clipboard watcher stopped");
        })
        .map_err(|err| AppError::Other(anyhow!("failed to spawn clipboard watcher: {err}")))?;

    ready_rx
        .recv()
        .map_err(|_| AppError::Other(anyhow!("clipboard watcher exited before starting")))?
}

struct ClipboardChangeHandler {
    reader: ClipboardReader<SystemClipboard>,
    core: Weak<CoreInner>,
}

impl ClipboardHandler for ClipboardChangeHandler {
    fn on_clipboard_change(&mut self) {
        let Some(core) = self.core.upgrade() else {
            return;
        };

        if let Some((item, source_app)) =
            capture_change(&core, &self.reader, &CLIPBOARD_READ_RETRY_DELAYS)
        {
            persist_captured(core, item, source_app);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    const ZERO_DELAY_RETRIES: [Duration; 3] = [Duration::ZERO; 3];

    #[test]
    fn clipboard_read_retry_returns_immediate_success() {
        let attempts = Cell::new(0);

        let result = read_with_retry(&ZERO_DELAY_RETRIES, || {
            attempts.set(attempts.get() + 1);
            Ok::<_, &'static str>(Some("captured"))
        });

        assert_eq!(result, Ok(Some("captured")));
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn clipboard_read_retry_recovers_after_transient_error() {
        let attempts = Cell::new(0);

        let result = read_with_retry(&ZERO_DELAY_RETRIES, || {
            attempts.set(attempts.get() + 1);
            if attempts.get() == 1 {
                Err("clipboard busy")
            } else {
                Ok(Some("captured"))
            }
        });

        assert_eq!(result, Ok(Some("captured")));
        assert_eq!(attempts.get(), 2);
    }

    #[test]
    fn clipboard_read_retry_does_not_retry_empty_content() {
        let attempts = Cell::new(0);

        let result = read_with_retry(&ZERO_DELAY_RETRIES, || {
            attempts.set(attempts.get() + 1);
            Ok::<Option<&'static str>, &'static str>(None)
        });

        assert_eq!(result, Ok(None));
        assert_eq!(attempts.get(), 1);
    }

    #[test]
    fn clipboard_read_retry_returns_final_error_after_exhaustion() {
        let attempts = Cell::new(0);

        let result = read_with_retry(&ZERO_DELAY_RETRIES, || {
            attempts.set(attempts.get() + 1);
            Err::<Option<&'static str>, _>(attempts.get())
        });

        assert_eq!(result, Err(4));
        assert_eq!(attempts.get(), 4);
    }
}
