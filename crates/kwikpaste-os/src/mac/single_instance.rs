//! macOS 单实例：`/tmp/<identifier>_si.sock`，与 tauri-plugin-single-instance 相同。

use std::io::{self, ErrorKind, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::thread;

use crate::single_instance::{
    Claim, Invocation, current_invocation, decode_nul, encode_nul, socket_path,
};

type Sink = Box<dyn Fn(Invocation) + Send>;

/// 主实例守卫：丢弃时删掉 socket 文件。监听线程随进程退出。
pub struct PrimaryInstance {
    socket: PathBuf,
}

impl Drop for PrimaryInstance {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

pub(crate) fn claim(identifier: &str, sink: Sink) -> io::Result<Claim> {
    let socket = socket_path(identifier);

    match forward(&socket, &current_invocation()) {
        Ok(()) => Ok(Claim::Forwarded),
        Err(err)
            if matches!(
                err.kind(),
                ErrorKind::NotFound | ErrorKind::ConnectionRefused
            ) =>
        {
            // 没有主实例在听：上一个实例崩溃时可能留下了陈旧的 socket 文件。
            let _ = std::fs::remove_file(&socket);
            let listener = UnixListener::bind(&socket)?;
            thread::Builder::new()
                .name("single-instance".to_owned())
                .spawn(move || listen(listener, sink))?;
            Ok(Claim::Primary(PrimaryInstance { socket }))
        }
        Err(err) => Err(err),
    }
}

fn forward(socket: &Path, invocation: &Invocation) -> io::Result<()> {
    let mut stream = UnixStream::connect(socket)?;
    stream.write_all(&encode_nul(invocation))?;
    stream.flush()
}

fn listen(listener: UnixListener, sink: Sink) {
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(err) => {
                log::debug!("single instance accept failed: {err}");
                continue;
            }
        };
        let mut text = String::new();
        if let Err(err) = stream.read_to_string(&mut text) {
            log::debug!("single instance read failed: {err}");
            continue;
        }
        sink(decode_nul(&text));
    }
}
