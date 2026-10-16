//! 普通启动的日志文件，与 1.x（tauri-plugin-log 2 的默认设置）相同：
//!
//! - 位置：`<日志目录>/KwikPaste.log`（`CorePaths::log_dir`：安装版 `%LOCALAPPDATA%\<id>\logs`，
//!   便携版 `data\logs`），追加写，1.x 留下的内容接着用；
//! - 大小与轮换：上限 40 000 字节，再写一行就会超过时轮换到 `KwikPaste.log.1`（覆盖上一份），打开时已经
//!   超过也一样；
//! - 格式：`[2026-10-03][04:05:06][target][LEVEL] message`，UTC；
//! - 级别：本 workspace 的 crate（应用、core、updater、os）记到 info，debug 构建到 debug；其余 crate
//!   只记 warn 以上，免得 GPUI 等库的日志把 40 KB 挤满。
//! - 普通日志有界排队，由单个后台线程写入；warn/error 与崩溃信息同步落盘。

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

const FILE_NAME: &str = "KwikPaste.log";
/// tauri-plugin-log 的 `DEFAULT_MAX_FILE_SIZE`。
const MAX_BYTES: u64 = 40_000;
const QUEUE_CAPACITY: usize = 256;
const FLUSH_TIMEOUT: Duration = Duration::from_millis(200);

struct FileLogger {
    path: PathBuf,
    sink: Mutex<Sink>,
    ordinary: SyncSender<Message>,
    dropped: AtomicUsize,
}

enum Message {
    Line(String),
    Flush(mpsc::Sender<()>),
}

struct Sink {
    file: Option<File>,
    size: u64,
}

static LOGGER: OnceLock<Arc<FileLogger>> = OnceLock::new();

/// 打开日志文件并设为全局 logger。目录建不了、文件打不开时什么也不做（应用照常运行）。
pub fn init(dir: PathBuf) {
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!(
            "log directory {} could not be created: {err}",
            dir.display()
        );
        return;
    }
    let logger = LOGGER.get_or_init(|| FileLogger::open(dir.join(FILE_NAME)));
    if log::set_logger(logger.as_ref()).is_ok() {
        log::set_max_level(if cfg!(debug_assertions) {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        });
    }
}

/// 仅保留上一份；改名失败时保留当前日志，下一次写入再尝试。
fn rotate(path: &Path) {
    let previous = path.with_file_name("KwikPaste.log.1");
    if let Err(err) = std::fs::remove_file(&previous)
        && err.kind() != std::io::ErrorKind::NotFound
    {
        eprintln!("previous log could not be replaced: {err}");
        return;
    }
    if let Err(err) = std::fs::rename(path, previous) {
        eprintln!("log could not be rotated: {err}");
    }
}

fn open_append(path: &Path) -> (Option<File>, u64) {
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(file) => {
            let size = file.metadata().map_or(0, |meta| meta.len());
            (Some(file), size)
        }
        Err(err) => {
            eprintln!("log file {} could not be opened: {err}", path.display());
            (None, 0)
        }
    }
}

/// 1.x 的时间戳格式（UTC）。
fn timestamp() -> String {
    chrono::Utc::now()
        .format("[%Y-%m-%d][%H:%M:%S]")
        .to_string()
}

fn line(target: &str, level: log::Level, message: &std::fmt::Arguments<'_>) -> String {
    format!("{}[{target}][{level}] {message}\n", timestamp())
}

