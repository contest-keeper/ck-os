use std::fs;
use std::io::{ErrorKind, Write};

use ck_protocol::{ENROLMENT_SCHEMA, EnrolmentCode, EnrolmentConfig, SCHEMA, StickConfig, Token};
use serde::{Deserialize, de::DeserializeOwned};

use crate::{Error, Root, apply::write};

const STATE: &str = "/var/lib/ck/enrolment";

#[derive(Deserialize)]
struct Schema {
    schema: u32,
}

/// The code still on CKCONFIG and the fingerprint that binds the local key to that config.
pub struct PendingEnrolment {
    code: EnrolmentCode,
    fingerprint: String,
}

impl PendingEnrolment {
    #[must_use]
    pub fn code(&self) -> &str {
        self.code.expose()
    }

    pub fn confirmed(&self, root: &Root) -> Result<bool, Error> {
        root.path(&self.confirmation_file())
            .try_exists()
            .map_err(|error| Error::io("checking the enrolment state", error))
    }

    pub fn confirm(&self, root: &Root) -> Result<(), Error> {
        write(root, &self.confirmation_file(), b"claimed\n", 0o600)
    }

    fn confirmation_file(&self) -> String {
        format!("{STATE}/{}.claimed", self.fingerprint)
    }
}

/// Resolves an applied config using the key already prepared by the boot configuration service.
pub fn read_applied_config(
    root: &Root,
    raw: &[u8],
) -> Result<(StickConfig, Option<PendingEnrolment>), Error> {
    resolve(root, raw, false)
}

pub(crate) fn resolve(
    root: &Root,
    raw: &[u8],
    prepare_key: bool,
) -> Result<(StickConfig, Option<PendingEnrolment>), Error> {
    let schema: Schema = parse(raw)?;
    if schema.schema != ENROLMENT_SCHEMA {
        return Ok((parse(raw)?, None));
    }

    let bootstrap: EnrolmentConfig = parse(raw)?;
    let fingerprint = ck_crypto::sha256_hex(raw);
    let key_path = root.path(&format!("{STATE}/{fingerprint}.key"));
    let read_key = || fs::read_to_string(&key_path).map(Token::new);
    let token = match read_key() {
        Err(error) if error.kind() == ErrorKind::NotFound && prepare_key => {
            let token = ck_crypto::generate_stick_token();
            match save_first_key(&key_path, &token) {
                Ok(()) => Ok(token),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => read_key(),
                Err(error) => Err(error),
            }
        }
        result => result,
    }
    .map_err(|error| Error::io("reading or preparing the private enrolment key", error))?;

    Ok((
        StickConfig {
            schema: SCHEMA,
            stick_id: bootstrap.stick_id,
            created_at: bootstrap.created_at,
            token,
            machine_number: bootstrap.machine_number,
            settings: bootstrap.settings,
        },
        Some(PendingEnrolment {
            code: bootstrap.enrolment_code,
            fingerprint,
        }),
    ))
}

fn parse<T: DeserializeOwned>(raw: &[u8]) -> Result<T, Error> {
    serde_json::from_slice(raw).map_err(|error| Error::SettingsUnreadable(error.to_string()))
}

fn save_first_key(path: &std::path::Path, token: &Token) -> std::io::Result<()> {
    let folder = path
        .parent()
        .expect("the enrolment key has a state directory");
    fs::create_dir_all(folder)?;
    let mut pending = tempfile::NamedTempFile::new_in(folder)?;
    pending.write_all(token.expose().as_bytes())?;
    pending.as_file().sync_all()?;
    pending
        .persist_noclobber(path)
        .map_err(|error| error.error)?;
    #[cfg(unix)]
    fs::File::open(folder)?.sync_all()?;
    Ok(())
}
