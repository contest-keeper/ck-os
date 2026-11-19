use std::fs;

use ck_agent_config::{Error, Root, apply_basics, load};
use ck_protocol::Problem;
use tempfile::TempDir;

const SETTINGS: &str = r#"{
    "schema": 1,
    "stick_id": "03c9f556",
    "created_at": "2026-11-10T15:22",
    "token": "the-secret",
    "team": " Team A ",
    "site": "Lab 1",
    "allowed_domains": ["example.org", ".docs.example.org"],
    "recording": {"mode": "standard", "start_delay_s": 30, "start_at": "2026-11-15T09:00", "snapshot_interval_s": 5}
}"#;

fn stick(settings: &str) -> (TempDir, Root) {
    let dir = TempDir::new().expect("temp dir");
    let root = Root::new(dir.path());
    let efi = dir.path().join("boot/efi/ck");
    fs::create_dir_all(&efi).expect("efi folder");
    fs::write(efi.join("config.json"), settings).expect("settings");
    (dir, root)
}

fn read(dir: &TempDir, path: &str) -> String {
    fs::read_to_string(dir.path().join(path)).expect(path)
}

#[test]
fn first_boot_keeps_the_same_private_key_without_putting_it_in_config() {
    let (dir, root) = stick(SETTINGS);
    let config_path = dir.path().join("ck-config");
    fs::create_dir_all(&config_path).expect("ckconfig");
    let enrolling = SETTINGS
        .replace(r#""schema": 1"#, r#""schema": 3"#)
        .replace(
            r#""token": "the-secret""#,
            r#""enrolment_code": "ABCD-EFGH-JKLM-NPQR-STUV""#,
        );
    fs::write(config_path.join("config.json"), &enrolling).expect("enrolment config");

    let first = load(&root).expect("first boot prepares the key");
    apply_basics(&root, &first).expect("apply basic settings");
    let again = load(&root).expect("restart reads the same key");
    assert_eq!(first.config.stick_id, again.config.stick_id);
    assert_eq!(first.config.token, again.config.token);
    assert_eq!(read(&dir, "etc/ck/config.json"), enrolling);
    assert_eq!(first.config.token.expose().len(), 43);

    let keys: Vec<_> = fs::read_dir(dir.path().join("var/lib/ck/enrolment"))
        .expect("enrolment state")
        .collect::<Result<_, _>>()
        .expect("enrolment state files");
    assert_eq!(keys.len(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = keys[0]
            .metadata()
            .expect("key metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn applies_the_settings_to_the_system_files() {
    let (dir, root) = stick(SETTINGS);
    let loaded = load(&root).expect("valid settings");
    apply_basics(&root, &loaded).expect("apply");

    assert_eq!(read(&dir, "etc/ck/config.json"), SETTINGS);
    assert_eq!(read(&dir, "etc/ck/config.sha256").trim(), loaded.sha256);
    assert_eq!(read(&dir, "etc/ck/team"), "Team A\n");
    assert_eq!(read(&dir, "etc/ck/site"), "Lab 1\n");

    let recording = read(&dir, "etc/ck/recording.conf");
    for line in [
        "MODE=standard",
        "START_DELAY=30",
        "START_AT=\"2026-11-15T09:00\"",
        "SNAPSHOT_INTERVAL=5",
    ] {
        assert!(recording.lines().any(|l| l == line), "missing {line}");
    }
    assert!(!recording.contains("UPLOAD_STICK="));

    let list = read(&dir, "etc/squid/acl_allowed_domains.txt");
    let sites: Vec<&str> = list.lines().filter(|line| !line.starts_with('#')).collect();
    assert_eq!(sites, [".example.org", ".docs.example.org"]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(dir.path().join("etc/ck/config.json"))
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[test]
fn recording_handoff_is_scoped_to_the_stick_only_when_opted_in() {
    let opted_in = SETTINGS.replace(
        "\"recording\": {",
        "\"recording\": {\"upload\":\"after_contest\",",
    );
    let (directory, root) = stick(&opted_in);
    let loaded = load(&root).expect("valid settings");
    apply_basics(&root, &loaded).expect("apply");
    let conf = read(&directory, "etc/ck/recording.conf");
    assert!(conf.lines().any(|line| line == "UPLOAD_STICK=03c9f556"));
}

#[test]
fn keeps_the_image_allowlist_when_no_sites_are_chosen() {
    let (dir, root) = stick(&SETTINGS.replace(r#"["example.org", ".docs.example.org"]"#, "[]"));
    let squid = dir.path().join("etc/squid");
    fs::create_dir_all(&squid).expect("squid folder");
    fs::write(
        squid.join("acl_allowed_domains.txt"),
        ".image-default.org\n",
    )
    .expect("image list");

    apply_basics(&root, &load(&root).expect("valid settings")).expect("apply");
    assert_eq!(
        read(&dir, "etc/squid/acl_allowed_domains.txt"),
        ".image-default.org\n"
    );
}

#[test]
fn refuses_missing_or_bad_settings_without_writing_anything() {
    let missing = TempDir::new().expect("temp dir");
    assert!(matches!(
        load(&Root::new(missing.path())),
        Err(Error::SettingsMissing(_))
    ));

    let (_dir, root) = stick("not json");
    assert!(matches!(load(&root), Err(Error::SettingsUnreadable(_))));

    let (dir, root) = stick(&SETTINGS.replace(r#""schema": 1"#, r#""schema": 3"#));
    let error = load(&root).expect_err("wrong schema");
    assert!(matches!(error, Error::SettingsInvalid(_)));
    assert!(!dir.path().join("etc").exists());
}

#[test]
fn lists_each_site_once_in_lowercase() {
    let sites = r#"["Example.org", ".example.org", "other.net"]"#;
    let (dir, root) = stick(&SETTINGS.replace(r#"["example.org", ".docs.example.org"]"#, sites));
    apply_basics(&root, &load(&root).expect("valid settings")).expect("apply");

    let list = read(&dir, "etc/squid/acl_allowed_domains.txt");
    let sites: Vec<&str> = list.lines().filter(|line| !line.starts_with('#')).collect();
    assert_eq!(sites, [".example.org", ".other.net"]);
}

#[test]
fn refuses_team_and_site_that_are_not_one_line() {
    // JSON allows "\n" escapes, so a settings file can carry a line break inside a name.
    let with_break = SETTINGS.replace(r#""team": " Team A ""#, r#""team": "A\nB""#);
    let (_dir, root) = stick(&with_break);
    assert!(matches!(
        load(&root),
        Err(Error::SettingsInvalid(p)) if p == [Problem::IdentityTextInvalid("team")]
    ));

    let blank = SETTINGS.replace(r#""site": "Lab 1""#, r#""site": "  ""#);
    let (_dir, root) = stick(&blank);
    assert!(matches!(
        load(&root),
        Err(Error::SettingsInvalid(p)) if p == [Problem::IdentityTextInvalid("site")]
    ));
}

#[test]
fn refuses_an_oversized_settings_file() {
    let (_dir, root) = stick(&format!("{SETTINGS}{}", " ".repeat(2 * 1024 * 1024)));
    assert!(matches!(load(&root), Err(Error::SettingsTooLarge(_))));
}

#[test]
fn leaves_no_temporary_files_and_replaces_old_content() {
    let (dir, root) = stick(SETTINGS);
    let loaded = load(&root).expect("valid settings");
    // A leftover from an interrupted run must not get in the way.
    fs::create_dir_all(dir.path().join("etc/ck")).expect("folder");
    fs::write(dir.path().join("etc/ck/config.json.ck-tmp"), "old").expect("leftover");
    fs::write(dir.path().join("etc/ck/config.json"), "stale").expect("old config");

    apply_basics(&root, &loaded).expect("apply");
    apply_basics(&root, &loaded).expect("apply again");

    assert_eq!(read(&dir, "etc/ck/config.json"), SETTINGS);
    let names: Vec<String> = fs::read_dir(dir.path().join("etc/ck"))
        .expect("list")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().all(|n| !n.ends_with(".ck-tmp")), "{names:?}");
}

#[test]
fn the_bluetooth_block_is_written_when_asked_and_removed_when_not() {
    let blocked = SETTINGS.replace(
        r#""team": " Team A ","#,
        r#""block_bluetooth": true, "team": " Team A ","#,
    );
    let (dir, root) = stick(&blocked);
    apply_basics(&root, &load(&root).expect("valid settings")).expect("apply");
    assert_eq!(
        read(&dir, "etc/ck/block-bluetooth"),
        "1
"
    );

    // The next boot has settings without the block: the old marker must not stay behind.
    fs::write(dir.path().join("boot/efi/ck/config.json"), SETTINGS).expect("settings");
    apply_basics(&root, &load(&root).expect("valid settings")).expect("apply");
    assert!(!dir.path().join("etc/ck/block-bluetooth").exists());
}

fn with_wifi(settings_json: &str) -> String {
    settings_json.replace(
        r#""team": " Team A ","#,
        r#""wifi": {"ssid": "Venue Net", "password": "p@ss w0rd\\"}, "team": " Team A ","#,
    )
}

#[test]
fn writes_the_wifi_connection_for_networkmanager_and_keeps_the_secret_to_two_private_files() {
    let (dir, root) = stick(&with_wifi(SETTINGS));
    apply_basics(&root, &load(&root).expect("valid settings")).expect("apply");

    let connection = read(
        &dir,
        "etc/NetworkManager/system-connections/ck-wifi.nmconnection",
    );
    assert!(connection.contains("ssid=Venue Net\n"));
    assert!(connection.contains("key-mgmt=wpa-psk\n"));
    // The backslash at the end of the password is doubled for the keyfile reader.
    assert!(connection.contains("psk=p@ss w0rd\\\\\n"), "{connection}");

    // Every file that holds the password must be root-only; none of the readable ones may.
    let allowed = [
        "etc/NetworkManager/system-connections/ck-wifi.nmconnection",
        "etc/ck/config.json",
        "boot/efi/ck/config.json",
    ];
    let mut found = Vec::new();
    collect(dir.path(), dir.path(), &mut found);
    for (relative, content) in found {
        if content.contains("w0rd") {
            assert!(
                allowed.contains(&relative.as_str()),
                "{relative} has the secret"
            );
            #[cfg(unix)]
            if relative != "boot/efi/ck/config.json" {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(dir.path().join(&relative))
                    .expect("metadata")
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o077, 0, "{relative} is readable by others");
            }
        }
    }
}

fn collect(base: &std::path::Path, folder: &std::path::Path, out: &mut Vec<(String, String)>) {
    for entry in fs::read_dir(folder).expect("folder").flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(base, &path, out);
        } else if let Ok(text) = fs::read_to_string(&path) {
            let relative = path.strip_prefix(base).expect("inside");
            out.push((relative.to_string_lossy().replace('\\', "/"), text));
        }
    }
}

#[test]
fn an_open_network_has_no_security_section_and_a_removed_network_removes_the_file() {
    let open = SETTINGS.replace(
        r#""team": " Team A ","#,
        r#""wifi": {"ssid": "Guest"}, "team": " Team A ","#,
    );
    let (dir, root) = stick(&open);
    apply_basics(&root, &load(&root).expect("valid settings")).expect("apply");
    let path = "etc/NetworkManager/system-connections/ck-wifi.nmconnection";
    let connection = read(&dir, path);
    assert!(connection.contains("ssid=Guest\n") && !connection.contains("wifi-security"));

    fs::write(dir.path().join("boot/efi/ck/config.json"), SETTINGS).expect("settings");
    apply_basics(&root, &load(&root).expect("valid settings")).expect("apply");
    assert!(
        !dir.path().join(path).exists(),
        "the old network does not stay"
    );
}
