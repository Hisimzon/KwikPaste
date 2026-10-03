use anyhow::{Context, Result};
use gpui_util::ResultExt;
// [kwikpaste patch 0003] itertools no longer needed: one attempt per call.
use windows::Win32::{
    Foundation::HMODULE,
    Graphics::{
        Direct3D::{
            D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_1,
            D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
        },
        Direct3D11::{
            D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_CREATE_DEVICE_DEBUG,
            D3D11_FEATURE_D3D10_X_HARDWARE_OPTIONS, D3D11_FEATURE_DATA_D3D10_X_HARDWARE_OPTIONS,
            D3D11_SDK_VERSION, D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext,
        },
        Dxgi::{
            CreateDXGIFactory2, DXGI_CREATE_FACTORY_DEBUG, DXGI_CREATE_FACTORY_FLAGS,
            IDXGIAdapter1, IDXGIFactory6,
        },
    },
};
use windows::core::Interface;

// [kwikpaste patch 0003] One attempt per call: the vsync thread retries the whole recovery on the
// deadline schedule below instead of sleeping inside here (the old 5 tries x ~110 ms ran out in
// about 850 ms, then panicked).
pub(crate) fn try_to_recover_from_device_lost<T>(mut f: impl FnMut() -> Result<T>) -> Result<T> {
    f().context("DirectXRenderer failed to recover from lost device")
}

// [kwikpaste patch 0003] Device-loss recovery that never panics, its health signal, and selftest
// hooks. After a loss the vsync thread tries again at these offsets from the detection; after the
// last one it keeps trying once a second and reports `failing`.
pub(crate) const RECOVERY_SCHEDULE_MS: [u64; 8] = [0, 100, 250, 500, 1000, 2000, 4000, 8000];

static DEVICE_LOSSES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static DEVICE_RECOVERIES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static LAST_RECOVERY_MS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static RECOVERY_FAILING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RECHECK_REQUESTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static WINDOW_RECOVERY_FAILED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static SIMULATED_FAILURES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// [kwikpaste patch 0003] Device-loss counters for the app's health checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceLossStatus {
    /// Device losses detected (real, requested rechecks and simulated ones).
    pub losses: u32,
    /// Losses recovered from.
    pub recoveries: u32,
    /// Detection to recovered, for the latest recovery.
    pub last_recovery_ms: u32,
    /// Every scheduled attempt failed; the vsync thread keeps trying once a second.
    pub failing: bool,
}

/// [kwikpaste patch 0003] Current device-loss counters.
pub fn device_loss_status() -> DeviceLossStatus {
    use std::sync::atomic::Ordering::Acquire;
    DeviceLossStatus {
        losses: DEVICE_LOSSES.load(Acquire),
        recoveries: DEVICE_RECOVERIES.load(Acquire),
        last_recovery_ms: LAST_RECOVERY_MS.load(Acquire),
        failing: RECOVERY_FAILING.load(Acquire),
    }
}

/// [kwikpaste patch 0003] Rebuild the devices on the next vsync-thread tick (after a system wake,
/// a session switch, or to simulate a loss in selftests), as if the device had been lost.
pub fn request_device_recheck() {
    RECHECK_REQUESTED.store(true, std::sync::atomic::Ordering::Release);
    crate::vsync::wake_vsync_thread();
}

/// [kwikpaste patch 0003] Selftest: simulate a device loss whose first `failed_attempts`
/// recreations of the global device fail.
pub fn simulate_device_lost(failed_attempts: u32) {
    SIMULATED_FAILURES.store(failed_attempts, std::sync::atomic::Ordering::Release);
    request_device_recheck();
}

pub(crate) fn take_recheck_request() -> bool {
    RECHECK_REQUESTED.swap(false, std::sync::atomic::Ordering::AcqRel)
}

pub(crate) fn note_device_lost() {
    DEVICE_LOSSES.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
}

pub(crate) fn note_recovered(elapsed: std::time::Duration) {
    use std::sync::atomic::Ordering::Release;
    LAST_RECOVERY_MS.store(elapsed.as_millis().min(u32::MAX as u128) as u32, Release);
    RECOVERY_FAILING.store(false, Release);
    DEVICE_RECOVERIES.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
}

pub(crate) fn note_recovery_failing() {
    RECOVERY_FAILING.store(true, std::sync::atomic::Ordering::Release);
}

/// A window renderer could not be rebuilt on the new device; the vsync thread retries.
pub(crate) fn note_window_recovery_failed() {
    WINDOW_RECOVERY_FAILED.store(true, std::sync::atomic::Ordering::Release);
}

pub(crate) fn take_window_recovery_failed() -> bool {
    WINDOW_RECOVERY_FAILED.swap(false, std::sync::atomic::Ordering::AcqRel)
}

/// Offset from the detection of the `attempt`-th recovery attempt.
pub(crate) fn recovery_deadline_ms(attempt: usize) -> u64 {
    match RECOVERY_SCHEDULE_MS.get(attempt) {
        Some(ms) => *ms,
        None => {
            let last = RECOVERY_SCHEDULE_MS[RECOVERY_SCHEDULE_MS.len() - 1];
            last + 1000 * (attempt + 1 - RECOVERY_SCHEDULE_MS.len()) as u64
        }
    }
}

#[derive(Clone)]
pub(crate) struct DirectXDevices {
    pub(crate) adapter: IDXGIAdapter1,
    pub(crate) dxgi_factory: IDXGIFactory6,
    pub(crate) device: ID3D11Device,
    pub(crate) device_context: ID3D11DeviceContext,
}

