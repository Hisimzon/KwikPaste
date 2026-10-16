//! WIC 按显示尺寸解码；COM 和工厂只在各自的工作线程内复用。

use std::{os::windows::ffi::OsStrExt, path::Path};

use windows::{
    Win32::{
        Foundation::{GENERIC_READ, RPC_E_CHANGED_MODE},
        Graphics::Imaging::{
            CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICImagingFactory,
            WICBitmapInterpolationModeFant, WICConvertBitmapSource, WICDecodeMetadataCacheOnDemand,
        },
        System::Com::{
            CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
            CoUninitialize,
        },
    },
    core::PCWSTR,
};

struct Apartment(bool);

impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

struct Factory {
    // 字段按声明顺序释放：先释放 COM 对象，再退出 apartment。
    imaging: IWICImagingFactory,
    _apartment: Apartment,
}

impl Factory {
    /// 在当前线程初始化 COM；已有 STA 时沿用它，不替别人解除初始化。
    fn new() -> windows::core::Result<Self> {
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result != RPC_E_CHANGED_MODE {
            result.ok()?;
        }
        let apartment = Apartment(result.is_ok());
        let imaging =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };

        Ok(Self {
            imaging,
            _apartment: apartment,
        })
    }
}

thread_local! {
    static FACTORY: windows::core::Result<Factory> = Factory::new();
}

/// 解码静态图并缩放到指定物理尺寸，返回非预乘的 BGRA 字节；失败由调用方记录日志。
/// 尺寸上限由调用方检查；不分配完整源图的 RGBA 缓冲。
pub fn decode_bgra(path: &Path, width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
    FACTORY
        .try_with(|factory| {
            let factory = &factory.as_ref().map_err(Clone::clone)?.imaging;
            let wide_path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();

            // 所有 COM 对象都在工厂所属线程内创建、使用和释放。
            unsafe {
                let stream = factory.CreateStream()?;
                stream.InitializeFromFilename(PCWSTR(wide_path.as_ptr()), GENERIC_READ.0)?;
                let decoder = factory.CreateDecoderFromStream(
                    &stream,
                    std::ptr::null(),
                    WICDecodeMetadataCacheOnDemand,
                )?;
                let frame = decoder.GetFrame(0)?;

                let scaler = factory.CreateBitmapScaler()?;
                scaler.Initialize(&frame, width, height, WICBitmapInterpolationModeFant)?;
                let source = WICConvertBitmapSource(&GUID_WICPixelFormat32bppBGRA, &scaler)?;
                let len = width as usize * height as usize * 4;
                let mut pixels = Vec::new();
                pixels.try_reserve_exact(len)?;
                pixels.resize(len, 0);
                source.CopyPixels(std::ptr::null(), width * 4, &mut pixels)?;

                Ok(pixels)
            }
        })
        .map_err(|_| anyhow::anyhow!("WIC factory unavailable during thread shutdown"))?
}
