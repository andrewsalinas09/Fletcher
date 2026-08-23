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

#[cfg(test)]
mod tests {
    use super::*;

    /// Machine-dependent: only meaningful where APO is installed (the dev
    /// box). Run with `cargo test -- --ignored`.
    #[test]
    #[ignore = "requires a local Equalizer APO installation"]
    fn detects_local_install() {
        let apo = detect().expect("APO installed on dev machine");
        assert!(apo.config_path.join("config.txt").exists());
    }
}
