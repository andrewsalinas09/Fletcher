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

const RENDER_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\MMDevices\\Audio\\Render";
/// PKEY_Device_DeviceDesc: the endpoint's own name ("Speakers").
const DEVICE_DESC: &str = "{a45c254e-df1c-4efd-8020-67d146a850e0},2";
/// PKEY_DeviceInterface_FriendlyName: the adapter's name ("Schiit Modi 3+").
const INTERFACE_NAME: &str = "{b3f8fa53-0004-438e-9003-51a46e139bfc},6";

fn endpoint_key(guid: &str) -> Option<winreg::RegKey> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(format!("{RENDER_KEY}\\{guid}"), KEY_READ)
        .ok()
}

/// The text APO matches `Device:` patterns against: "Device_name
/// Connection_name GUID" (APO configuration reference). Every word of a
/// pattern must be found in it.
pub fn device_match_text(endpoint_id: &str) -> Option<String> {
    let guid = render_endpoint_guid(endpoint_id)?;
    let props = endpoint_key(guid)?.open_subkey("Properties").ok()?;
    let get = |k: &str| props.get_value::<String, _>(k).unwrap_or_default();
    Some(format!(
        "{} {} {guid}",
        get(INTERFACE_NAME),
        get(DEVICE_DESC)
    ))
}

/// Whether a `Device:` line's patterns match `text`. A pattern matches when
/// all its space-separated words are found in the text; `;` separates
/// alternatives; "all" always matches. APO's docs don't say whether matching
/// ignores case; this does.
pub fn device_pattern_matches(pattern: &str, text: &str) -> bool {
    let text = text.to_lowercase();
    pattern.split(';').any(|alt| {
        let alt = alt.trim().to_lowercase();
        alt == "all" || (!alt.is_empty() && alt.split_whitespace().all(|w| text.contains(w)))
    })
}

/// Which of APO's two classes the device's effect chain names: (pre-mix,
/// post-mix). APO runs filters post-mix unless a `Stage:` line says
/// otherwise, so a device with only the pre-mix class plays nothing Fletcher
/// writes.
pub fn device_apo_stages(endpoint_id: &str) -> Option<(bool, bool)> {
    use winreg::enums::KEY_READ;
    let guid = render_endpoint_guid(endpoint_id)?;
    let fx = endpoint_key(guid)?
        .open_subkey_with_flags("FxProperties", KEY_READ)
        .ok()?;
    let raws: Vec<Vec<u8>> = fx
        .enum_values()
        .filter_map(|v| v.ok())
        .map(|(_, v)| v.bytes.to_vec())
        .collect();
    let has = |clsid: &str| {
        raws.iter()
            .any(|b| decode_utf16(b).to_ascii_uppercase().contains(clsid))
    };
    Some((has(APO_CLSIDS[0]), has(APO_CLSIDS[1])))
}

fn decode_utf16(bytes: &[u8]) -> String {
    let wide: Vec<u16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_le_bytes(c))
        .collect();
    String::from_utf16_lossy(&wide)
}

/// A COM class's registered name, e.g. an effect's CLSID → "Dolby Audio".
fn clsid_name(clsid: &str) -> Option<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CLASSES_ROOT;
    RegKey::predef(HKEY_CLASSES_ROOT)
        .open_subkey(format!("CLSID\\{clsid}"))
        .ok()?
        .get_value::<String, _>("")
        .ok()
        .filter(|s| !s.is_empty())
}

