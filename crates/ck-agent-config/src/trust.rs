//! Signed settings.
//!
//! The settings file on the `CKCONFIG` partition can be edited by anyone who holds the stick, on any
//! PC. An image that carries public keys (`/etc/ck/config-public-keys`) therefore only takes a
//! settings file whose signature the Hub made (`config.json.sig`), and refuses anything else: an
//! edited file, an unsigned one, one signed with another key, and
//! one that is signed but made for another stick than the one that is already set up.
//!
//! A refused file is ignored. The stick goes on with the last settings it verified, which are kept
//! root-only on its own system partition, and says so in every heartbeat. A stick that never had
//! verified settings starts with the image's own defaults.

use std::fs::File;
use std::io::{ErrorKind, Read};

use ck_crypto::{CONFIG_SIGNATURE_FILE, ConfigVerifyingKey, parse_verifying_keys, verify_config};
use ck_protocol::{CONFIG_MOUNT_POINT, CONFIG_PUBLIC_KEYS_PATH};
use serde::Deserialize;

use crate::apply::write;
use crate::error::Error;
use crate::load::{Loaded, MAX_SETTINGS_BYTES};
use crate::root::Root;

/// What the agent kept of the settings it applied at the last boot that could.
const APPLIED_COPY: &str = "/etc/ck/config.json";

/// What the stick made of its settings file, for the heartbeat to report.
pub const TRUST_MARKER: &str = "/etc/ck/config.trust";

const MAX_SIGNATURE_BYTES: u64 = 4096;
const MAX_KEY_LIST_BYTES: u64 = 64 * 1024;

/// The keys an image trusts to sign settings. Only a development build may have none, and then it
/// takes a settings file as it is.
pub(crate) enum Keys {
    Unchecked,
    Trusted(Vec<ConfigVerifyingKey>),
}

/// What the stick made of its settings file at this boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Trust {
    /// The file was signed by a trusted key and applied.
    Verified,
    /// A file was refused. This is the hash of the file that was refused.
    Rejected(String),
    /// The image carries no key, so the file was applied as it was.
    Unchecked,
}

impl Trust {
    fn marker_text(&self) -> String {
        match self {
            Self::Verified => "verified\n".to_owned(),
            Self::Rejected(sha256) => format!("rejected {sha256}\n"),
            Self::Unchecked => "unchecked\n".to_owned(),
        }
    }
}

/// Keeps what the stick made of its settings, for the heartbeat.
///
/// # Errors
/// [`Error::Io`] when the marker cannot be written.
pub fn write_trust(root: &Root, trust: &Trust) -> Result<(), Error> {
    write(root, TRUST_MARKER, trust.marker_text().as_bytes(), 0o644)
}

fn read_limited(root: &Root, path: &str, limit: u64) -> Result<Option<Vec<u8>>, Error> {
    let file = match File::open(root.path(path)) {
        Ok(file) => file,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::io(format!("opening {path}"), error)),
    };
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| Error::io(format!("reading {path}"), error))?;
    if bytes.len() as u64 > limit {
        return Err(Error::SettingsTooLarge(limit));
    }
    Ok(Some(bytes))
}

/// Reads the key list of the image. A list that cannot be read as keys trusts nothing, so every
/// settings file is refused: a damaged list must not turn the check off. A release build without a
/// list is an error; only a development build (debug assertions on) takes settings unchecked.
pub(crate) fn load_keys(root: &Root) -> Result<Keys, Error> {
    load_keys_with(root, !cfg!(debug_assertions))
}

fn load_keys_with(root: &Root, list_required: bool) -> Result<Keys, Error> {
    let Some(bytes) = read_limited(root, CONFIG_PUBLIC_KEYS_PATH, MAX_KEY_LIST_BYTES)? else {
        return if list_required {
            Err(Error::KeysMissing(CONFIG_PUBLIC_KEYS_PATH.to_owned()))
        } else {
            Ok(Keys::Unchecked)
        };
    };
    match parse_verifying_keys(&String::from_utf8_lossy(&bytes)) {
        Ok(keys) => Ok(Keys::Trusted(keys)),
        Err(error) => {
            tracing::error!(
                "the key list of this image is not valid ({error}); no settings file can be accepted"
            );
            Ok(Keys::Trusted(Vec::new()))
        }
    }
}

