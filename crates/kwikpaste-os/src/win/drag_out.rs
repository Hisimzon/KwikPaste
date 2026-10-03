//! Windows 拖出：OLE `DoDragDrop`（附录 C §5，实测见 r2-03）。
//!
//! - 文本：自实现的 `IDataObject`（与 1.x 相同）提供 `Rich Text Format`、`HTML Format`、
//!   `CF_UNICODETEXT`、`CF_TEXT`，接收方按偏好取。`EnumFormatEtc` 必须有（Qt / WinForms / WPF 靠它
//!   列格式），`tymed` 按位掩码认，OLE 拖拽不会像剪贴板那样自动合成 `CF_TEXT`。
//! - 文件（图片记录拖原图文件）：Shell 的数据对象（`IShellItemArray` → `BHID_DataObject`），
//!   带 `CF_HDROP`、`FileContents` 等全套 Shell 格式。
//! - 预览图：PNG 经 WIC 解成 32 位预乘 BGRA，交给 `IDragSourceHelper::InitializeFromBitmap`。
//!
//! 约束（调用方，见 `kwikpaste-app` 的 `platform::drag_out`）：
//! - [`run`] 必须在主线程（拥有窗口、已 `OleInitialize` 的线程）上调用，并且在 GPUI 的 `cx.spawn`
//!   前台任务里、不持有任何 `update` 借用；在事件处理器里同步调用会让窗口 0 帧、丢事件。
//! - 拖出期间 GPUI 自己的 `IDropTarget` 会把自拖当成外部拖入（文本合成一次 `MouseUp`，文件发
//!   `FileDrop`）。[`run`] 先给窗口装一层过滤（`GetPropW("OleDropTargetInterface")` →
//!   `RevokeDragDrop` → `RegisterDragDrop(包装)`）：本进程在拖出时只转发给拖影 helper、回
//!   `DROPEFFECT_NONE`，其余时候原样转发。
//! - Esc：面板从不激活，ole32 自己看不到 Esc。键盘钩子在拖出时吞掉 Esc 并调用 [`request_cancel`]，
//!   `QueryContinueDrag` 看到标志就返回 `DRAGDROP_S_CANCEL`（见 [`request_cancel`]）。
//! - 投放是否成功看 effect，不看返回码：目标回 NONE 时返回码仍是 `DRAGDROP_S_DROP`。

