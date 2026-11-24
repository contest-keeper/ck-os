//! The `CKCONFIG` partition is a USB stick's FAT filesystem: anyone who has the stick can put any bytes
//! in it. Reading it must end in settings or an error, never a panic, and an image with Hub keys must
//! never take a file the Hub did not sign.
//!
//! The number of cases per property is proptest's default (256) and follows `PROPTEST_CASES`.

use std::fs;

use ck_agent_config::{Error, Root, load};
use ck_crypto::{ConfigSignature, ConfigSigningKey};
use proptest::prelude::*;
use tempfile::TempDir;

const SETTINGS: &str = r#"{
    "schema": 2,
    "stick_id": "03c9f556",
    "created_at": "2026-11-10T15:22",
    "token": "the-secret",
    "team": "Team A",
    "site": "Lab 1",
    "allowed_domains": ["example.org"]
}"#;

/// A stick whose image trusts `key`, or has no key list at all.
fn stick(key: Option<&ConfigSigningKey>) -> (TempDir, Root) {
    let dir = TempDir::new().expect("temp dir");
    fs::create_dir_all(dir.path().join("ck-config")).expect("ckconfig");
    fs::create_dir_all(dir.path().join("etc/ck")).expect("etc");
    if let Some(key) = key {
        fs::write(
            dir.path().join("etc/ck/config-public-keys"),
            format!("{}\n", key.verifying_key().to_base64()),
        )
        .expect("keys");
    }
    let root = Root::new(dir.path());
    (dir, root)
}

fn put(dir: &TempDir, name: &str, bytes: &[u8]) {
    fs::write(dir.path().join("ck-config").join(name), bytes).expect(name);
}

/// The valid settings with a stretch of bytes replaced, so the file is close to valid.
fn damaged(at: usize, with: &[u8]) -> Vec<u8> {
    let mut bytes = SETTINGS.as_bytes().to_vec();
    let at = at % bytes.len();
    let end = (at + with.len()).min(bytes.len());
    bytes.splice(at..end, with.iter().copied());
    bytes
}

proptest! {
    #[test]
    fn any_bytes_as_settings_end_in_settings_or_an_error(
        bytes in proptest::collection::vec(any::<u8>(), 0..4096),
    ) {
        let (dir, root) = stick(None);
        put(&dir, "config.json", &bytes);
        if let Ok(loaded) = load(&root) {
            prop_assert!(loaded.config.validate().is_empty());
            prop_assert_eq!(loaded.raw, bytes);
            prop_assert!(!loaded.verified);
        }
    }

    #[test]
    fn settings_damaged_anywhere_end_in_settings_or_an_error(
        at in any::<usize>(),
        with in proptest::collection::vec(any::<u8>(), 0..16),
    ) {
        let (dir, root) = stick(None);
        put(&dir, "config.json", &damaged(at, &with));
        if let Ok(loaded) = load(&root) {
            prop_assert!(loaded.config.validate().is_empty());
        }
    }

    #[test]
    fn an_image_with_a_key_refuses_any_unsigned_or_wrongly_signed_file(
        settings in proptest::collection::vec(any::<u8>(), 0..2048),
        signature in proptest::collection::vec(any::<u8>(), 0..512),
    ) {
        let key = ConfigSigningKey::generate();
        let (dir, root) = stick(Some(&key));
        put(&dir, "config.json", &settings);
        put(&dir, "config.json.sig", &signature);
        prop_assert!(load(&root).is_err());
    }

    #[test]
    fn a_signed_file_changed_in_any_byte_is_refused(index in any::<usize>(), flip in 1u8..=255) {
        let key = ConfigSigningKey::generate();
        let (dir, root) = stick(Some(&key));
        let mut settings = SETTINGS.as_bytes().to_vec();
        put(&dir, "config.json", &settings);
        put(&dir, "config.json.sig", key.sign(&settings).to_file_text().as_bytes());
        prop_assert!(load(&root).is_ok(), "the untouched file loads");

        let index = index % settings.len();
        settings[index] ^= flip;
        put(&dir, "config.json", &settings);
        let refused = matches!(load(&root), Err(Error::SettingsRejected { .. }));
        prop_assert!(refused);
    }

    #[test]
    fn a_signature_that_says_something_else_is_refused(index in any::<usize>(), flip in 1u8..=255) {
        let key = ConfigSigningKey::generate();
        let (dir, root) = stick(Some(&key));
        put(&dir, "config.json", SETTINGS.as_bytes());
        let text = key.sign(SETTINGS.as_bytes()).to_file_text();
        let mut changed = text.clone().into_bytes();
        let index = index % changed.len();
        changed[index] ^= flip;
        put(&dir, "config.json.sig", &changed);

        // Some changes leave the same signature: a tab for the space, a capital in the key id, the
        // unused bits of the last base64 character. Those are still the Hub's signature.
        let same_signature = std::str::from_utf8(&changed)
            .ok()
            .and_then(|text| ConfigSignature::parse(text).ok())
            .is_some_and(|parsed| parsed.to_file_text() == text);
        if !same_signature {
            prop_assert!(load(&root).is_err());
        }
    }
}

#[test]
fn a_file_over_the_size_limit_is_refused_before_it_is_read_as_settings() {
    let (dir, root) = stick(None);
    put(&dir, "config.json", &vec![b' '; 1024 * 1024 + 1]);
    assert!(matches!(load(&root), Err(Error::SettingsTooLarge(_))));
}
