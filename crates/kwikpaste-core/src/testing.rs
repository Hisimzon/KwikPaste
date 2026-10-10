//! 单元测试共用的 core 夹具：临时数据目录、自带 runtime、内存剪贴板、假的平台层，
//! 以及不依赖 tokio 的最小执行器（模拟 GPUI 这类非 tokio 执行器）。

use std::future::Future;
use std::path::Path;
use std::pin::pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Wake, Waker};

use crate::clipboard::MemoryClipboard;
use crate::db::models::Platform;
use crate::env::{AppEnv, AppInfo, CoreOptions};
use crate::error::{AppError, Result};
use crate::events::CoreEvent;
use crate::paths::CorePaths;
use crate::platform::{FrontmostApp, PlatformServices, ScannedApp};
use crate::root::Core;
use crate::runtime::CoreRuntime;

/// 在当前线程上跑完一个 future，不进入任何 tokio 上下文。
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    struct ThreadWaker(std::thread::Thread);

    impl Wake for ThreadWaker {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    let waker = Waker::from(Arc::new(ThreadWaker(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::park();
    }
}

pub(crate) fn sample_png(w: u32, h: u32) -> Vec<u8> {
    let buf = image::RgbaImage::from_pixel(w, h, image::Rgba([9, 8, 7, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(buf)
        .write_to(&mut out, image::ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

/// 假的平台层：前台应用与运行中应用由测试指定，提示音只记录次数和音量，不发声。
#[derive(Default)]
pub(crate) struct FakePlatform {
    pub frontmost: Mutex<Option<FrontmostApp>>,
    pub running: Mutex<Vec<ScannedApp>>,
    pub sounds: AtomicUsize,
    pub sound_volumes: Mutex<Vec<u8>>,
}

impl FakePlatform {
    pub fn set_frontmost(&self, id: &str, name: &str) {
        *self.frontmost.lock().unwrap() = Some(FrontmostApp {
            id: id.to_owned(),
            name: name.to_owned(),
            platform: Platform::Windows,
            icon_source: None,
        });
    }

    pub fn sounds(&self) -> usize {
        self.sounds.load(Ordering::SeqCst)
    }
}

impl PlatformServices for FakePlatform {
    fn frontmost_app(&self) -> Option<FrontmostApp> {
        self.frontmost.lock().unwrap().clone()
    }

    fn running_apps(&self) -> Vec<ScannedApp> {
        self.running.lock().unwrap().clone()
    }

    fn app_from_path(&self, path: &Path) -> Result<ScannedApp> {
        if path.extension().and_then(|ext| ext.to_str()) != Some("exe") {
            return Err(AppError::Clipboard(
                "please choose a Windows executable".to_owned(),
            ));
        }

        Ok(ScannedApp {
            id: path.to_string_lossy().into_owned(),
            name: path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_owned(),
            path: Some(path.to_path_buf()),
            platform: Platform::Windows,
        })
    }

    fn app_from_id(&self, _id: &str) -> Option<ScannedApp> {
        None
    }

    fn play_copy_sound(&self, volume_percent: u8) {
        self.sounds.fetch_add(1, Ordering::SeqCst);
        self.sound_volumes.lock().unwrap().push(volume_percent);
    }
}

pub(crate) struct Fixture {
    temp: tempfile::TempDir,
    pub runtime: CoreRuntime,
    pub paths: CorePaths,
    pub events: Arc<Mutex<Vec<CoreEvent>>>,
    pub clipboard: MemoryClipboard,
    pub platform: Arc<FakePlatform>,
}

impl Fixture {
    pub fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let local = temp.path().join("local");
        let paths = CorePaths::new(AppEnv::Dev, local.clone(), local.join("logs"), None);

        Self {
            temp,
            runtime: CoreRuntime::new().unwrap(),
            paths,
            events: Arc::default(),
            clipboard: MemoryClipboard::new(),
            platform: Arc::default(),
        }
    }

    pub fn root(&self) -> &Path {
        self.temp.path()
    }

    pub fn write_settings(&self, content: &str) {
        let dir = self.paths.config_dir().unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("settings.json"), content).unwrap();
    }

    /// 启动 core，接上内存剪贴板与假的平台层。
    pub fn start(&self) -> Core {
        let events = self.events.clone();
        let sink = move |event: CoreEvent| {
            events.lock().unwrap().push(event);
        };
        let info = AppInfo {
            name: crate::APP_NAME,
            identifier: crate::APP_IDENTIFIER,
            version: semver::Version::new(2, 0, 0),
            env: AppEnv::Dev,
        };

        let core = block_on(Core::start(
            info,
            self.paths.clone(),
            CoreOptions::default(),
            Arc::new(sink),
            self.runtime.handle(),
        ))
        .unwrap();
        core.set_clipboard_provider(Arc::new(self.clipboard.clone()));
        core.set_platform_services(self.platform.clone());
        core
    }

    pub fn take_events(&self) -> Vec<CoreEvent> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
}