use std::ffi::c_void;
use std::io;
use std::iter::once;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicIsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    COLORREF, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, DV_E_FORMATETC,
    E_NOTIMPL, HWND, LPARAM, OLE_E_ADVISENOTSUPPORTED, POINT, POINTL, S_OK, SIZE, WPARAM,
};
use windows::Win32::Globalization::{CP_ACP, WideCharToMultiByte};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDIBSection, DIB_RGB_COLORS, DeleteObject,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
    WICConvertBitmapSource, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, DATADIR_GET, DVASPECT_CONTENT, FORMATETC, IAdviseSink,
    IDataObject, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, STGMEDIUM, STGMEDIUM_0,
    TYMED_HGLOBAL,
};
use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
use windows::Win32::System::Memory::{
    GMEM_FIXED, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::{
    CF_TEXT, CF_UNICODETEXT, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_NONE, DoDragDrop, IDropSource,
    IDropSource_Impl, IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium,
    RevokeDragDrop,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    BHID_DataObject, CLSID_DragDropHelper, IDragSourceHelper, IDropTargetHelper, ILCreateFromPathW,
    ILFree, IShellItemArray, SHCreateShellItemArrayFromIDLists, SHCreateStdEnumFmtEtc, SHDRAGIMAGE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetCursorPos, GetPropW, GetSystemMetrics, GetWindowThreadProcessId,
    PostMessageW, SM_CXDRAG, SM_CYDRAG, WM_KEYDOWN, WindowFromPoint,
};
use windows::core::{
    BOOL, Error as WinError, HRESULT, Interface as _, PCSTR, PCWSTR, Ref, implement, w,
};

use crate::clock;
use crate::drag_out::{DragData, DragReport, DragResult, set_active};

static CF_HTML: OnceLock<u16> = OnceLock::new();
static CF_RTF: OnceLock<u16> = OnceLock::new();

/// 拖出中按了 Esc（钩子线程置位，`QueryContinueDrag` 读）。
static CANCEL: AtomicBool = AtomicBool::new(false);
static CANCEL_AT: AtomicI64 = AtomicI64::new(0);
/// 拖出源窗口：取消时往它投一条 Esc 按下，唤醒 ole32 的模态循环。
static DRAG_WINDOW: AtomicIsize = AtomicIsize::new(0);
/// 已经装过过滤层的窗口。
static FILTERED: Mutex<Vec<isize>> = Mutex::new(Vec::new());
static GUARD: OnceLock<DropGuard> = OnceLock::new();

/// 自测保护：只允许投放到这些进程的窗口上，拖拽超过 `max` 自动取消。正式运行不设。
struct DropGuard {
    allowed: Box<dyn Fn(u32) -> bool + Send + Sync>,
    max: Duration,
}

/// 自测用：松开时光标下窗口的进程 id 不被 `allowed` 接受就取消，拖拽超过 `max` 也取消。
/// 进程内只能设一次。
pub fn set_drop_guard(allowed: impl Fn(u32) -> bool + Send + Sync + 'static, max: Duration) {
    let _ = GUARD.set(DropGuard {
        allowed: Box::new(allowed),
        max,
    });
}

/// 系统的拖拽阈值（物理像素）：按下后移动超过它才算开始拖动。
pub fn threshold() -> (i32, i32) {
    let x = unsafe { GetSystemMetrics(SM_CXDRAG) };
    let y = unsafe { GetSystemMetrics(SM_CYDRAG) };

    (x.max(1), y.max(1))
}

/// 键盘钩子在拖出时收到 Esc 调用：置取消标志，并往拖出源窗口投一条 Esc 按下。ole32 的拖拽循环
/// 见到键盘消息会立刻调 `QueryContinueDrag`（实测约 15 ms）并自己消费掉它，不派发给 GPUI；
/// `WM_NULL` 或按键松开都叫不醒它，要等它约 60–95 ms 一次的轮询。没在拖出时返回 `false`。
pub fn request_cancel() -> bool {
    if !crate::drag_out::is_active() {
        return false;
    }
    CANCEL_AT.store(clock::now_ticks(), Ordering::SeqCst);
    CANCEL.store(true, Ordering::SeqCst);
    let window = DRAG_WINDOW.load(Ordering::SeqCst);
    if window != 0 {
        let _ = unsafe {
            PostMessageW(
                Some(HWND(window as *mut c_void)),
                WM_KEYDOWN,
                WPARAM(0x1B),
                LPARAM(0x0001_0001),
            )
        };
    }

    true
}

/// 从 `window`（GPUI 窗口句柄）开始一次拖出，阻塞到投放或取消。只能在主线程、GPUI 借用之外调用
/// （见模块文档）。`preview_png` 是跟随光标的预览图。
pub fn run(window: isize, data: &DragData, preview_png: Option<&[u8]>) -> io::Result<DragReport> {
    if crate::drag_out::is_active() {
        return Err(io::Error::other("a drag-out is already running"));
    }
    install_filter(window);

    let object = data_object(data)?;
    if let Some(png) = preview_png
        && let Err(err) = attach_preview(&object, png)
    {
        log::debug!("drag preview unavailable: {err}");
    }

    let source: IDropSource = DropSource {
        started: Instant::now(),
    }
    .into();
    CANCEL.store(false, Ordering::SeqCst);
    DRAG_WINDOW.store(window, Ordering::SeqCst);
    set_active(true);
    let mut effect = DROPEFFECT_NONE;
    let hresult = unsafe { DoDragDrop(&object, &source, DROPEFFECT_COPY, &mut effect) };
    set_active(false);
    DRAG_WINDOW.store(0, Ordering::SeqCst);
    let cancelled_by_key = CANCEL.swap(false, Ordering::SeqCst);

    let result = if hresult == DRAGDROP_S_DROP {
        if effect.0 & DROPEFFECT_COPY.0 != 0 {
            DragResult::Dropped
        } else {
            DragResult::Refused
        }
    } else if hresult == DRAGDROP_S_CANCEL {
        DragResult::Cancelled
    } else {
        return Err(io::Error::other(WinError::from_hresult(hresult)));
    };

    Ok(DragReport {
        result,
        effect: effect.0,
        hresult: hresult.0,
        cancel_requested: cancelled_by_key.then(|| CANCEL_AT.load(Ordering::SeqCst)),
    })
}

fn data_object(data: &DragData) -> io::Result<IDataObject> {
    match data {
        DragData::Text { plain, html, rtf } => {
            if plain.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "drag-out text is empty",
                ));
            }
            Ok(RichDataObject::new(plain, html.as_deref(), rtf.as_deref()).into())
        }
        DragData::Files(paths) => files_object(paths),
    }
}