impl DirectXDevices {
    pub(crate) fn new() -> Result<Self> {
        // [kwikpaste patch 0003] selftest: the adapter is gone for a few attempts.
        if SIMULATED_FAILURES
            .fetch_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |left| left.checked_sub(1),
            )
            .is_ok()
        {
            anyhow::bail!("simulated device loss: no adapter available");
        }
        let debug_layer_available = check_debug_layer_available();
        let dxgi_factory =
            get_dxgi_factory(debug_layer_available).context("Creating DXGI factory")?;
        let (adapter, device, device_context, feature_level) =
            get_adapter(&dxgi_factory, debug_layer_available).context("Getting DXGI adapter")?;
        match feature_level {
            D3D_FEATURE_LEVEL_11_1 => {
                log::info!("Created device with Direct3D 11.1 feature level.")
            }
            D3D_FEATURE_LEVEL_11_0 => {
                log::info!("Created device with Direct3D 11.0 feature level.")
            }
            D3D_FEATURE_LEVEL_10_1 => {
                log::info!("Created device with Direct3D 10.1 feature level.")
            }
            _ => unreachable!(),
        }

        Ok(Self {
            adapter,
            dxgi_factory,
            device,
            device_context,
        })
    }
}

#[inline]
fn check_debug_layer_available() -> bool {
    #[cfg(debug_assertions)]
    {
        use windows::Win32::Graphics::Dxgi::{DXGIGetDebugInterface1, IDXGIInfoQueue};

        unsafe { DXGIGetDebugInterface1::<IDXGIInfoQueue>(0) }
            .log_err()
            .is_some()
    }
    #[cfg(not(debug_assertions))]
    {
        false
    }
}

#[inline]
fn get_dxgi_factory(debug_layer_available: bool) -> Result<IDXGIFactory6> {
    let factory_flag = if debug_layer_available {
        DXGI_CREATE_FACTORY_DEBUG
    } else {
        #[cfg(debug_assertions)]
        log::warn!(
            "Failed to get DXGI debug interface. DirectX debugging features will be disabled."
        );
        DXGI_CREATE_FACTORY_FLAGS::default()
    };
    unsafe { Ok(CreateDXGIFactory2(factory_flag)?) }
}

#[inline]
fn get_adapter(
    dxgi_factory: &IDXGIFactory6,
    debug_layer_available: bool,
) -> Result<(
    IDXGIAdapter1,
    ID3D11Device,
    ID3D11DeviceContext,
    D3D_FEATURE_LEVEL,
)> {
    for adapter_index in 0.. {
        let adapter: IDXGIAdapter1 = unsafe { dxgi_factory.EnumAdapters(adapter_index)?.cast()? };
        if let Ok(desc) = unsafe { adapter.GetDesc1() } {
            let gpu_name = String::from_utf16_lossy(&desc.Description)
                .trim_matches(char::from(0))
                .to_string();
            log::info!("Using GPU: {}", gpu_name);
        }
        // Check to see whether the adapter supports Direct3D 11 and create
        // the device if it does.
        let mut context: Option<ID3D11DeviceContext> = None;
        let mut feature_level = D3D_FEATURE_LEVEL::default();
        if let Some(device) = get_device(
            &adapter,
            Some(&mut context),
            Some(&mut feature_level),
            debug_layer_available,
        )
        .log_err()
        {
            return Ok((adapter, device, context.unwrap(), feature_level));
        }
    }

    unreachable!()
}

#[inline]
fn get_device(
    adapter: &IDXGIAdapter1,
    context: Option<*mut Option<ID3D11DeviceContext>>,
    feature_level: Option<*mut D3D_FEATURE_LEVEL>,
    debug_layer_available: bool,
) -> Result<ID3D11Device> {
    let mut device: Option<ID3D11Device> = None;
    let device_flags = if debug_layer_available {
        D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_DEBUG
    } else {
        D3D11_CREATE_DEVICE_BGRA_SUPPORT
    };
    unsafe {
        D3D11CreateDevice(
            adapter,
            D3D_DRIVER_TYPE_UNKNOWN,
            HMODULE::default(),
            device_flags,
            // 4x MSAA is required for Direct3D Feature Level 10.1 or better
            Some(&[
                D3D_FEATURE_LEVEL_11_1,
                D3D_FEATURE_LEVEL_11_0,
                D3D_FEATURE_LEVEL_10_1,
            ]),
            D3D11_SDK_VERSION,
            Some(&mut device),
            feature_level,
            context,
        )?;
    }
    let device = device.unwrap();
    let mut data = D3D11_FEATURE_DATA_D3D10_X_HARDWARE_OPTIONS::default();
    unsafe {
        device
            .CheckFeatureSupport(
                D3D11_FEATURE_D3D10_X_HARDWARE_OPTIONS,
                &mut data as *mut _ as _,
                std::mem::size_of::<D3D11_FEATURE_DATA_D3D10_X_HARDWARE_OPTIONS>() as u32,
            )
            .context("Checking GPU device feature support")?;
    }
    if data
        .ComputeShaders_Plus_RawAndStructuredBuffers_Via_Shader_4_x
        .as_bool()
    {
        Ok(device)
    } else {
        Err(anyhow::anyhow!(
            "Required feature StructuredBuffer is not supported by GPU/driver"
        ))
    }
}
