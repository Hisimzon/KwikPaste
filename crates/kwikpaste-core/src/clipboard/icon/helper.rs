//! Windows Shell 图标隔离：二进制管线协议与短生命周期 helper。

use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
use std::path::Path;

use super::{encode_icon, shell_icon};

use std::sync::{Mutex, OnceLock};

pub(super) static HELPER_EXE: OnceLock<std::path::PathBuf> = OnceLock::new();

/// 配置 Windows 图标 helper 的可执行文件；未配置时保留进程内 Shell 回退。
pub fn set_helper_exe(path: std::path::PathBuf) {
    let _ = HELPER_EXE.set(path);
}

/// 将所有 Shell 请求串行发送给按需启动的 helper，不在调用进程加载 Shell。
pub(super) fn helper_icon_png(path: &Path, size: u32) -> Option<Vec<u8>> {
    static CLIENT: OnceLock<Mutex<HelperClient>> = OnceLock::new();
    let exe = HELPER_EXE.get()?.clone();
    let client = CLIENT.get_or_init(|| Mutex::new(HelperClient::new(exe)));
    let mut client = match client.lock() {
        Ok(client) => client,
        Err(err) => {
            log::warn!("icon helper client lock failed: {err}");
            return None;
        }
    };
    client.request(path, size)
}

/// 一次请求的结果：`IdleExit` 表示 helper 恰好空闲退出，换新的重试。
enum Attempt {
    Done(Option<Vec<u8>>),
    IdleExit,
}

struct HelperClient {
    exe: std::path::PathBuf,
    child: Option<std::process::Child>,
    stdin: Option<std::process::ChildStdin>,
    stdout: Option<std::process::ChildStdout>,
    job: Option<OwnedHandle>,
    failures: u32,
    retry_after: std::time::Instant,
}

impl HelperClient {
    fn new(exe: std::path::PathBuf) -> Self {
        Self {
            exe,
            child: None,
            stdin: None,
            stdout: None,
            job: None,
            failures: 0,
            retry_after: std::time::Instant::now(),
        }
    }

    /// 请求的写入与响应读取共用 5 秒限时；失败后只终止自己创建的 helper。
    ///
    /// helper 空闲满 30 秒会自己退出，请求恰好赶上它退出时管道会断；这种情况换一个新 helper
    /// 重试一次，不算失败、不退避。
    fn request(&mut self, path: &Path, size: u32) -> Option<Vec<u8>> {
        match self.attempt(path, size) {
            Attempt::Done(bytes) => bytes,
            Attempt::IdleExit => match self.attempt(path, size) {
                Attempt::Done(bytes) => bytes,
                Attempt::IdleExit => {
                    self.fail_child();
                    None
                }
            },
        }
    }

