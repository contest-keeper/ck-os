use std::io;

use ck_protocol::Problem;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// There is no settings file. The image keeps its defaults.
    #[error("there are no settings at {0}")]
    SettingsMissing(String),
    #[error("the settings file is not valid: {0}")]
    SettingsUnreadable(String),
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
