//! Changing the default device. Windows has no public API for this; the Sound settings page and every tool that
//! does it (EarTrumpet, SoundSwitch, nircmd) use the undocumented `IPolicyConfig` interface, unchanged since
//! Windows 7 (ARCHITECTURE.md R7). If creating it fails, the error is returned and nothing else is affected.

// the COM methods keep their Windows names
#![allow(non_snake_case)]

use std::ffi::c_void;

use windows::Win32::Media::Audio::{ERole, eCommunications, eConsole, eMultimedia};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows_core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, PCWSTR, interface};

const CLSID_POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

// Only SetDefaultEndpoint is called. The methods before it are declared so that it sits at the right place in the
// vtable; their parameter types don't matter.
#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: IUnknown {
    fn GetMixFormat(&self, device: PCWSTR, format: *mut *mut c_void) -> HRESULT;
    fn GetDeviceFormat(&self, device: PCWSTR, default: i32, format: *mut *mut c_void) -> HRESULT;
    fn ResetDeviceFormat(&self, device: PCWSTR) -> HRESULT;
    fn SetDeviceFormat(&self, device: PCWSTR, endpoint: *mut c_void, mix: *mut c_void) -> HRESULT;
    fn GetProcessingPeriod(&self, device: PCWSTR, default: i32, period: *mut i64, min: *mut i64) -> HRESULT;
    fn SetProcessingPeriod(&self, device: PCWSTR, period: *mut i64) -> HRESULT;
    fn GetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
    fn SetShareMode(&self, device: PCWSTR, mode: *mut c_void) -> HRESULT;
    fn GetPropertyValue(&self, device: PCWSTR, key: *const c_void, value: *mut c_void) -> HRESULT;
    fn SetPropertyValue(&self, device: PCWSTR, key: *const c_void, value: *mut c_void) -> HRESULT;
    fn SetDefaultEndpoint(&self, device: PCWSTR, role: ERole) -> HRESULT;
    fn SetEndpointVisibility(&self, device: PCWSTR, visible: i32) -> HRESULT;
}

/// Makes `endpoint_id` the default device for every role (like the Sound settings page does). Must be called on a
/// thread with COM initialised.
pub fn set_default_endpoint(endpoint_id: &str) -> windows_core::Result<()> {
    let id: Vec<u16> = endpoint_id.encode_utf16().chain(Some(0)).collect();

    // SAFETY: COM is initialised on the calling (audio) thread; `id` outlives the calls
    unsafe {
        let policy: IPolicyConfig = CoCreateInstance(&CLSID_POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)?;
        for role in [eConsole, eMultimedia, eCommunications] {
            policy.SetDefaultEndpoint(PCWSTR(id.as_ptr()), role).ok()?;
        }
    }
    Ok(())
}
