use std::fs;
use std::io::ErrorKind;

use ck_protocol::StickConfig;

use crate::error::Error;
use crate::root::Root;

pub(crate) const EFI_CONFIG: &str = "/boot/efi/ck/config.json";

/// The settings read from the EFI partition, checked and ready to apply.
pub struct Loaded {
    pub config: StickConfig,
    /// The file exactly as read, kept so its hash and the root-only copy match it byte for byte.
    pub raw: Vec<u8>,
    pub sha256: String,
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loaded")
            .field("stick_id", &self.config.stick_id)
            .field("sha256", &self.sha256)
            .finish_non_exhaustive()
    }
}

/// Reads and validates `/boot/efi/ck/config.json`.
///
/// # Errors
/// [`Error::SettingsMissing`] when the file does not exist, otherwise when it cannot be read,
/// is not valid or has problems.
pub fn load(root: &Root) -> Result<Loaded, Error> {
    let raw = match fs::read(root.path(EFI_CONFIG)) {
        Ok(raw) => raw,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(Error::SettingsMissing(EFI_CONFIG.to_owned()));
        }
        Err(e) => return Err(Error::io(format!("reading {EFI_CONFIG}"), e)),
    };
    let config: StickConfig =
        serde_json::from_slice(&raw).map_err(|e| Error::SettingsUnreadable(e.to_string()))?;
    let problems = config.validate();
    if !problems.is_empty() {
        return Err(Error::SettingsInvalid(problems));
    }
    let sha256 = ck_crypto::sha256_hex(&raw);
    Ok(Loaded {
        config,
        raw,
        sha256,
    })
}