// ------------------------------------------------------------------------------------ 文件

fn files_object(paths: &[PathBuf]) -> io::Result<IDataObject> {
    if paths.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "drag-out file list is empty",
        ));
    }
    let wides: Vec<Vec<u16>> = paths
        .iter()
        .map(|path| path.as_os_str().encode_wide().chain(once(0)).collect())
        .collect();
    let pidls: Vec<*mut ITEMIDLIST> = wides
        .iter()
        .map(|wide| unsafe { ILCreateFromPathW(PCWSTR(wide.as_ptr())) })
        .collect();
    let free = |pidls: &[*mut ITEMIDLIST]| {
        for &pidl in pidls {
            if !pidl.is_null() {
                unsafe { ILFree(Some(pidl)) };
            }
        }
    };
    if let Some(index) = pidls.iter().position(|pidl| pidl.is_null()) {
        free(&pidls);
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("{} has no shell item", paths[index].display()),
        ));
    }

    let list: Vec<*const ITEMIDLIST> = pidls.iter().map(|pidl| pidl.cast_const()).collect();
    let object = unsafe { SHCreateShellItemArrayFromIDLists(&list) }
        .and_then(|array: IShellItemArray| unsafe { array.BindToHandler(None, &BHID_DataObject) });
    free(&pidls);

    object.map_err(io::Error::other)
}

// ------------------------------------------------------------------------------------ 预览图

/// PNG → 32 位预乘 BGRA 的自顶向下 DIB → `InitializeFromBitmap`（成功后位图归拖影管理器）。
fn attach_preview(object: &IDataObject, png: &[u8]) -> windows::core::Result<()> {
    let (width, height, pixels) = decode_pbgra(png)?;
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    let bitmap = unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }?;
    unsafe { std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len()) };

    let image = SHDRAGIMAGE {
        sizeDragImage: SIZE {
            cx: width as i32,
            cy: height as i32,
        },
        ptOffset: POINT {
            x: width as i32 / 2,
            y: height as i32 / 2,
        },
        hbmpDragImage: bitmap,
        // CLR_NONE：按 alpha 通道透明。
        crColorKey: COLORREF(0xFFFF_FFFF),
    };
    let helper: IDragSourceHelper =
        unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) }?;
    let attached = unsafe { helper.InitializeFromBitmap(&image, object) };
    if attached.is_err() {
        let _ = unsafe { DeleteObject(bitmap.into()) };
    }

    attached
}

fn decode_pbgra(png: &[u8]) -> windows::core::Result<(u32, u32, Vec<u8>)> {
    let factory: IWICImagingFactory =
        unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }?;
    let stream = unsafe { factory.CreateStream() }?;
    unsafe { stream.InitializeFromMemory(png) }?;
    let decoder = unsafe {
        factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)
    }?;
    let frame = unsafe { decoder.GetFrame(0) }?;
    let (mut width, mut height) = (0, 0);
    unsafe { frame.GetSize(&mut width, &mut height) }?;
    let converted = unsafe { WICConvertBitmapSource(&GUID_WICPixelFormat32bppPBGRA, &frame) }?;
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    unsafe { converted.CopyPixels(std::ptr::null(), width * 4, &mut pixels) }?;

    Ok((width, height, pixels))
}

// ------------------------------------------------------------------------------------ 拖放源

#[implement(IDropSource)]
struct DropSource {
    started: Instant,
}

impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, escape: BOOL, keys: MODIFIERKEYS_FLAGS) -> HRESULT {
        if escape.as_bool() || CANCEL.load(Ordering::SeqCst) {
            return DRAGDROP_S_CANCEL;
        }
        let guard = GUARD.get();
        if let Some(guard) = guard
            && self.started.elapsed() > guard.max
        {
            log::warn!("selftest drop guard: the drag ran too long; cancelled");
            return DRAGDROP_S_CANCEL;
        }
        if keys.0 & MK_LBUTTON.0 != 0 {
            return S_OK;
        }
        if let Some(guard) = guard {
            let process = process_under_cursor();
            if !(guard.allowed)(process) {
                log::warn!(
                    "selftest drop guard: released over process {process}, which is not allowed; cancelled"
                );
                return DRAGDROP_S_CANCEL;
            }
        }

        DRAGDROP_S_DROP
    }

    fn GiveFeedback(&self, _effect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

fn process_under_cursor() -> u32 {
    let mut point = POINT::default();
    if unsafe { GetCursorPos(&mut point) }.is_err() {
        return 0;
    }
    let window = unsafe { GetAncestor(WindowFromPoint(point), GA_ROOT) };
    let mut process = 0;
    unsafe { GetWindowThreadProcessId(window, Some(&mut process)) };

    process
}

// ------------------------------------------------------------------------------------ 自拖过滤

/// 包住 GPUI 窗口的 `IDropTarget`：本进程拖出时只转发给拖影 helper、回 NONE，GPUI 看不到这次
/// 拖拽；其余时候（别的应用拖进来）原样转发。
#[implement(IDropTarget)]
struct FilterDropTarget {
    inner: IDropTarget,
    helper: IDropTargetHelper,
    window: HWND,
    filtering: AtomicBool,
}

impl IDropTarget_Impl for FilterDropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        let own = crate::drag_out::is_active();
        self.filtering.store(own, Ordering::SeqCst);
        if !own {
            return unsafe { self.inner.DragEnter(data.as_ref(), keys, *point, effect) };
        }
        unsafe {
            *effect = DROPEFFECT_NONE;
            let point = POINT {
                x: point.x,
                y: point.y,
            };
            let _ = self
                .helper
                .DragEnter(self.window, data.as_ref(), &point, DROPEFFECT_NONE);
        }
        Ok(())
    }

    fn DragOver(
        &self,
        keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        if !self.filtering.load(Ordering::SeqCst) {
            return unsafe { self.inner.DragOver(keys, *point, effect) };
        }
        unsafe {
            *effect = DROPEFFECT_NONE;
            let point = POINT {
                x: point.x,
                y: point.y,
            };
            let _ = self.helper.DragOver(&point, DROPEFFECT_NONE);
        }
        Ok(())
    }

    fn DragLeave(&self) -> windows::core::Result<()> {
        if !self.filtering.swap(false, Ordering::SeqCst) {
            return unsafe { self.inner.DragLeave() };
        }
        let _ = unsafe { self.helper.DragLeave() };
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        keys: MODIFIERKEYS_FLAGS,
        point: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> windows::core::Result<()> {
        if !self.filtering.swap(false, Ordering::SeqCst) {
            return unsafe { self.inner.Drop(data.as_ref(), keys, *point, effect) };
        }
        unsafe {
            *effect = DROPEFFECT_NONE;
            let point = POINT {
                x: point.x,
                y: point.y,
            };
            let _ = self.helper.Drop(data.as_ref(), &point, DROPEFFECT_NONE);
        }
        log::debug!("own drag-out released on the panel; not forwarded to GPUI");
        Ok(())
    }
}

/// 给窗口装自拖过滤层（每个窗口一次）。失败只记日志：拖出照常进行，只是自拖会被 GPUI 收到。
fn install_filter(window: isize) {
    let mut filtered = FILTERED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if filtered.contains(&window) {
        return;
    }
    match wrap_drop_target(HWND(window as *mut c_void)) {
        Ok(()) => filtered.push(window),
        Err(err) => log::warn!("drag-out self-drop filter not installed: {err}"),
    }
}