impl FileLogger {
    /// 打开文件并启动一个持有 logger 的后台写入线程。
    fn open(path: PathBuf) -> Arc<Self> {
        let (file, size) = open_append(&path);
        let (file, size) = if size > MAX_BYTES {
            // Windows 改名前必须先关闭句柄。
            drop(file);
            rotate(&path);
            open_append(&path)
        } else {
            (file, size)
        };

        let (ordinary, pending) = mpsc::sync_channel(QUEUE_CAPACITY);
        let logger = Arc::new(Self {
            path,
            sink: Mutex::new(Sink { file, size }),
            ordinary,
            dropped: AtomicUsize::new(0),
        });
        let writer = logger.clone();
        if let Err(err) = std::thread::Builder::new()
            .name("log-writer".to_owned())
            .spawn(move || {
                while let Ok(message) = pending.recv() {
                    let Ok(mut sink) = writer.sink.lock() else {
                        break;
                    };
                    let dropped = writer.dropped.swap(0, Ordering::Relaxed);
                    if dropped != 0 {
                        writer.append(
                            &mut sink,
                            &line(
                                "KwikPaste::health",
                                log::Level::Warn,
                                &format_args!("dropped {dropped} ordinary log lines: queue full"),
                            ),
                        );
                    }
                    if let Message::Line(text) = &message {
                        writer.append(&mut sink, text);
                    }
                    drop(sink);
                    if let Message::Flush(done) = message {
                        let _ = done.send(());
                    }
                }
            })
        {
            let _ = writeln!(std::io::stderr(), "log writer could not be started: {err}");
        }
        logger
    }

    fn append(&self, sink: &mut Sink, text: &str) {
        let len = text.len() as u64;
        if sink.file.is_none() || (sink.size != 0 && sink.size + len > MAX_BYTES) {
            sink.file = None;
            if sink.size != 0 {
                rotate(&self.path);
            }
            (sink.file, sink.size) = open_append(&self.path);
        }
        if let Some(file) = &mut sink.file
            && file.write_all(text.as_bytes()).is_ok()
        {
            let _ = file.flush();
            sink.size += len;
        }
    }
}

impl log::Log for FileLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        // 二进制 crate 的日志 target 以 `KwikPaste::` 开头，库 crate 以 `kwikpaste_` 开头。
        let ours = metadata
            .target()
            .get(..9)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("kwikpaste"));
        let max = if !ours {
            log::Level::Warn
        } else if cfg!(debug_assertions) {
            log::Level::Debug
        } else {
            log::Level::Info
        };

        metadata.level() <= max
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let text = line(record.target(), record.level(), record.args());
        if record.level() > log::Level::Warn {
            if let Err(TrySendError::Full(_)) = self.ordinary.try_send(Message::Line(text)) {
                self.dropped.fetch_add(1, Ordering::Relaxed);
            }
            return;
        }
        if let Ok(mut sink) = self.sink.lock() {
            self.append(&mut sink, &text);
        }
    }

    /// 排入刷新标记，并在短超时内等待此前排队的行写完。
    fn flush(&self) {
        let (done, completed) = mpsc::channel();
        if self.ordinary.send(Message::Flush(done)).is_err() {
            return;
        }
        let _ = completed.recv_timeout(FLUSH_TIMEOUT);
    }
}

/// panic hook / 原生崩溃处理用：把崩溃信息写进日志并落盘（每行写完即 flush，release 是
/// `panic = "abort"` 时 hook 返回后进程立刻结束）。日志的锁拿不到（崩溃正发生在写日志时）就另开
/// 一个句柄追加，不会在 hook 里死锁。自测进程写 stderr。
pub fn write_crash(text: &str) {
    let text = line(
        "KwikPaste::health",
        log::Level::Error,
        &format_args!("{text}"),
    );
    let Some(logger) = LOGGER.get() else {
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(text.as_bytes());
        let _ = stderr.flush();
        return;
    };
    if let Ok(mut sink) = logger.sink.try_lock() {
        logger.append(&mut sink, &text);
        return;
    }
    if let Ok(mut file) = OpenOptions::new().append(true).open(&logger.path) {
        let _ = file.write_all(text.as_bytes());
        let _ = file.flush();
    }
}

#[cfg(test)]
mod tests {
    use log::Log as _;

    use super::*;

    fn logger_in(name: &str) -> (Arc<FileLogger>, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "kwikpaste-logger-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        (FileLogger::open(dir.join(FILE_NAME)), dir)
    }

    fn emit(logger: &FileLogger, level: log::Level, target: &str, text: &str) {
        logger.log(
            &log::Record::builder()
                .level(level)
                .target(target)
                .args(format_args!("{text}"))
                .build(),
        );
    }

