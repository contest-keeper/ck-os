use std::fs::File;
use std::io::{ErrorKind, Read};

use ck_protocol::{CONFIG_FILE, CONFIG_MOUNT_POINT, ESP_CONFIG_FILE, StickConfig};

use crate::error::Error;
use crate::root::Root;
use crate::trust::{self, Keys};

/// A real settings file is a few kilobytes; the partition is not trusted to say how big it is.
pub(crate) const MAX_SETTINGS_BYTES: u64 = 1024 * 1024;

/// The old place of the settings, on the EFI partition. Read only when `CKCONFIG` has none, so
/// sticks made before the config partition existed keep working (one release).
pub(crate) const EFI_FOLDER: &str = "/boot/efi/ck";

/// Where the settings (and the wallpaper beside them) were found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The `CKCONFIG` partition, mounted at `/ck-config`.
    CkConfig,
    /// The EFI partition of a stick made before `CKCONFIG`.
    Esp,
}

impl Source {
    /// The folder that holds `config.json` and the wallpaper.
    #[must_use]
    pub fn folder(self) -> String {
        match self {
            Self::CkConfig => CONFIG_MOUNT_POINT.to_owned(),
            Self::Esp => EFI_FOLDER.to_owned(),
        }
    }
}

/// The settings read from the stick, checked and ready to apply.
pub struct Loaded {
    pub source: Source,
    pub config: StickConfig,
    /// The file exactly as read, kept so its hash and the root-only copy match it byte for byte.
    pub raw: Vec<u8>,
    pub sha256: String,
    /// Whether the settings were checked against the Hub's signature. An image that carries no key
    /// takes its settings as they are.
    pub verified: bool,
}

/// Read the legacy ESP only when the CKCONFIG partition or its settings file is missing.
pub fn read_with_esp_fallback<T, E>(
    mut read: impl FnMut(Source) -> Result<T, E>,
    is_missing: impl FnOnce(&E) -> bool,
) -> Result<T, E> {
    match read(Source::CkConfig) {
        Err(error) if is_missing(&error) => read(Source::Esp),
        result => result,
    }
}

impl std::fmt::Debug for Loaded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Loaded")
            .field("source", &self.source)
            .field("stick_id", &self.config.stick_id)
            .field("sha256", &self.sha256)
            .finish_non_exhaustive()
    }
}

/// Reads and validates `/ck-config/config.json`, or, on a stick without that partition,
/// `/boot/efi/ck/config.json`. A stick with neither is [`Error::SettingsMissing`], which callers
/// treat differently from a bad file. A `CKCONFIG` file that exists but is bad is an error: the
/// old copy is not used instead.
pub fn load(root: &Root) -> Result<Loaded, Error> {
    let keys = trust::load_keys(root)?;
    // An image that checks signatures never falls back to the unsigned legacy copy on the EFI
    // partition: deleting the file on CKCONFIG would otherwise let an unsigned one take its place.
    let may_fall_back = matches!(keys, Keys::Unchecked);
    let result = read_with_esp_fallback(
        |source| {
            let file_name = match source {
                Source::CkConfig => CONFIG_FILE,
                Source::Esp => ESP_CONFIG_FILE,
            };
            let path = format!("{}/{file_name}", source.folder());
            load_from(root, source, &path, &keys)
        },
        |error| may_fall_back && matches!(error, Error::SettingsMissing(_)),
    );
    match result {
        Err(Error::SettingsMissing(_)) => Err(Error::SettingsMissing(format!(
            "{CONFIG_MOUNT_POINT}/{CONFIG_FILE} or {EFI_FOLDER}/{ESP_CONFIG_FILE}"
        ))),
        other => other,
    }
}

fn load_from(root: &Root, source: Source, path: &str, keys: &Keys) -> Result<Loaded, Error> {
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
        trust::verify(root, source, &raw, list)?;
        trust::same_stick(root, &raw)?;
    }
    let config = if source == Source::CkConfig {
        crate::enrolment::resolve(root, &raw, true)?.0
    } else {
        serde_json::from_slice(&raw).map_err(|e| Error::SettingsUnreadable(e.to_string()))?
    };
    let problems = config.validate();
    if !problems.is_empty() {
        return Err(Error::SettingsInvalid(problems));
    }
    let sha256 = ck_crypto::sha256_hex(&raw);
    Ok(Loaded {
        source,
        config,
        raw,
        sha256,
        verified: matches!(keys, Keys::Trusted(_)),
    })
}
