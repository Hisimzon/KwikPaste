//! 输入法上下文的读取与切换，平台自测用（编辑态的输入法验收）。
//!
//! 只能对调用线程自己的窗口使用（IMM 的限制）；面板在 GPUI 主线程上，调用方在主线程调用。

use std::ffi::c_void;

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Input::Ime::{
    CANDIDATEFORM, GCS_COMPSTR, IME_CMODE_NATIVE, IME_CONVERSION_MODE, IME_SENTENCE_MODE,
    ImmGetCandidateWindow, ImmGetCompositionStringW, ImmGetContext, ImmGetConversionStatus,
    ImmGetOpenStatus, ImmReleaseContext, ImmSetConversionStatus, ImmSetOpenStatus,
};

/// 窗口当前输入上下文的状态。没有上下文（GPUI 在没有文本焦点时会断开）时 `attached` 为 false。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImeState {
    pub attached: bool,
    pub open: bool,
    pub conversion: u32,
    pub composition: String,
    /// `ImmGetCandidateWindow(0)` 的样式与位置（客户区坐标），即应用告诉输入法的候选窗位置。
    pub candidate: Option<(u32, i32, i32)>,
}

/// 读取 `hwnd` 的输入上下文。
pub fn state(hwnd: isize) -> ImeState {
    let window = HWND(hwnd as *mut c_void);
    let context = unsafe { ImmGetContext(window) };
    if context.is_invalid() {
        return ImeState::default();
    }

    let open = unsafe { ImmGetOpenStatus(context) }.as_bool();
    let mut conversion = IME_CONVERSION_MODE(0);
    let mut sentence = IME_SENTENCE_MODE(0);
    let _ = unsafe { ImmGetConversionStatus(context, Some(&mut conversion), Some(&mut sentence)) };

    let bytes = unsafe { ImmGetCompositionStringW(context, GCS_COMPSTR, None, 0) };
    let composition = if bytes > 0 {
        let mut buffer = vec![0u16; bytes as usize / 2];
        unsafe {
            ImmGetCompositionStringW(
                context,
                GCS_COMPSTR,
                Some(buffer.as_mut_ptr().cast()),
                bytes as u32,
            )
        };
        String::from_utf16_lossy(&buffer)
    } else {
        String::new()
    };

    let mut form = CANDIDATEFORM::default();
    let candidate = unsafe { ImmGetCandidateWindow(context, 0, &mut form) }
        .as_bool()
        .then_some((form.dwStyle, form.ptCurrentPos.x, form.ptCurrentPos.y));
    let _ = unsafe { ImmReleaseContext(window, context) };

    ImeState {
        attached: true,
        open,
        conversion: conversion.0,
        composition,
        candidate,
    }
}

/// 打开输入法并切到中文（native）模式。微软拼音在新的输入上下文里默认是英文模式。
pub fn set_native_mode(hwnd: isize) -> bool {
    let window = HWND(hwnd as *mut c_void);
    let context = unsafe { ImmGetContext(window) };
    if context.is_invalid() {
        return false;
    }

    let mut conversion = IME_CONVERSION_MODE(0);
    let mut sentence = IME_SENTENCE_MODE(0);
    let switched = unsafe {
        let _ = ImmSetOpenStatus(context, true);
        let _ = ImmGetConversionStatus(context, Some(&mut conversion), Some(&mut sentence));
        ImmSetConversionStatus(context, conversion | IME_CMODE_NATIVE, sentence).as_bool()
    };
    let _ = unsafe { ImmReleaseContext(window, context) };
    switched
}