    fn attempt(&mut self, path: &Path, size: u32) -> Attempt {
        if self.ensure_child().is_none() {
            return Attempt::Done(None);
        }
        let Some(mut stdin) = self.stdin.take() else {
            return Attempt::Done(None);
        };
        let Some(mut stdout) = self.stdout.take() else {
            return Attempt::Done(None);
        };
        let path = path.to_path_buf();
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        if let Err(err) = std::thread::Builder::new()
            .name("icon-helper-request".into())
            .spawn(move || {
                let result = write_request(&mut stdin, &path, size)
                    .and_then(|()| read_response(&mut stdout));
                let _ = sender.send((stdin, stdout, result));
            })
        {
            log::warn!("icon helper request thread failed: {err}");
            self.fail_child();
            return Attempt::Done(None);
        }
        match receiver.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok((stdin, stdout, Ok(bytes))) => {
                self.stdin = Some(stdin);
                self.stdout = Some(stdout);
                self.failures = 0;
                Attempt::Done(bytes)
            }
            Ok((_, _, Err(err))) => {
                if self.exited_when_idle() {
                    log::debug!("icon helper exited while idle, starting a new one");
                    return Attempt::IdleExit;
                }
                log::warn!("icon helper pipe failed: {err}");
                self.fail_child();
                Attempt::Done(None)
            }
            Err(err) => {
                log::warn!("icon helper request timed out or disconnected: {err}");
                self.fail_child();
                Attempt::Done(None)
            }
        }
    }

    /// 管道断开后看 helper 是不是空闲到点正常退出了（等它最多 500 ms 退完）；是就清掉，不退避。
    fn exited_when_idle(&mut self) -> bool {
        let Some(child) = self.child.as_mut() else {
            return false;
        };
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                _ => return false,
            }
        };
        if !status.success() {
            return false;
        }
        self.child = None;
        self.stdin = None;
        self.stdout = None;
        self.job = None;
        true
    }

    /// 空闲正常退出可以立即重启；异常退出和启动失败按指数退避。
    fn ensure_child(&mut self) -> Option<()> {
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(None) => return Some(()),
                Ok(Some(status)) if status.success() => {}
                result => {
                    log::warn!("icon helper exited unexpectedly: {result:?}");
                    self.fail_child();
                    return None;
                }
            }
            self.child = None;
            self.stdin = None;
            self.stdout = None;
            self.job = None;
        }
        if std::time::Instant::now() < self.retry_after {
            return None;
        }
        let mut command = std::process::Command::new(&self.exe);
        use std::os::windows::process::CommandExt as _;
        command
            .arg(HELPER_ARG)
            .creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        match command.spawn() {
            Ok(mut child) => {
                let job = match helper_job(&child) {
                    Ok(job) => Some(job),
                    Err(err) => {
                        log::warn!("icon helper job setup failed: {err}");
                        None
                    }
                };
                log::debug!("icon helper started: pid={}", child.id());
                self.stdin = child.stdin.take();
                self.stdout = child.stdout.take();
                self.child = Some(child);
                self.job = job;
                Some(())
            }
            Err(err) => {
                log::warn!("icon helper spawn failed: {err}");
                self.fail_child();
                None
            }
        }
    }

    fn fail_child(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        self.stdin = None;
        self.stdout = None;
        self.job = None;
        self.failures = self.failures.saturating_add(1);
        let seconds = (1u64 << self.failures.min(5)).min(30);
        self.retry_after = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    }
}

/// 父进程突然退出时连带结束 helper，避免 Shell 卡住后 EOF 无法被处理而留下孤儿。
fn helper_job(child: &std::process::Child) -> std::io::Result<OwnedHandle> {
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if handle.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    let job = unsafe { OwnedHandle::from_raw_handle(handle) };
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    } == 0
        || unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) } == 0
    {
        return Err(std::io::Error::last_os_error());
    }
    Ok(job)
}

const HELPER_ARG: &str = "--icon-helper";

const MAX_PROTOCOL_BYTES: u32 = 16 * 1024 * 1024;

struct IconRequest {
    path: std::path::PathBuf,
    size: u32,
}

/// 请求：路径字节长 u32 LE、尺寸 u32 LE、原始 UTF-16LE 路径（无终止符）。
fn write_request(writer: &mut impl std::io::Write, path: &Path, size: u32) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    let path: Vec<u16> = path.as_os_str().encode_wide().collect();
    let bytes = u32::try_from(path.len().saturating_mul(2)).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "icon path is too long")
    })?;
    if bytes > MAX_PROTOCOL_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "icon path is too long",
        ));
    }
    writer.write_all(&bytes.to_le_bytes())?;
    writer.write_all(&size.to_le_bytes())?;
    for unit in path {
        writer.write_all(&unit.to_le_bytes())?;
    }
    writer.flush()
}

/// 仅帧边界上的 EOF 为正常结束，半帧 EOF 视为协议失败。
fn read_request(reader: &mut impl std::io::Read) -> std::io::Result<Option<IconRequest>> {
    use std::os::windows::ffi::OsStringExt as _;
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length[..1]) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(err),
    }
    reader.read_exact(&mut length[1..])?;
    let bytes = u32::from_le_bytes(length);
    if bytes > MAX_PROTOCOL_BYTES || bytes % 2 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid icon request length",
        ));
    }
    let mut size = [0u8; 4];
    reader.read_exact(&mut size)?;
    let mut raw = vec![0u8; bytes as usize];
    reader.read_exact(&mut raw)?;
    let units: Vec<u16> = raw
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect();
    Ok(Some(IconRequest {
        path: std::ffi::OsString::from_wide(&units).into(),
        size: u32::from_le_bytes(size),
    }))
}