/// Every CLSID in `text`, each followed by its registered name.
fn annotate_clsids(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        match tail.find('}') {
            Some(end) if end == 37 => {
                let clsid = &tail[..=end];
                out.push_str(clsid);
                if let Some(name) = clsid_name(clsid) {
                    out.push_str(&format!(" ({name})"));
                }
                rest = &tail[end + 1..];
            }
            _ => {
                out.push('{');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn reg_value_text(v: &winreg::RegValue) -> String {
    use winreg::enums::RegType::*;
    match v.vtype {
        REG_SZ | REG_EXPAND_SZ | REG_MULTI_SZ => decode_utf16(&v.bytes)
            .trim_end_matches('\0')
            .replace('\0', " | "),
        REG_DWORD if v.bytes.len() >= 4 => {
            u32::from_le_bytes(v.bytes[..4].try_into().unwrap()).to_string()
        }
        _ => {
            let hex: Vec<String> = v
                .bytes
                .iter()
                .take(48)
                .map(|b| format!("{b:02x}"))
                .collect();
            format!(
                "{:?} {}{}",
                v.vtype,
                hex.join(""),
                if v.bytes.len() > 48 { "…" } else { "" }
            )
        }
    }
}

/// Every value under `HKLM\SOFTWARE\EqualizerAPO` and its subkeys, as text.
pub fn registry_dump() -> String {
    use winreg::RegKey;
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    fn walk(key: &winreg::RegKey, path: &str, out: &mut String) {
        out.push_str(&format!("[{path}]\n"));
        for (name, v) in key.enum_values().filter_map(|v| v.ok()) {
            out.push_str(&format!("  {name} = {}\n", reg_value_text(&v)));
        }
        for sub in key.enum_keys().filter_map(|k| k.ok()) {
            if let Ok(k) = key.open_subkey_with_flags(&sub, KEY_READ) {
                walk(&k, &format!("{path}\\{sub}"), out);
            }
        }
    }
    let mut out = String::new();
    match RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags("SOFTWARE\\EqualizerAPO", KEY_READ)
    {
        Ok(k) => walk(&k, "HKLM\\SOFTWARE\\EqualizerAPO", &mut out),
        Err(e) => out.push_str(&format!("HKLM\\SOFTWARE\\EqualizerAPO: {e}\n")),
    }
    out
}

/// Every playback endpoint Windows knows, active or not: its names, state,
/// "Disable all enhancements", whether APO is on it, and every effect in its
/// chain by name (other EQs and enhancement suites show up here).
pub fn render_endpoints_dump() -> String {
    use winreg::RegKey;
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    let mut out = String::new();
    let Ok(root) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags(RENDER_KEY, KEY_READ)
    else {
        return "render endpoints unreadable\n".into();
    };
    for guid in root.enum_keys().filter_map(|k| k.ok()) {
        let Ok(dev) = root.open_subkey_with_flags(&guid, KEY_READ) else {
            continue;
        };
        // DEVICE_STATE_*: 1 active, 2 disabled, 4 not present, 8 unplugged.
        let state = match dev.get_value::<u32, _>("DeviceState").unwrap_or(0) & 0xF {
            1 => "active",
            2 => "disabled",
            4 => "not present",
            8 => "unplugged",
            _ => "unknown",
        };
        let props = dev.open_subkey_with_flags("Properties", KEY_READ).ok();
        let get = |k: &str| {
            props
                .as_ref()
                .and_then(|p| p.get_value::<String, _>(k).ok())
                .unwrap_or_default()
        };
        let sysfx = props
            .as_ref()
            .and_then(|p| p.get_value::<u32, _>(DISABLE_SYSFX).ok());
        let id = format!("{{0.0.0.00000000}}.{guid}");
        out.push_str(&format!(
            "{} ({}) {guid}\n  state: {state}\n  APO: {}\n  Disable all enhancements: {}\n",
            get(DEVICE_DESC),
            get(INTERFACE_NAME),
            device_apo_status(&id).as_str(),
            sysfx.map_or("not set".to_string(), |v| v.to_string()),
        ));
        if let Ok(fx) = dev.open_subkey_with_flags("FxProperties", KEY_READ) {
            for (name, v) in fx.enum_values().filter_map(|v| v.ok()) {
                out.push_str(&format!(
                    "  fx {name} = {}\n",
                    annotate_clsids(&reg_value_text(&v))
                ));
            }
        } else {
            out.push_str("  fx: none\n");
        }
        out.push('\n');
    }
    out
}

/// Every value in one endpoint's Properties key (names, format, spatial
/// sound, enhancement switches), as text.
pub fn endpoint_properties_dump(endpoint_id: &str) -> String {
    use winreg::enums::KEY_READ;
    let Some(props) = render_endpoint_guid(endpoint_id)
        .and_then(endpoint_key)
        .and_then(|k| k.open_subkey_with_flags("Properties", KEY_READ).ok())
    else {
        return "properties unreadable\n".into();
    };
    let mut out = String::new();
    for (name, v) in props.enum_values().filter_map(|v| v.ok()) {
        out.push_str(&format!(
            "{name} = {}\n",
            annotate_clsids(&reg_value_text(&v))
        ));
    }
    out
}

/// APO's installed version, from its uninstall entry.
pub fn installed_version() -> Option<String> {
    use winreg::RegKey;
    use winreg::enums::HKEY_LOCAL_MACHINE;
    RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\EqualizerAPO")
        .ok()?
        .get_value::<String, _>("DisplayVersion")
        .ok()
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

    #[test]
    fn device_patterns_match_like_apo() {
        let text = "Schiit Modi 3+ Speakers {e411be18-23e9-460d-9df3-bb467280999e}";
        assert!(device_pattern_matches("all", text));
        assert!(device_pattern_matches("Modi Speakers", text));
        assert!(device_pattern_matches("speakers", text));
        assert!(device_pattern_matches("Realtek Headphones; Modi", text));
        assert!(!device_pattern_matches("Realtek Speakers", text));
        assert!(device_pattern_matches(
            "{e411be18-23e9-460d-9df3-bb467280999e}",
            text
        ));
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
