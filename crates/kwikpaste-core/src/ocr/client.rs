//! 短生命周期 helper 客户端；管道读取线程随子进程一起结束。
use std::io;
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::protocol::{self, Request, Response};

pub(super) type ChildHandle = Arc<Mutex<Child>>;

pub(super) struct Client {
    pub child: ChildHandle,
    requests: Option<mpsc::SyncSender<Request>>,
    replies: Option<mpsc::Receiver<io::Result<Response>>>,
    reader: Option<JoinHandle<()>>,
    #[cfg(target_os = "windows")]
    _job: std::os::windows::io::OwnedHandle,
}

impl Client {
    /// 启动自己的 helper；隔离策略设置失败时必须杀掉子进程而非降级。
    pub fn start(exe: &std::path::Path) -> io::Result<Self> {
        let mut command = Command::new(exe);
        command
            .arg("--ocr-helper")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt as _;
            use windows_sys::Win32::System::Threading::{
                BELOW_NORMAL_PRIORITY_CLASS, CREATE_NO_WINDOW,
            };
            command.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
        }
        let mut child = command.spawn()?;
        #[cfg(target_os = "windows")]
        let job = match helper_job(&child) {
            Ok(job) => job,
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(err);
            }
        };
        let Some(mut stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("OCR stdin unavailable"));
        };
        let Some(mut stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("OCR stdout unavailable"));
        };
        let child = Arc::new(Mutex::new(child));
        let (sender, replies) = mpsc::sync_channel(1);
        let (requests, input) = mpsc::sync_channel(1);
        let reader = match std::thread::Builder::new()
            .name("ocr-pipe".into())
            .spawn(move || {
                let mut buffer = Vec::new();
                while let Ok(request) = input.recv() {
                    let response = protocol::write_frame(&mut stdin, &request, &mut buffer)
                        .and_then(|()| protocol::read_frame(&mut stdout, &mut buffer))
                        .and_then(|response| {
                            response.ok_or_else(|| {
                                io::Error::new(
                                    io::ErrorKind::UnexpectedEof,
                                    "OCR helper closed stdout",
                                )
                            })
                        });
                    let failed = response.is_err();
                    if sender.send(response).is_err() || failed {
                        break;
                    }
                }
            }) {
            Ok(reader) => reader,
            Err(err) => {
                kill(&child);
                return Err(err);
            }
        };
        Ok(Self {
            child,
            requests: Some(requests),
            replies: Some(replies),
            reader: Some(reader),
            #[cfg(target_os = "windows")]
            _job: job,
        })
    }

    /// 每图一次有界等待；调用方在错误后丢弃客户端并用新的 helper 继续。
    pub fn request(&mut self, request: &Request) -> io::Result<Response> {
        self.requests
            .as_ref()
            .ok_or_else(|| io::Error::other("OCR input closed"))?
            .send(request.clone())
            .map_err(|err| io::Error::new(io::ErrorKind::BrokenPipe, err))?;
        self.replies
            .as_ref()
            .ok_or_else(|| io::Error::other("OCR replies closed"))?
            .recv_timeout(Duration::from_secs(60))
            .map_err(|err| io::Error::new(io::ErrorKind::TimedOut, err))?
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // 关闭 stdin 使正常 helper 在 EOF 上退出；异常路径仅杀自己持有的 PID。
        self.requests.take();
        self.replies.take();
        let deadline = std::time::Instant::now() + Duration::from_millis(500);
        loop {
            let done = self
                .child
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .try_wait();
            match done {
                Ok(Some(_)) => break,
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                _ => {
                    kill(&self.child);
                    break;
                }
            }
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

pub(super) fn kill(child: &ChildHandle) {
    let mut child = child.lock().unwrap_or_else(|p| p.into_inner());
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(target_os = "windows")]
fn helper_job(child: &Child) -> io::Result<std::os::windows::io::OwnedHandle> {
    use std::os::windows::io::{AsRawHandle as _, FromRawHandle as _, OwnedHandle};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if handle.is_null() {
        return Err(io::Error::last_os_error());
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
        return Err(io::Error::last_os_error());
    }
    Ok(job)
}

/// PeakPagefileUsage 是 Windows 的进程私有提交峰值，不激活 OCR 或解码图片。
#[cfg(target_os = "windows")]
pub(super) fn peak_private_usage(child: &ChildHandle) -> u64 {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    let child = child.lock().unwrap_or_else(|p| p.into_inner());
    let mut memory: PROCESS_MEMORY_COUNTERS_EX = unsafe { std::mem::zeroed() };
    memory.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    if unsafe {
        GetProcessMemoryInfo(
            child.as_raw_handle(),
            (&raw mut memory).cast::<PROCESS_MEMORY_COUNTERS>(),
            memory.cb,
        )
    } == 0
    {
        return 0;
    }
    memory.PeakPagefileUsage as u64
}
