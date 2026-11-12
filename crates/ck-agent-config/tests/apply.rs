use std::fs;

use ck_agent_config::{Error, Root, apply_basics, load};
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
fn applies_the_settings_to_the_system_files() {
    let (dir, root) = stick(SETTINGS);
    let loaded = load(&root).expect("valid settings");
    apply_basics(&root, &loaded).expect("apply");

    assert_eq!(read(&dir, "etc/ck/config.json"), SETTINGS);
    assert_eq!(read(&dir, "etc/ck/config.sha256").trim(), loaded.sha256);
    assert_eq!(read(&dir, "icpc/TEAM"), "Team A\n");
    assert_eq!(read(&dir, "icpc/SITE"), "Lab 1\n");

    let recording = read(&dir, "etc/ck/recording.conf");
    for line in [
        "MODE=standard",
        "START_DELAY=30",
        "START_AT=\"2026-11-15T09:00\"",
        "SNAPSHOT_INTERVAL=5",
    ] {
        assert!(recording.lines().any(|l| l == line), "missing {line}");
    }

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

    let (dir, root) = stick(&SETTINGS.replace(r#""schema": 1"#, r#""schema": 2"#));
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
    assert!(matches!(load(&root), Err(Error::SettingsText("team"))));

    let blank = SETTINGS.replace(r#""site": "Lab 1""#, r#""site": "  ""#);
    let (_dir, root) = stick(&blank);
    assert!(matches!(load(&root), Err(Error::SettingsText("site"))));
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
