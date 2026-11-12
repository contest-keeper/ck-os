use std::fs::File;
use std::io::{ErrorKind, Read};

use ck_protocol::StickConfig;

use crate::error::Error;
use crate::root::Root;

/// A real settings file is a few kilobytes; the partition is not trusted to say how big it is.
const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

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
    let file = match File::open(root.path(EFI_CONFIG)) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(Error::SettingsMissing(EFI_CONFIG.to_owned()));
        }
        Err(e) => return Err(Error::io(format!("opening {EFI_CONFIG}"), e)),
    };
    let mut raw = Vec::new();
    file.take(MAX_SETTINGS_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|e| Error::io(format!("reading {EFI_CONFIG}"), e))?;
    if raw.len() as u64 > MAX_SETTINGS_BYTES {
        return Err(Error::SettingsTooLarge(MAX_SETTINGS_BYTES));
    }
    let config: StickConfig =
        serde_json::from_slice(&raw).map_err(|e| Error::SettingsUnreadable(e.to_string()))?;
    let problems = config.validate();
    if !problems.is_empty() {
        return Err(Error::SettingsInvalid(problems));
    }
    // `validate` also refuses a team or site that is not one line, since both end up in one-line files.
    let sha256 = ck_crypto::sha256_hex(&raw);
    Ok(Loaded {
        config,
        raw,
        sha256,
    })
}
