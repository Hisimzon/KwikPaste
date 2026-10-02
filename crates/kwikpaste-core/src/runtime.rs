//! core 自带的 tokio runtime，以及把任务切到它上面执行的 [`hop`]。
//!
//! sqlx、tokio 定时器和阻塞线程池都要求处在 tokio 上下文里，而宿主（GPUI）的执行器不是 tokio。
//! core 的公开 async API 一律先 `hop` 到 core runtime 上执行，再在调用方的执行器里等待
//! `JoinHandle`，所以调用方不需要处在 tokio 上下文里。

use std::future::Future;

use anyhow::{anyhow, Context};
use tokio::runtime::{Builder, Handle, Runtime};

use crate::error::{AppError, Result};

/// 工作线程数：剪贴板管线和数据库操作都很轻，两个线程足够，空闲时不多占内存。
const WORKER_THREADS: usize = 2;
/// 阻塞线程池上限：缩略图解码、SQLite 读写都在这里跑。
const MAX_BLOCKING_THREADS: usize = 4;

/// 原生宿主为 core 建的 runtime，在 GUI 事件循环开始之前创建，进程结束时丢弃。
pub struct CoreRuntime {
    runtime: Runtime,
}

impl CoreRuntime {
    pub fn new() -> Result<Self> {
        let runtime = Builder::new_multi_thread()
            .worker_threads(WORKER_THREADS)
            .max_blocking_threads(MAX_BLOCKING_THREADS)
            .thread_name("kp-core")
            .enable_all()
            .build()
            .context("failed to start the core runtime")?;
        Ok(Self { runtime })
    }

    /// 交给 [`crate::Core::start`] 的句柄，可以随意克隆。
    pub fn handle(&self) -> Handle {
        self.runtime.handle().clone()
    }
}

/// 在 `rt` 上执行 `fut` 并等待结果。返回的 future 可以在任意执行器里 await。
///
/// 调用方的 future 被丢弃时，已经切过去的任务仍会执行完，写操作不会做到一半。
pub async fn hop<T, F>(rt: &Handle, fut: F) -> Result<T>
where
    F: Future<Output = Result<T>> + Send + 'static,
    T: Send + 'static,
{
    rt.spawn(fut)
        .await
        .map_err(|err| AppError::Other(anyhow!("core task did not finish: {err}")))?
}