fn wrap_drop_target(window: HWND) -> windows::core::Result<()> {
    let raw = unsafe { GetPropW(window, w!("OleDropTargetInterface")) };
    if raw.is_invalid() {
        return Err(WinError::new(
            E_NOTIMPL,
            "the window has no OLE drop target",
        ));
    }
    let raw = raw.0;
    let inner = unsafe { IDropTarget::from_raw_borrowed(&raw) }
        .cloned()
        .ok_or_else(|| WinError::new(E_NOTIMPL, "the OLE drop target is null"))?;
    let helper: IDropTargetHelper =
        unsafe { CoCreateInstance(&CLSID_DragDropHelper, None, CLSCTX_INPROC_SERVER) }?;
    let wrapper: IDropTarget = FilterDropTarget {
        inner,
        helper,
        window,
        filtering: AtomicBool::new(false),
    }
    .into();

    unsafe {
        RevokeDragDrop(window)?;
        RegisterDragDrop(window, &wrapper)
    }
}

// ------------------------------------------------------------------------------------ 文本数据对象

fn register_format(name: &str, slot: &'static OnceLock<u16>) -> u16 {
    *slot.get_or_init(|| {
        let wide: Vec<u16> = name.encode_utf16().chain(once(0)).collect();
        unsafe { RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) as u16 }
    })
}

fn cf_html() -> u16 {
    register_format("HTML Format", &CF_HTML)
}

fn cf_rtf() -> u16 {
    register_format("Rich Text Format", &CF_RTF)
}

/// 按 CF_HTML 规范构造 payload：ASCII 十进制偏移头（固定 10 位宽，先占位再回填）+ 片段注释。
fn build_cf_html_payload(html: &str) -> Vec<u8> {
    const HEADER: &str = "Version:0.9\r\nStartHTML:0000000000\r\nEndHTML:0000000000\r\nStartFragment:0000000000\r\nEndFragment:0000000000\r\n";
    const PREFIX: &str = "<html><body>\r\n<!--StartFragment-->";
    const SUFFIX: &str = "<!--EndFragment-->\r\n</body></html>";

    let mut buf = String::with_capacity(HEADER.len() + PREFIX.len() + html.len() + SUFFIX.len());
    buf.push_str(HEADER);
    let start_html = buf.len();
    buf.push_str(PREFIX);
    let start_fragment = buf.len();
    buf.push_str(html);
    let end_fragment = buf.len();
    buf.push_str(SUFFIX);
    let end_html = buf.len();

    for (label, value) in [
        ("StartHTML", start_html),
        ("EndHTML", end_html),
        ("StartFragment", start_fragment),
        ("EndFragment", end_fragment),
    ] {
        let needle = format!("{label}:0000000000");
        if let Some(pos) = buf.find(&needle) {
            buf.replace_range(pos..pos + needle.len(), &format!("{label}:{value:010}"));
        }
    }

    buf.into_bytes()
}

fn to_ansi(text: &[u16]) -> Option<Vec<u8>> {
    let len = unsafe { WideCharToMultiByte(CP_ACP, 0, text, None, PCSTR::null(), None) };
    if len <= 0 {
        return None;
    }
    let mut buf = vec![0u8; len as usize];
    let written =
        unsafe { WideCharToMultiByte(CP_ACP, 0, text, Some(&mut buf), PCSTR::null(), None) };

    (written == len).then_some(buf)
}

fn hglobal_format(format: u16) -> FORMATETC {
    FORMATETC {
        cfFormat: format,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    }
}

fn bytes_to_medium(bytes: &[u8]) -> windows::core::Result<STGMEDIUM> {
    unsafe {
        let handle = GlobalAlloc(GMEM_FIXED, bytes.len())?;
        let target = GlobalLock(handle).cast::<u8>();
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
        let _ = GlobalUnlock(handle);
        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: handle },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        })
    }
}

