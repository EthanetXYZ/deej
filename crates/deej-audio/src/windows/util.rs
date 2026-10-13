//! Small Win32 helpers for the engine: strings, process names and error classification.

use windows::Win32::Media::Audio::{AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_SERVICE_NOT_RUNNING};
use windows::Win32::System::Com::CoTaskMemFree;
use windows_core::{HRESULT, PCWSTR, PWSTR};

/// Converts a COM-allocated string to a `String` and frees it.
pub fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }

    // SAFETY: COM returned a valid, NUL-terminated, CoTaskMemAlloc'd string that we now own
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Copies a string passed into a callback (owned by the caller).
pub fn pcwstr_to_string(p: &PCWSTR) -> String {
    if p.is_null() {
        return String::new();
    }

    // SAFETY: the caller passes a valid NUL-terminated string for the duration of the call
    unsafe { p.to_string().unwrap_or_default() }
}

pub use deej_platform::process::{exe_path, process_names};

/// How the engine should react to a failed Core Audio call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// The device or session went away: re-scan devices.
    DeviceGone,
    /// The audio service stopped or restarted: drop everything and reconnect.
    ServiceGone,
    /// Anything else: log it and carry on.
    Other,
}

// HRESULT_FROM_WIN32(ERROR_NOT_FOUND): the device id no longer exists
const E_NOTFOUND: HRESULT = HRESULT(0x8007_0490_u32 as i32);
const RPC_E_DISCONNECTED: HRESULT = HRESULT(0x8001_0108_u32 as i32);
const RPC_S_SERVER_UNAVAILABLE: HRESULT = HRESULT(0x8007_06BA_u32 as i32);
const RPC_S_CALL_FAILED: HRESULT = HRESULT(0x8007_06BE_u32 as i32);

pub fn classify(err: &windows_core::Error) -> Failure {
    let code = err.code();

    if code == AUDCLNT_E_DEVICE_INVALIDATED || code == E_NOTFOUND {
        Failure::DeviceGone
    } else if code == AUDCLNT_E_SERVICE_NOT_RUNNING
        || code == RPC_E_DISCONNECTED
        || code == RPC_S_SERVER_UNAVAILABLE
        || code == RPC_S_CALL_FAILED
    {
        Failure::ServiceGone
    } else {
        Failure::Other
    }
}
