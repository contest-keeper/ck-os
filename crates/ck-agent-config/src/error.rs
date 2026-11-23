use std::io;

use ck_protocol::Problem;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// There is no settings file. The image keeps its defaults.
    #[error("there are no settings at {0}")]
    SettingsMissing(String),
    #[error("the settings file is not valid: {0}")]
    SettingsUnreadable(String),
    /// An image that carries public keys refused the settings file: it is not signed, the signature
    /// does not match, or it is made for another stick. `sha256` is the hash of the refused file.
    #[error("the settings file was refused: {reason}")]
    SettingsRejected { sha256: String, reason: String },
    /// A release image must carry the list of keys that sign settings; without it nothing can be
    /// checked, so nothing is applied.
    #[error("this image has no list of settings keys at {0}")]
    KeysMissing(String),
    #[error("the settings file is larger than {0} bytes")]
    SettingsTooLarge(u64),
    #[error("the settings have problems: {}", join(.0))]
    SettingsInvalid(Vec<Problem>),
    #[error("the Firefox policies file cannot be changed: {0}")]
    PoliciesUnreadable(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: io::Error,
    },
}

impl Error {
    pub(crate) fn io(context: impl Into<String>, source: io::Error) -> Self {
        Self::Io {
            context: context.into(),
            source,
        }
    }
}

fn join(problems: &[Problem]) -> String {
    problems
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("; ")
}