/// 复制一份 HGLOBAL 介质（拖影 helper 经 `SetData` 存进来的私有格式，按需取出）。
fn clone_hglobal_medium(source: &STGMEDIUM) -> windows::core::Result<STGMEDIUM> {
    unsafe {
        let handle = source.u.hGlobal;
        let size = GlobalSize(handle);
        let from = GlobalLock(handle).cast::<u8>();
        if from.is_null() {
            return Err(WinError::from_hresult(E_NOTIMPL));
        }
        let bytes = std::slice::from_raw_parts(from, size).to_vec();
        let _ = GlobalUnlock(handle);
        bytes_to_medium(&bytes)
    }
}

/// `SetData` 存进来的条目（拖影 helper 的 `DragImageBits`、`DragContext` 等）。
struct StoredEntry {
    format: FORMATETC,
    medium: STGMEDIUM,
}

impl Drop for StoredEntry {
    fn drop(&mut self) {
        unsafe { ReleaseStgMedium(&mut self.medium) };
    }
}

#[implement(IDataObject)]
struct RichDataObject {
    text_utf16: Vec<u16>,
    text_ansi: Option<Vec<u8>>,
    html: Option<Vec<u8>>,
    rtf: Option<Vec<u8>>,
    stored: Mutex<Vec<StoredEntry>>,
}

impl RichDataObject {
    fn new(plain: &str, html: Option<&str>, rtf: Option<&str>) -> Self {
        let text_utf16: Vec<u16> = plain.encode_utf16().chain(once(0)).collect();
        // HTML、RTF 与剪贴板惯例一致带结尾 NUL：有的接收方按 C 字符串读，不看介质大小。
        let nul_terminated = |text: &[u8]| text.iter().copied().chain(once(0)).collect::<Vec<u8>>();
        Self {
            text_ansi: to_ansi(&text_utf16),
            text_utf16,
            html: html.map(|html| nul_terminated(&build_cf_html_payload(html))),
            rtf: rtf.map(|rtf| nul_terminated(rtf.as_bytes())),
            stored: Mutex::new(Vec::new()),
        }
    }

    /// 由富到简：RTF、HTML、Unicode 文本、ANSI 文本。
    fn formats(&self) -> Vec<u16> {
        let mut formats = Vec::with_capacity(4);
        if self.rtf.is_some() {
            formats.push(cf_rtf());
        }
        if self.html.is_some() {
            formats.push(cf_html());
        }
        formats.push(CF_UNICODETEXT.0);
        if self.text_ansi.is_some() {
            formats.push(CF_TEXT.0);
        }
        formats
    }

    fn supported(&self, format: *const FORMATETC) -> Option<u16> {
        let format = unsafe { format.as_ref()? };
        if format.tymed & TYMED_HGLOBAL.0 as u32 == 0 || format.dwAspect != DVASPECT_CONTENT.0 {
            return None;
        }
        self.formats()
            .contains(&format.cfFormat)
            .then_some(format.cfFormat)
    }

    fn medium_for(&self, format: u16) -> windows::core::Result<STGMEDIUM> {
        let bytes: Option<&[u8]> = if format == CF_UNICODETEXT.0 {
            Some(unsafe {
                std::slice::from_raw_parts(
                    self.text_utf16.as_ptr().cast::<u8>(),
                    self.text_utf16.len() * 2,
                )
            })
        } else if format == CF_TEXT.0 {
            self.text_ansi.as_deref()
        } else if format == cf_html() {
            self.html.as_deref()
        } else if format == cf_rtf() {
            self.rtf.as_deref()
        } else {
            None
        };

        match bytes {
            Some(bytes) => bytes_to_medium(bytes),
            None => Err(WinError::from_hresult(DV_E_FORMATETC)),
        }
    }

    fn stored(&self) -> windows::core::Result<std::sync::MutexGuard<'_, Vec<StoredEntry>>> {
        self.stored
            .lock()
            .map_err(|_| WinError::from_hresult(E_NOTIMPL))
    }
}

