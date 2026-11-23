use std::fs::File;
use std::io::{ErrorKind, Read};

use ck_protocol::{CONFIG_FILE, CONFIG_MOUNT_POINT, StickConfig};

use crate::error::Error;
use crate::root::Root;
use crate::trust::{self, Keys};

/// A real settings file is a few kilobytes; the partition is not trusted to say how big it is.
pub(crate) const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

/// The settings read from the stick, checked and ready to apply.
pub struct Loaded {
    pub config: StickConfig,
    /// The file exactly as read, kept so its hash and the root-only copy match it byte for byte.
    pub raw: Vec<u8>,
    pub sha256: String,
    /// Whether the settings were checked against the Hub's signature. An image that carries no key
    /// takes its settings as they are.
    pub verified: bool,
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loaded")
            .field("stick_id", &self.config.stick_id)
            .field("sha256", &self.sha256)
            .finish_non_exhaustive()
    }
}

/// Reads and validates `/ck-config/config.json`. A stick without it is
/// [`Error::SettingsMissing`], which callers treat differently from a bad file.
pub fn load(root: &Root) -> Result<Loaded, Error> {
    let keys = trust::load_keys(root)?;
    load_from(root, &format!("{CONFIG_MOUNT_POINT}/{CONFIG_FILE}"), &keys)
}

fn load_from(root: &Root, path: &str, keys: &Keys) -> Result<Loaded, Error> {
    let file = match File::open(root.path(path)) {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            return Err(Error::SettingsMissing(path.to_owned()));
        }
        Err(e) => return Err(Error::io(format!("opening {path}"), e)),
    };
    let mut raw = Vec::new();
    file.take(MAX_SETTINGS_BYTES + 1)
        .read_to_end(&mut raw)
        .map_err(|e| Error::io(format!("reading {path}"), e))?;
    if raw.len() as u64 > MAX_SETTINGS_BYTES {
        return Err(Error::SettingsTooLarge(MAX_SETTINGS_BYTES));
    }
    // Before anything is read out of the file: a refused file reaches nothing.
    if let Keys::Trusted(list) = keys {
        trust::verify(root, &raw, list)?;
        trust::same_stick(root, &raw)?;
    }
    let config = crate::enrolment::resolve(root, &raw, true)?.0;
    let problems = config.validate();
    if !problems.is_empty() {
        return Err(Error::SettingsInvalid(problems));
    }
    let sha256 = ck_crypto::sha256_hex(&raw);
    Ok(Loaded {
        config,
        raw,
        sha256,
        verified: matches!(keys, Keys::Trusted(_)),
    })
}