fn rejected(raw: &[u8], reason: impl Into<String>) -> Error {
    Error::SettingsRejected {
        sha256: ck_crypto::sha256_hex(raw),
        reason: reason.into(),
    }
}

/// Checks the exact bytes of a settings file against the signature next to it. This runs before
/// anything is read out of the file, so nothing in a refused file reaches the system.
pub(crate) fn verify(root: &Root, raw: &[u8], keys: &[ConfigVerifyingKey]) -> Result<(), Error> {
    let path = format!("{CONFIG_MOUNT_POINT}/{CONFIG_SIGNATURE_FILE}");
    let Some(text) = read_limited(root, &path, MAX_SIGNATURE_BYTES)? else {
        return Err(rejected(raw, "the settings are not signed"));
    };
    let text =
        std::str::from_utf8(&text).map_err(|_| rejected(raw, "the signature is not text"))?;
    verify_config(keys, raw, text).map_err(|error| rejected(raw, error.to_string()))
}

#[derive(Deserialize)]
struct Head {
    stick_id: String,
}

/// Refuses a settings file made for another stick than the one already set up here. A valid file of
/// another stick (a practice stick's, say) would otherwise pass the signature. A stick that has no
/// applied settings yet takes any signed file, and writing a stick again starts from a fresh system.
pub(crate) fn same_stick(root: &Root, raw: &[u8]) -> Result<(), Error> {
    let Ok(new) = serde_json::from_slice::<Head>(raw) else {
        return Ok(());
    };
    let Some(applied) = read_limited(root, APPLIED_COPY, MAX_SETTINGS_BYTES)? else {
        return Ok(());
    };
    match serde_json::from_slice::<Head>(&applied) {
        Ok(old) if old.stick_id != new.stick_id => {
            Err(rejected(raw, "the settings are signed for another stick"))
        }
        _ => Ok(()),
    }
}

/// The settings the agent applied at the last boot that could: what a refused file is replaced by.
/// `None` for a stick that never applied any.
///
/// # Errors
/// [`Error`] when the copy cannot be read.
pub fn load_last_good(root: &Root) -> Result<Option<Loaded>, Error> {
    let Some(raw) = read_limited(root, APPLIED_COPY, MAX_SETTINGS_BYTES)? else {
        return Ok(None);
    };
    let config = crate::enrolment::resolve(root, &raw, false)?.0;
    if !config.validate().is_empty() {
        return Ok(None);
    }
    let sha256 = ck_crypto::sha256_hex(&raw);
    Ok(Some(Loaded {
        config,
        raw,
        sha256,
        verified: true,
    }))
}

/// The settings to apply at this boot and what the stick made of its settings file.
pub struct Booted {
    pub loaded: Loaded,
    pub trust: Trust,
}

/// Reads the settings for this boot. A file the image refuses is replaced by the last settings that
/// were verified, when there are any; without them the refusal is the error.
///
/// # Errors
/// [`Error::SettingsRejected`] when the file was refused and nothing verified came before it, and
/// the errors of [`crate::load`] otherwise.
pub fn load_for_boot(root: &Root) -> Result<Booted, Error> {
    match crate::load::load(root) {
        Ok(loaded) => {
            let trust = if loaded.verified {
                Trust::Verified
            } else {
                Trust::Unchecked
            };
            Ok(Booted { loaded, trust })
        }
        Err(Error::SettingsRejected { sha256, reason }) => match load_last_good(root)? {
            Some(loaded) => {
                tracing::error!(
                    "the settings file was refused ({reason}); keeping the last verified settings"
                );
                Ok(Booted {
                    loaded,
                    trust: Trust::Rejected(sha256),
                })
            }
            None => Err(Error::SettingsRejected { sha256, reason }),
        },
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_key_list_is_fatal_when_it_is_required() {
        let dir = tempfile::tempdir().expect("dir");
        let root = Root::new(dir.path());
        assert!(matches!(
            load_keys_with(&root, true),
            Err(Error::KeysMissing(_))
        ));
        assert!(matches!(load_keys_with(&root, false), Ok(Keys::Unchecked)));
    }
}