impl IDataObject_Impl for RichDataObject_Impl {
    fn GetData(&self, format: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        if let Some(format) = self.supported(format) {
            return self.medium_for(format);
        }
        let Some(request) = (unsafe { format.as_ref() }) else {
            return Err(WinError::from_hresult(DV_E_FORMATETC));
        };
        let stored = self.stored()?;
        let entry = stored.iter().find(|entry| {
            entry.format.cfFormat == request.cfFormat
                && entry.format.tymed & request.tymed != 0
                && entry.format.dwAspect == request.dwAspect
        });
        match entry {
            Some(entry) if entry.medium.tymed == TYMED_HGLOBAL.0 as u32 => {
                clone_hglobal_medium(&entry.medium)
            }
            _ => Err(WinError::from_hresult(DV_E_FORMATETC)),
        }
    }

    fn GetDataHere(
        &self,
        _format: *const FORMATETC,
        _medium: *mut STGMEDIUM,
    ) -> windows::core::Result<()> {
        Err(WinError::from_hresult(E_NOTIMPL))
    }

    fn QueryGetData(&self, format: *const FORMATETC) -> HRESULT {
        if self.supported(format).is_some() {
            return S_OK;
        }
        let Some(request) = (unsafe { format.as_ref() }) else {
            return DV_E_FORMATETC;
        };
        let known = self.stored().is_ok_and(|stored| {
            stored.iter().any(|entry| {
                entry.format.cfFormat == request.cfFormat
                    && entry.format.dwAspect == request.dwAspect
            })
        });

        if known { S_OK } else { DV_E_FORMATETC }
    }

    fn GetCanonicalFormatEtc(&self, _input: *const FORMATETC, output: *mut FORMATETC) -> HRESULT {
        if let Some(output) = unsafe { output.as_mut() } {
            output.ptd = std::ptr::null_mut();
        }
        E_NOTIMPL
    }

    fn SetData(
        &self,
        format: *const FORMATETC,
        medium: *const STGMEDIUM,
        release: BOOL,
    ) -> windows::core::Result<()> {
        if !release.as_bool() {
            return Err(WinError::from_hresult(E_NOTIMPL));
        }
        let format = unsafe { format.as_ref() }
            .copied()
            .ok_or_else(|| WinError::from_hresult(E_NOTIMPL))?;
        let medium = match unsafe { medium.as_ref() } {
            // 调用方把介质的所有权交给我们（fRelease = TRUE），按位接过来，由 StoredEntry 释放。
            Some(medium) => unsafe { std::ptr::read(medium) },
            None => return Err(WinError::from_hresult(E_NOTIMPL)),
        };
        let mut stored = self.stored()?;
        stored.retain(|entry| {
            !(entry.format.cfFormat == format.cfFormat && entry.format.dwAspect == format.dwAspect)
        });
        stored.push(StoredEntry { format, medium });
        Ok(())
    }

    fn EnumFormatEtc(&self, direction: u32) -> windows::core::Result<IEnumFORMATETC> {
        if direction != DATADIR_GET.0 as u32 {
            return Err(WinError::from_hresult(E_NOTIMPL));
        }
        let formats: Vec<FORMATETC> = self.formats().into_iter().map(hglobal_format).collect();
        unsafe { SHCreateStdEnumFmtEtc(&formats) }
    }

    fn DAdvise(
        &self,
        _format: *const FORMATETC,
        _flags: u32,
        _sink: Ref<IAdviseSink>,
    ) -> windows::core::Result<u32> {
        Err(WinError::from_hresult(OLE_E_ADVISENOTSUPPORTED))
    }

    fn DUnadvise(&self, _connection: u32) -> windows::core::Result<()> {
        Err(WinError::from_hresult(OLE_E_ADVISENOTSUPPORTED))
    }

    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(WinError::from_hresult(OLE_E_ADVISENOTSUPPORTED))
    }
}

#[cfg(test)]
mod tests {
    use windows::Win32::System::Com::TYMED_ISTREAM;

    use super::*;

    /// 按接收方的方式把某个格式的字节读出来。
    fn read_bytes(data: &IDataObject, format: &FORMATETC) -> Vec<u8> {
        unsafe {
            let mut medium = data.GetData(format).expect("GetData");
            let size = GlobalSize(medium.u.hGlobal);
            let ptr = GlobalLock(medium.u.hGlobal).cast::<u8>();
            let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
            let _ = GlobalUnlock(medium.u.hGlobal);
            ReleaseStgMedium(&mut medium);
            bytes
        }
    }

