//! An image that carries public keys only takes settings the Hub signed. A refused file is ignored:
//! the stick keeps the last settings it verified and says so.

use std::fs;

use ck_agent_config::{Error, Root, Trust, apply_basics, load, load_for_boot, write_trust};
use ck_crypto::ConfigSigningKey;
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

const OTHER_STICK: &str = r#"{
    "schema": 2,
    "stick_id": "aaaa1111",
    "created_at": "2026-11-10T15:22",
    "token": "another-secret",
    "team": "Practice",
    "site": "Lab 1",
    "allowed_domains": ["example.org", "open.example.net"]
}"#;

struct Stick {
    dir: TempDir,
    root: Root,
}

impl Stick {
    /// A stick whose image trusts `keys`; none means an image with no key list.
    fn new(keys: &[&ConfigSigningKey]) -> Self {
        let dir = TempDir::new().expect("temp dir");
        fs::create_dir_all(dir.path().join("ck-config")).expect("ckconfig");
        fs::create_dir_all(dir.path().join("etc/ck")).expect("etc");
        if !keys.is_empty() {
            let list: Vec<String> = keys
                .iter()
                .map(|key| key.verifying_key().to_base64())
                .collect();
            fs::write(
                dir.path().join("etc/ck/config-public-keys"),
                format!("# trusted\n{}\n", list.join("\n")),
            )
            .expect("keys");
        }
        let root = Root::new(dir.path());
        Self { dir, root }
    }

    fn path(&self, relative: &str) -> std::path::PathBuf {
        self.dir.path().join(relative)
    }

    fn put(&self, settings: &str) {
        fs::write(self.path("ck-config/config.json"), settings).expect("settings");
    }

    fn put_signed(&self, key: &ConfigSigningKey, settings: &str) {
        self.put(settings);
        fs::write(
            self.path("ck-config/config.json.sig"),
            key.sign(settings.as_bytes()).to_file_text(),
        )
        .expect("signature");
    }

    /// What one boot does: read the settings for it and apply them.
    fn boot(&self) -> Result<ck_agent_config::Booted, Error> {
        let booted = load_for_boot(&self.root)?;
        apply_basics(&self.root, &booted.loaded).expect("apply");
        write_trust(&self.root, &booted.trust).expect("trust");
        Ok(booted)
    }

    fn marker(&self) -> String {
        fs::read_to_string(self.path("etc/ck/config.trust")).expect("trust marker")
    }

    fn allowlist(&self) -> String {
        fs::read_to_string(self.path("etc/squid/acl_allowed_domains.txt")).expect("allowlist")
    }
}

fn rejected(result: &Result<ck_agent_config::Booted, Error>) -> bool {
    matches!(result, Err(Error::SettingsRejected { .. }))
}

#[test]
fn settings_signed_by_a_trusted_key_are_applied_and_reported_verified() {
    let key = ConfigSigningKey::generate();
    let stick = Stick::new(&[&key]);
    stick.put_signed(&key, SETTINGS);
    let booted = stick.boot().expect("verified settings");
    assert_eq!(booted.trust, Trust::Verified);
    assert!(booted.loaded.verified);
    assert_eq!(stick.marker(), "verified\n");
    assert!(stick.allowlist().contains(".example.org"));
}

#[test]
fn an_edited_file_is_refused_and_the_last_verified_settings_stay() {
    let key = ConfigSigningKey::generate();
    let stick = Stick::new(&[&key]);
    stick.put_signed(&key, SETTINGS);
    stick.boot().expect("the first boot");

    // Someone adds a site on another PC; the old signature is still next to the file.
    let edited = SETTINGS.replace(
        r#"["example.org"]"#,
        r#"["example.org", "cheat.example.net"]"#,
    );
    stick.put(&edited);
    let booted = stick
        .boot()
        .expect("the stick goes on with what it verified");
    let Trust::Rejected(sha256) = &booted.trust else {
        panic!("the edited file must be reported: {:?}", booted.trust);
    };
    assert_eq!(sha256, &ck_crypto::sha256_hex(edited.as_bytes()));
    assert_eq!(booted.loaded.raw, SETTINGS.as_bytes());
    assert_eq!(stick.marker(), format!("rejected {sha256}\n"));
    assert!(!stick.allowlist().contains("cheat"));
    // The hash the stick reports is the verified file's, not the edited one's.
    let reported = fs::read_to_string(stick.path("etc/ck/config.sha256")).expect("hash");
    assert_eq!(reported.trim(), ck_crypto::sha256_hex(SETTINGS.as_bytes()));
}

#[test]
fn a_first_boot_with_a_refused_file_has_nothing_to_fall_back_on() {
    let key = ConfigSigningKey::generate();
    let stick = Stick::new(&[&key]);
    stick.put_signed(&key, SETTINGS);
    stick.put(&SETTINGS.replace("Team A", "Team B"));
    assert!(rejected(&load_for_boot(&stick.root)));
    assert!(matches!(
        load(&stick.root),
        Err(Error::SettingsRejected { .. })
    ));
    // Nothing from the refused file reached the system.
    assert!(!stick.path("etc/ck/config.json").exists());
}