/// 响应：状态 u8（1 成功 / 0 失败）、PNG 字节长 u32 LE、PNG；失败长度为 0。
fn write_response(writer: &mut impl std::io::Write, result: Option<&[u8]>) -> std::io::Result<()> {
    let (ok, bytes) = result.map_or((0u8, &[][..]), |bytes| (1, bytes));
    let length = u32::try_from(bytes.len()).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "icon response is too large",
        )
    })?;
    writer.write_all(&[ok])?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(bytes)?;
    writer.flush()
}

/// 校验响应长度与状态后读取完整 PNG。
fn read_response(reader: &mut impl std::io::Read) -> std::io::Result<Option<Vec<u8>>> {
    let mut ok = [0u8; 1];
    reader.read_exact(&mut ok)?;
    let mut length = [0u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_le_bytes(length);
    if length > MAX_PROTOCOL_BYTES || ok[0] > 1 || (ok[0] == 0 && length != 0) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid icon response length",
        ));
    }
    let mut bytes = vec![0u8; length as usize];
    reader.read_exact(&mut bytes)?;
    Ok((ok[0] == 1).then_some(bytes))
}

/// 运行 Windows 图标 helper；输入输出均为二进制协议，EOF 或空闲约 30 秒后退出。
pub fn run_helper<R: std::io::Read + Send + 'static, W: std::io::Write>(
    reader: R,
    mut writer: W,
) -> std::io::Result<()> {
    let (sender, receiver) =
        std::sync::mpsc::sync_channel::<std::io::Result<Option<IconRequest>>>(1);
    std::thread::Builder::new()
        .name("icon-helper-input".into())
        .spawn(move || {
            let mut reader = reader;
            loop {
                match read_request(&mut reader) {
                    Ok(request) => {
                        let done = request.is_none();
                        if sender.send(Ok(request)).is_err() {
                            break;
                        }
                        if done {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = sender.send(Err(err));
                        break;
                    }
                }
            }
        })?;
    loop {
        let request = match receiver.recv_timeout(std::time::Duration::from_secs(30)) {
            Ok(request) => request,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let request = match request {
            Ok(Some(request)) => request,
            Ok(None) => break,
            Err(err) => {
                let _ = write_response(&mut writer, None);
                return Err(err);
            }
        };
        let png = shell_icon(&request.path, request.size)
            .and_then(|icon| encode_icon(icon, &request.path, request.size));
        write_response(&mut writer, png.as_deref())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn helper_protocol_round_trips_success_and_failure() {
        let system = std::env::var_os("SystemRoot").expect("SystemRoot is set");
        let notepad = Path::new(&system).join("System32").join("notepad.exe");
        let mut request = Vec::new();
        write_request(&mut request, &notepad, 64).unwrap();
        let missing = Path::new(&system).join("does-not-exist.kwpk");
        write_request(&mut request, &missing, 64).unwrap();
        write_request(&mut request, &notepad, 32).unwrap();
        let mut response = Vec::new();
        run_helper(Cursor::new(request), &mut response).unwrap();
        let mut response = Cursor::new(response);
        let png = read_response(&mut response).unwrap().unwrap();
        let expected = shell_icon(&notepad, 64)
            .and_then(|icon| encode_icon(icon, &notepad, 64))
            .unwrap();
        assert_eq!(png, expected);
        assert!(read_response(&mut response).unwrap().is_none());
        let png = read_response(&mut response).unwrap().unwrap();
        let decoded = image::load_from_memory(&png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (32, 32));
        assert_eq!(response.position() as usize, response.get_ref().len());
    }

    #[test]
    fn helper_rejects_truncated_frames_and_invalid_responses() {
        let mut response = Vec::new();
        let err = run_helper(Cursor::new(vec![1u8]), &mut response).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::UnexpectedEof);
        assert!(read_response(&mut Cursor::new(response)).unwrap().is_none());
        assert!(read_response(&mut Cursor::new(vec![2, 0, 0, 0, 0])).is_err());
        assert!(read_response(&mut Cursor::new(vec![0, 1, 0, 0, 0])).is_err());
    }
}
