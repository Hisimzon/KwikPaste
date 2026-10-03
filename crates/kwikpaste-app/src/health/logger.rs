//! 普通启动的日志文件：`<日志目录>/KwikPaste.log`，与 1.x（tauri-plugin-log）同一个文件，追加写。
//!
//! 本 workspace 的 crate 记到 info（debug 构建到 debug），其余 crate 只记 warn 以上。文件超过
//! [`MAX_BYTES`] 时改名为 `KwikPaste.old.log`（覆盖上一份）再重新开始，最多占两份的空间。

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

const FILE_NAME: &str = "KwikPaste.log";
const OLD_FILE_NAME: &str = "KwikPaste.old.log";
const MAX_BYTES: u64 = 2 * 1024 * 1024;

struct FileLogger {
    path: PathBuf,
    sink: Mutex<Sink>,
}

struct Sink {
    file: Option<File>,
    written: u64,
}

static LOGGER: OnceLock<FileLogger> = OnceLock::new();

/// 打开日志文件并设为全局 logger。目录建不了、文件打不开时什么也不做（应用照常运行）。
pub fn init(dir: PathBuf) {
    if let Err(err) = std::fs::create_dir_all(&dir) {
        eprintln!(
            "log directory {} could not be created: {err}",
            dir.display()
        );
        return;
    }
    let path = dir.join(FILE_NAME);
    let (file, written) = open(&path);
    let logger = LOGGER.get_or_init(|| FileLogger {
        path,
        sink: Mutex::new(Sink { file, written }),
    });
    if log::set_logger(logger).is_ok() {
        log::set_max_level(if cfg!(debug_assertions) {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        });
    }
}

/// 打开（必要时先轮换）日志文件，返回文件和已有的字节数。
fn open(path: &Path) -> (Option<File>, u64) {
    let size = std::fs::metadata(path).map_or(0, |meta| meta.len());
    let size = if size > MAX_BYTES {
        rotate(path);
        0
    } else {
        size
    };
    match OpenOptions::new().create(true).append(true).open(path) {
        Ok(file) => (Some(file), size),
        Err(err) => {
            eprintln!("log file {} could not be opened: {err}", path.display());
            (None, 0)
        }
    }
}

fn rotate(path: &Path) {
    let old = path.with_file_name(OLD_FILE_NAME);
    let _ = std::fs::remove_file(&old);
    let _ = std::fs::rename(path, old);
}

fn timestamp() -> String {
    chrono::Local::now()
        .format("%Y-%m-%d %H:%M:%S%.3f")
        .to_string()
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
        let line = format!(
            "[{}][{}][{}] {}\n",
            timestamp(),
            record.level(),
            record.target(),
            record.args()
        );
        self.append(&line);
    }

    fn flush(&self) {
        if let Ok(mut sink) = self.sink.lock()
            && let Some(file) = &mut sink.file
        {
            let _ = file.flush();
        }
    }
}

impl FileLogger {
    fn append(&self, text: &str) {
        let Ok(mut sink) = self.sink.lock() else {
            return;
        };
        if sink.written > MAX_BYTES {
            sink.file = None;
            rotate(&self.path);
            let (file, written) = open(&self.path);
            sink.file = file;
            sink.written = written;
        }
        if let Some(file) = &mut sink.file
            && file.write_all(text.as_bytes()).is_ok()
        {
            sink.written += text.len() as u64;
        }
    }
}

/// panic hook 用：把崩溃信息写进日志并落盘。日志的锁拿不到（panic 正发生在写日志时）就另开一个
/// 句柄追加，不会在 hook 里死锁。自测进程写 stderr。
pub fn write_crash(text: &str) {
    let line = format!("[{}][ERROR][KwikPaste::health] {text}\n", timestamp());
    let Some(logger) = LOGGER.get() else {
        let _ = std::io::stderr().lock().write_all(line.as_bytes());
        return;
    };
    if let Ok(mut sink) = logger.sink.try_lock()
        && let Some(file) = &mut sink.file
    {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
        return;
    }
    if let Ok(mut file) = OpenOptions::new().append(true).open(&logger.path) {
        let _ = file.write_all(line.as_bytes());
        let _ = file.flush();
    }
}

#[cfg(test)]
mod tests {
    use log::Log as _;

    use super::*;

    fn logger_in(name: &str) -> (FileLogger, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "kwikpaste-logger-test-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(FILE_NAME);
        let (file, written) = open(&path);
        let logger = FileLogger {
            path,
            sink: Mutex::new(Sink { file, written }),
        };
        (logger, dir)
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
    fn our_info_lines_are_kept_and_other_crates_only_from_warn() {
        let (logger, dir) = logger_in("levels");
        emit(&logger, log::Level::Info, "KwikPaste::platform", "ours");
        emit(&logger, log::Level::Info, "mdns_sd", "theirs");
        emit(&logger, log::Level::Warn, "mdns_sd", "their warning");
        logger.flush();

        let text = std::fs::read_to_string(dir.join(FILE_NAME)).expect("log file");
        assert!(text.contains("[INFO][KwikPaste::platform] ours"));
        assert!(!text.contains("theirs"));
        assert!(text.contains("[WARN][mdns_sd] their warning"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_full_file_moves_to_the_old_name() {
        let (logger, dir) = logger_in("rotate");
        let line = "x".repeat(64 * 1024);
        for _ in 0..34 {
            emit(&logger, log::Level::Error, "KwikPaste::health", &line);
        }
        emit(
            &logger,
            log::Level::Error,
            "KwikPaste::health",
            "after rotation",
        );
        logger.flush();

        let current = std::fs::read_to_string(dir.join(FILE_NAME)).expect("log file");
        let old = std::fs::metadata(dir.join(OLD_FILE_NAME)).expect("old log file");
        assert!(current.contains("after rotation"));
        assert!(current.len() < 1024 * 1024);
        assert!(old.len() > MAX_BYTES);
        drop(logger);
        let _ = std::fs::remove_dir_all(dir);
    }
}