#[test]
fn an_unsigned_file_and_a_file_signed_with_another_key_are_refused() {
    let trusted = ConfigSigningKey::generate();
    let stranger = ConfigSigningKey::generate();
    let stick = Stick::new(&[&trusted]);

    stick.put(SETTINGS);
    let error = load(&stick.root).expect_err("unsigned is refused");
    assert!(error.to_string().contains("not signed"), "{error}");

    stick.put_signed(&stranger, SETTINGS);
    let error = load(&stick.root).expect_err("another key is refused");
    assert!(
        error.to_string().contains("key this image does not know"),
        "{error}"
    );

    // A signature file that is not a signature is refused too.
    fs::write(stick.path("ck-config/config.json.sig"), "garbage").expect("signature");
    assert!(load(&stick.root).is_err());
}

#[test]
fn deleting_the_file_does_not_let_the_unsigned_legacy_copy_take_over() {
    let key = ConfigSigningKey::generate();
    let stick = Stick::new(&[&key]);
    fs::create_dir_all(stick.path("boot/efi/ck")).expect("efi");
    fs::write(
        stick.path("boot/efi/ck/config.json"),
        SETTINGS.replace(r#""schema": 2"#, r#""schema": 1"#),
    )
    .expect("legacy settings");
    // There is no file on CKCONFIG at all.
    assert!(matches!(load(&stick.root), Err(Error::SettingsMissing(_))));
    assert!(matches!(
        load_for_boot(&stick.root),
        Err(Error::SettingsMissing(_))
    ));
}

#[test]
fn an_image_without_keys_takes_unsigned_settings_as_before() {
    let stick = Stick::new(&[]);
    stick.put(SETTINGS);
    let booted = stick.boot().expect("unchecked settings");
    assert_eq!(booted.trust, Trust::Unchecked);
    assert!(!booted.loaded.verified);
    assert_eq!(stick.marker(), "unchecked\n");

    // The legacy copy on the EFI partition still works there.
    let legacy = Stick::new(&[]);
    fs::remove_dir_all(legacy.path("ck-config")).expect("no ckconfig");
    fs::create_dir_all(legacy.path("boot/efi/ck")).expect("efi");
    fs::write(
        legacy.path("boot/efi/ck/config.json"),
        SETTINGS.replace(r#""schema": 2"#, r#""schema": 1"#),
    )
    .expect("legacy settings");
    assert!(load(&legacy.root).is_ok());
}

#[test]
fn a_signed_file_of_another_stick_is_refused_once_the_stick_is_set_up() {
    let key = ConfigSigningKey::generate();
    let stick = Stick::new(&[&key]);
    stick.put_signed(&key, SETTINGS);
    stick.boot().expect("the first boot");

    // A practice stick's settings are validly signed, but they are not this stick's.
    stick.put_signed(&key, OTHER_STICK);
    let booted = stick.boot().expect("the stick keeps its own settings");
    assert!(matches!(booted.trust, Trust::Rejected(_)));
    assert_eq!(booted.loaded.raw, SETTINGS.as_bytes());
    assert!(!stick.allowlist().contains("open.example.net"));

    // The same stick with new signed settings is taken.
    let changed = SETTINGS.replace(
        r#"["example.org"]"#,
        r#"["example.org", "new.example.org"]"#,
    );
    stick.put_signed(&key, &changed);
    let booted = stick.boot().expect("new settings of this stick");
    assert_eq!(booted.trust, Trust::Verified);
    assert!(stick.allowlist().contains("new.example.org"));
}

#[test]
fn a_damaged_key_list_trusts_nothing() {
    let key = ConfigSigningKey::generate();
    let stick = Stick::new(&[&key]);
    stick.put_signed(&key, SETTINGS);
    fs::write(stick.path("etc/ck/config-public-keys"), "not a key\n").expect("keys");
    assert!(matches!(
        load(&stick.root),
        Err(Error::SettingsRejected { .. })
    ));
}

#[test]
fn a_key_that_replaces_another_keeps_old_sticks_working_while_both_are_listed() {
    let (old, new) = (ConfigSigningKey::generate(), ConfigSigningKey::generate());
    let stick = Stick::new(&[&old, &new]);
    for key in [&old, &new] {
        stick.put_signed(key, SETTINGS);
        assert!(load(&stick.root).is_ok());
    }
}

#[test]
fn nothing_of_a_refused_first_boot_enrolment_file_reaches_the_system() {
    let key = ConfigSigningKey::generate();
    let stick = Stick::new(&[&key]);
    let enrolling = SETTINGS
        .replace(r#""schema": 2"#, r#""schema": 3"#)
        .replace(
            r#""token": "the-secret""#,
            r#""enrolment_code": "ABCD-EFGH-JKLM-NPQR-STUV""#,
        );
    stick.put_signed(&key, &enrolling);
    stick.put(&enrolling.replace("Team A", "Team B"));
    assert!(matches!(
        load(&stick.root),
        Err(Error::SettingsRejected { .. })
    ));
    // No local key was made for the refused file.
    assert!(!stick.path("var/lib/ck/enrolment").exists());
}
