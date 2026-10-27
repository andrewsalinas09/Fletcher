//! Equalizer APO installation discovery (spike-verified: one registry read).

use std::fmt;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApoInstall {
    pub install_path: PathBuf,
    pub config_path: PathBuf,
}

#[derive(Debug)]
pub enum ApoError {
    /// Registry key absent — Equalizer APO is not installed (TB-01).
    NotInstalled,
    /// Key exists but is unreadable/malformed.
    Registry(std::io::Error),
}

impl fmt::Display for ApoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApoError::NotInstalled => write!(f, "Equalizer APO is not installed"),
            ApoError::Registry(e) => write!(f, "failed reading Equalizer APO registry key: {e}"),
        }
    }
}

impl std::error::Error for ApoError {}

/// Locate Equalizer APO via `HKLM\SOFTWARE\EqualizerAPO`.
pub fn detect() -> Result<ApoInstall, ApoError> {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;

    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey("SOFTWARE\\EqualizerAPO")
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ApoError::NotInstalled
            } else {
                ApoError::Registry(e)
            }
        })?;
    let install_path: String = key.get_value("InstallPath").map_err(ApoError::Registry)?;
    let config_path: String = key
        .get_value("ConfigPath")
        .unwrap_or_else(|_| format!("{install_path}\\config"));
    Ok(ApoInstall {
        install_path: PathBuf::from(install_path),
        config_path: PathBuf::from(config_path),
    })
}

/// APO's two registered COM classes (`HKLM\SOFTWARE\Classes\CLSID`). Its
/// installer writes one of them into a device's FxProperties; a device whose
/// FxProperties name neither is a device APO never sees (TB-32).
const APO_CLSIDS: [&str; 2] = [
    "{EACD2258-FCAC-4FF4-B36D-419E924A6D79}", // EqualizerAPO Pre-Mix Class
    "{EC1CC9CE-FAED-4822-828A-82A81A6F018F}", // EqualizerAPO Post-Mix Class
];

/// PKEY_AudioEndpoint_Disable_SysFx (mmdeviceapi.h) — the per-device
/// "Disable all enhancements" switch. When 1, Windows loads no effects at all,
/// APO included, even if it's registered.
const DISABLE_SYSFX: &str = "{1da5d803-d492-4edd-8c23-e0c0ffee7f0e},5";

/// Whether Equalizer APO actually processes a given playback device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceApo {
    /// APO is registered on the device's effect chain.
    Active,
    /// APO is not on this device — every config write is inaudible here.
    /// Windows feature updates and driver reinstalls reset FxProperties and
    /// silently cause this (hit live 2026-10-26).
    Missing,
    /// Registered, but Windows' "Disable all enhancements" is on.
    SysFxDisabled,
    /// The endpoint's registry entry couldn't be read; claim nothing.
    Unknown,
}

impl DeviceApo {
    pub fn as_str(self) -> &'static str {
        match self {
            DeviceApo::Active => "active",
            DeviceApo::Missing => "missing",
            DeviceApo::SysFxDisabled => "disabled",
            DeviceApo::Unknown => "unknown",
        }
    }
}

/// The registry key name of a render endpoint: the GUID after the last `.` in
/// an MMDevice id like `{0.0.0.00000000}.{e411be18-…}`. Capture endpoints
/// (`{0.0.1.…}`) are rejected — Fletcher only ever EQs playback.
fn render_endpoint_guid(endpoint_id: &str) -> Option<&str> {
    let (flow, guid) = endpoint_id.rsplit_once('.')?;
    (flow.starts_with("{0.0.0.") && guid.starts_with('{') && guid.ends_with('}')).then_some(guid)
}

/// True if any FxProperties value names an APO class. Values are REG_SZ for
/// the classic SFX/MFX slots and REG_MULTI_SZ for Windows 8.1+ composite
/// slots — both are UTF-16, so decoding raw bytes covers either.
fn names_apo<'a>(raw_values: impl IntoIterator<Item = &'a [u8]>) -> bool {
    raw_values.into_iter().any(|bytes| {
        let wide: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&c| u16::from_le_bytes(c))
            .collect();
        let text = String::from_utf16_lossy(&wide).to_ascii_uppercase();
        APO_CLSIDS.iter().any(|clsid| text.contains(clsid))
    })
}

