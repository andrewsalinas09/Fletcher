//! Audio device enumeration and default-device switching.
//!
//! Switching uses IPolicyConfig — undocumented but stable since Vista and the
//! mechanism every switcher tool (and Peace) relies on; there is no public API.

use std::sync::Once;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::core::{GUID, HRESULT, IUnknown, IUnknown_Vtbl, PCWSTR, interface};

fn ensure_com() {
    static COM: Once = Once::new();
    COM.call_once(|| {
        // Per-process MTA init; harmless if the thread is already initialized.
        let _ = wasapi::initialize_mta();
    });
}

/// Friendly name of the default render (playback) device, if resolvable.
pub fn default_render_device_name() -> Option<String> {
    ensure_com();
    let enumerator = wasapi::DeviceEnumerator::new().ok()?;
    let device = enumerator
        .get_default_device(&wasapi::Direction::Render)
        .ok()?;
    device.get_friendlyname().ok()
}

#[derive(Debug, Clone)]
pub struct RenderDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// All active render devices, default flagged.
pub fn list_render_devices() -> Vec<RenderDevice> {
    ensure_com();
    let Ok(enumerator) = wasapi::DeviceEnumerator::new() else {
        return Vec::new();
    };
    let default_id = enumerator
        .get_default_device(&wasapi::Direction::Render)
        .ok()
        .and_then(|d| d.get_id().ok());
    let Ok(collection) = enumerator.get_device_collection(&wasapi::Direction::Render) else {
        return Vec::new();
    };
    (&collection)
        .into_iter()
        .filter_map(|dev| {
            let dev = dev.ok()?;
            let id = dev.get_id().ok()?;
            Some(RenderDevice {
                name: dev.get_friendlyname().ok()?,
                is_default: default_id.as_deref() == Some(id.as_str()),
                id,
            })
        })
        .collect()
}

// The PolicyConfig COM class. Vtable order matters; earlier methods are
// declared as opaque stubs we never call.
const CLSID_POLICY_CONFIG: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

#[allow(non_snake_case, clippy::too_many_arguments)]
#[interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: IUnknown {
    unsafe fn GetMixFormat(&self, dev: PCWSTR, fmt: *mut *mut core::ffi::c_void) -> HRESULT;
    unsafe fn GetDeviceFormat(
        &self,
        dev: PCWSTR,
        default: i32,
        fmt: *mut *mut core::ffi::c_void,
    ) -> HRESULT;
    unsafe fn ResetDeviceFormat(&self, dev: PCWSTR) -> HRESULT;
    unsafe fn SetDeviceFormat(
        &self,
        dev: PCWSTR,
        endpoint: *mut core::ffi::c_void,
        mix: *mut core::ffi::c_void,
    ) -> HRESULT;
    unsafe fn GetProcessingPeriod(
        &self,
        dev: PCWSTR,
        default: i32,
        def_period: *mut i64,
        min_period: *mut i64,
    ) -> HRESULT;
    unsafe fn SetProcessingPeriod(&self, dev: PCWSTR, period: *mut i64) -> HRESULT;
    unsafe fn GetShareMode(&self, dev: PCWSTR, mode: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn SetShareMode(&self, dev: PCWSTR, mode: *mut core::ffi::c_void) -> HRESULT;
    unsafe fn GetPropertyValue(
        &self,
        dev: PCWSTR,
        store: i32,
        key: *const core::ffi::c_void,
        value: *mut core::ffi::c_void,
    ) -> HRESULT;
    unsafe fn SetPropertyValue(
        &self,
        dev: PCWSTR,
        store: i32,
        key: *const core::ffi::c_void,
        value: *const core::ffi::c_void,
    ) -> HRESULT;
    unsafe fn SetDefaultEndpoint(&self, dev: PCWSTR, role: u32) -> HRESULT;
    unsafe fn SetEndpointVisibility(&self, dev: PCWSTR, visible: i32) -> HRESULT;
}

/// Make `id` the Windows default render device for all roles.
pub fn set_default_render_device(id: &str) -> Result<(), String> {
    ensure_com();
    let policy: IPolicyConfig = unsafe { CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL) }
        .map_err(|e| format!("PolicyConfig unavailable: {e}"))?;
    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    // eConsole = 0, eMultimedia = 1, eCommunications = 2
    for role in [0u32, 1, 2] {
        unsafe { policy.SetDefaultEndpoint(PCWSTR(wide.as_ptr()), role) }
            .ok()
            .map_err(|e| format!("could not set default device: {e}"))?;
    }
    Ok(())
}