    #[test]
    fn lines_use_the_1x_format_and_levels() {
        let (logger, dir) = logger_in("levels");
        emit(&logger, log::Level::Info, "KwikPaste::platform", "ours");
        emit(&logger, log::Level::Info, "kwikpaste_core::db", "core");
        emit(&logger, log::Level::Info, "gpui", "theirs");
        logger.flush();
        emit(&logger, log::Level::Warn, "gpui", "their warning");

        let text = std::fs::read_to_string(dir.join(FILE_NAME)).expect("log file");
        let first = text.lines().next().expect("a line");
        // [2026-10-03][04:05:06][target][LEVEL] message
        assert_eq!(&first[..1], "[");
        assert_eq!(&first[11..13], "][");
        assert!(first.ends_with("][KwikPaste::platform][INFO] ours"));
        assert!(text.contains("[kwikpaste_core::db][INFO] core"));
        assert!(!text.contains("theirs"));
        assert!(text.contains("[gpui][WARN] their warning"));
        drop(logger);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_file_starts_over_past_40_000_bytes() {
        let (logger, dir) = logger_in("rotate");
        let filler = "x".repeat(1000);
        let len = line(
            "KwikPaste::health",
            log::Level::Info,
            &format_args!("{filler}"),
        )
        .len() as u64;
        let fit = MAX_BYTES / len;
        for _ in 0..fit {
            emit(&logger, log::Level::Info, "KwikPaste::health", &filler);
        }
        logger.flush();
        let path = dir.join(FILE_NAME);
        assert_eq!(std::fs::metadata(&path).expect("log").len(), fit * len);

        emit(&logger, log::Level::Info, "KwikPaste::health", &filler);
        logger.flush();
        let text = std::fs::read_to_string(&path).expect("log file");
        assert_eq!(text.lines().count(), 1, "the file was not started over");
        assert_eq!(
            std::fs::metadata(dir.join("KwikPaste.log.1"))
                .unwrap()
                .len(),
            fit * len
        );
        drop(logger);

        // 打开时已经超过上限：同样从头开始。
        std::fs::write(&path, vec![b'y'; 41_000]).expect("big file");
        let reopened = FileLogger::open(path.clone());
        emit(&reopened, log::Level::Info, "KwikPaste::health", "fresh");
        reopened.flush();
        let text = std::fs::read_to_string(&path).expect("log file");
        assert!(text.ends_with("[KwikPaste::health][INFO] fresh\n"));
        assert!(!text.contains('y'));
        assert_eq!(
            std::fs::read(dir.join("KwikPaste.log.1")).unwrap(),
            vec![b'y'; 41_000]
        );
        drop(reopened);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn ordinary_lines_do_not_wait_for_file_io_and_overflow_is_reported() {
        let (logger, dir) = logger_in("queue");
        let sink = logger.sink.lock().unwrap();
        let producer = logger.clone();
        let (done, completed) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            for _ in 0..QUEUE_CAPACITY + 2 {
                emit(&producer, log::Level::Info, "KwikPaste", "ordinary");
            }
            done.send(()).unwrap();
        });

        completed
            .recv_timeout(Duration::from_secs(2))
            .expect("logging must not wait for the sink");
        drop(sink);
        thread.join().unwrap();
        logger.flush();

        let text = std::fs::read_to_string(dir.join(FILE_NAME)).unwrap();
        assert!(text.contains("[INFO] ordinary"));
        assert!(text.contains("ordinary log lines: queue full"));
        drop(logger);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn warn_and_error_bypass_a_full_queue() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(FILE_NAME);
        let (file, size) = open_append(&path);
        let (ordinary, _pending) = mpsc::sync_channel(1);
        let logger = FileLogger {
            path: path.clone(),
            sink: Mutex::new(Sink { file, size }),
            ordinary,
            dropped: AtomicUsize::new(0),
        };
        emit(&logger, log::Level::Info, "KwikPaste", "queued");
        emit(&logger, log::Level::Warn, "KwikPaste", "warning");
        emit(&logger, log::Level::Error, "KwikPaste", "error");

        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("[WARN] warning"));
        assert!(text.contains("[ERROR] error"));
        assert!(!text.contains("queued"));
    }
}