    fn enumerated(data: &IDataObject) -> Vec<FORMATETC> {
        let mut fetched = [FORMATETC::default(); 8];
        let mut count = 0u32;
        unsafe {
            let enumerator = data
                .EnumFormatEtc(DATADIR_GET.0 as u32)
                .expect("EnumFormatEtc");
            let _ = enumerator.Next(&mut fetched, Some(&mut count));
        }
        fetched[..count as usize].to_vec()
    }

    #[test]
    fn enumerates_formats_richest_first() {
        let data: IDataObject =
            RichDataObject::new("hi", Some("<b>hi</b>"), Some("{\\rtf1 hi}")).into();
        let formats = enumerated(&data);

        assert_eq!(
            formats.iter().map(|f| f.cfFormat).collect::<Vec<_>>(),
            vec![cf_rtf(), cf_html(), CF_UNICODETEXT.0, CF_TEXT.0]
        );
        assert!(formats.iter().all(|f| f.tymed == TYMED_HGLOBAL.0 as u32));
    }

    #[test]
    fn matches_tymed_as_a_bitmask() {
        let data: IDataObject = RichDataObject::new("hi", None, None).into();
        let mut format = hglobal_format(CF_UNICODETEXT.0);

        format.tymed = (TYMED_HGLOBAL.0 | TYMED_ISTREAM.0) as u32;
        assert_eq!(unsafe { data.QueryGetData(&format) }, S_OK);
        format.tymed = TYMED_ISTREAM.0 as u32;
        assert_eq!(unsafe { data.QueryGetData(&format) }, DV_E_FORMATETC);
    }

    #[test]
    fn provides_unicode_ansi_html_and_rtf() {
        let data: IDataObject =
            RichDataObject::new("hello", Some("<b>hello</b>"), Some("{\\rtf1 hello}")).into();

        assert_eq!(read_bytes(&data, &hglobal_format(CF_TEXT.0)), b"hello\0");
        let unicode: Vec<u8> = "hello\0"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(
            read_bytes(&data, &hglobal_format(CF_UNICODETEXT.0)),
            unicode
        );
        let html = String::from_utf8(read_bytes(&data, &hglobal_format(cf_html()))).expect("utf-8");
        assert!(html.starts_with("Version:0.9\r\nStartHTML:"));
        assert!(html.contains("<!--StartFragment--><b>hello</b><!--EndFragment-->"));
        assert!(html.ends_with("</body></html>\0"));
        assert_eq!(
            read_bytes(&data, &hglobal_format(cf_rtf())),
            b"{\\rtf1 hello}\0"
        );
    }

    #[test]
    fn cf_html_offsets_point_at_the_fragment() {
        let payload = String::from_utf8(build_cf_html_payload("<i>x</i>")).expect("utf-8");
        let offset = |label: &str| -> usize {
            let start = payload.find(&format!("{label}:")).expect(label) + label.len() + 1;
            payload[start..start + 10].parse().expect("offset")
        };

        assert_eq!(
            &payload[offset("StartFragment")..offset("EndFragment")],
            "<i>x</i>"
        );
        assert!(payload[offset("StartHTML")..].starts_with("<html>"));
        assert_eq!(offset("EndHTML"), payload.len());
    }

    #[test]
    fn files_object_offers_cf_hdrop() {
        // Shell 的数据对象要 COM（测试线程上没有 GPUI 替我们 OleInitialize）。
        let _ = unsafe {
            windows::Win32::System::Com::CoInitializeEx(
                None,
                windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
            )
        };
        let dir = std::env::temp_dir().join(format!("kwikpaste-drag-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("a.txt");
        std::fs::write(&file, "a").expect("file");

        let data = files_object(std::slice::from_ref(&file)).expect("files object");
        let hdrop = hglobal_format(15);
        assert_eq!(unsafe { data.QueryGetData(&hdrop) }, S_OK);
        drop(data);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_files_and_empty_text_are_rejected() {
        assert!(files_object(&[]).is_err());
        assert!(
            data_object(&DragData::Text {
                plain: String::new(),
                html: None,
                rtf: None
            })
            .is_err()
        );
    }
}