/// Read the device's registry entry and say whether APO is in its chain.
pub fn device_apo_status(endpoint_id: &str) -> DeviceApo {
    use winreg::RegKey;
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};

    let Some(guid) = render_endpoint_guid(endpoint_id) else {
        return DeviceApo::Unknown;
    };
    let path =
        format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\MMDevices\\Audio\\Render\\{guid}");
    let Ok(dev) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(&path, KEY_READ) else {
        return DeviceApo::Unknown;
    };
    // A device with no FxProperties at all simply has no effects — Missing.
    let registered = dev
        .open_subkey_with_flags("FxProperties", KEY_READ)
        .map(|fx| {
            let raws: Vec<Vec<u8>> = fx
                .enum_values()
                .filter_map(|v| v.ok())
                .map(|(_, v)| v.bytes.to_vec())
                .collect();
            names_apo(raws.iter().map(|b| b.as_slice()))
        })
        .unwrap_or(false);
    if !registered {
        return DeviceApo::Missing;
    }
    let sysfx_off = dev
        .open_subkey_with_flags("Properties", KEY_READ)
        .and_then(|p| p.get_value::<u32, _>(DISABLE_SYSFX))
        .is_ok_and(|v| v == 1);
    if sysfx_off {
        DeviceApo::SysFxDisabled
    } else {
        DeviceApo::Active
    }
}

/// APO's per-device installer: `DeviceSelector.exe` since 1.4, `Configurator.exe`
/// before. Whichever this install has.
pub fn device_selector_path(install: &ApoInstall) -> Option<PathBuf> {
    ["DeviceSelector.exe", "Configurator.exe"]
        .iter()
        .map(|n| install.install_path.join(n))
        .find(|p| p.exists())
}

/// Launch APO's device selector elevated (it writes HKLM, so it needs admin —
/// the UAC prompt is the user's consent) and block until it closes, so the
/// caller can re-check device status afterwards.
pub fn run_device_selector(install: &ApoInstall) -> Result<(), String> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{INFINITE, WaitForSingleObject};
    use windows::Win32::UI::Shell::{
        SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
    };
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::PCWSTR;

    let exe = device_selector_path(install).ok_or_else(|| {
        format!(
            "couldn't find APO's DeviceSelector.exe or Configurator.exe in {}",
            install.install_path.display()
        )
    })?;
    let wide = |s: &std::ffi::OsStr| -> Vec<u16> {
        use std::os::windows::ffi::OsStrExt;
        s.encode_wide().chain(Some(0)).collect()
    };
    let verb = wide("runas".as_ref());
    let file = wide(exe.as_os_str());
    let dir = wide(install.install_path.as_os_str());
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpDirectory: PCWSTR(dir.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: every pointer in `info` borrows a buffer that outlives the call.
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| {
        // ERROR_CANCELLED: the user said no at the UAC prompt.
        if e.code().0 as u32 == 0x8007_04C7 {
            "cancelled at the Windows admin prompt".to_string()
        } else {
            format!("couldn't start APO's device selector: {e}")
        }
    })?;
    if !info.hProcess.is_invalid() {
        // SAFETY: hProcess was returned to us by ShellExecuteExW (NOCLOSEPROCESS).
        unsafe {
            WaitForSingleObject(info.hProcess, INFINITE);
            let _ = CloseHandle(info.hProcess);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    #[test]
    fn endpoint_guid_takes_render_only() {
        assert_eq!(
            render_endpoint_guid("{0.0.0.00000000}.{e411be18-23e9-460d-9df3-bb467280999e}"),
            Some("{e411be18-23e9-460d-9df3-bb467280999e}")
        );
        assert_eq!(
            render_endpoint_guid("{0.0.1.00000000}.{e411be18-23e9-460d-9df3-bb467280999e}"),
            None
        );
        assert_eq!(render_endpoint_guid("garbage"), None);
    }

    #[test]
    fn apo_found_in_sz_and_multi_sz_any_case() {
        // Exactly what the Modi 3+ held after the reset: Microsoft's defaults only.
        let stock = utf16("{C18E2F7E-933D-4965-B7D1-1EEF228D2AF3}\0");
        assert!(!names_apo([stock.as_slice()]));
        let sz = utf16("{eacd2258-fcac-4ff4-b36d-419e924a6d79}\0");
        assert!(names_apo([stock.as_slice(), sz.as_slice()]));
        let multi = utf16(
            "{C18E2F7E-933D-4965-B7D1-1EEF228D2AF3}\0{EC1CC9CE-FAED-4822-828A-82A81A6F018F}\0\0",
        );
        assert!(names_apo([multi.as_slice()]));
    }

    /// Machine-dependent: only meaningful where APO is installed (the dev
    /// box). Run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "requires a local Equalizer APO installation"]
    fn detects_local_install() {
        let apo = detect().expect("APO installed on dev machine");
        assert!(apo.config_path.join("config.txt").exists());
    }
}
