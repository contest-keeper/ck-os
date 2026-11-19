//! Where the settings are read from: the `CKCONFIG` partition first, the EFI partition of an older
//! stick second.

use std::fs;

use ck_agent_config::{Error, Root, Source, apply_branding, load};
use tempfile::TempDir;

fn settings(team: &str, wallpaper: Option<&str>) -> String {
    let branding = wallpaper.map_or_else(
        || "{}".to_owned(),
        |name| format!(r#"{{"wallpaper": "{name}"}}"#),
    );
    format!(
        r#"{{
    "schema": 2,
    "stick_id": "03c9f556",
    "created_at": "2026-11-10T15:22",
    "token": "the-secret",
    "team": "{team}",
    "site": "Lab 1",
    "branding": {branding}
}}"#
    )
}

fn put(dir: &TempDir, path: &str, content: impl AsRef<[u8]>) {
    let file = dir.path().join(path);
    fs::create_dir_all(file.parent().expect("parent")).expect("folder");
    fs::write(file, content).expect(path);
}

#[test]
fn settings_on_the_config_partition_are_read() {
    let dir = TempDir::new().expect("temp dir");
    put(
        &dir,
        "ck-config/config.json",
        settings("From CKCONFIG", None),
    );

    let loaded = load(&Root::new(dir.path())).expect("loads");
    assert_eq!(loaded.source, Source::CkConfig);
    assert_eq!(loaded.config.settings.team, "From CKCONFIG");
}

#[test]
fn an_older_stick_without_the_partition_still_reads_the_efi_copy() {
    let dir = TempDir::new().expect("temp dir");
    put(
        &dir,
        "boot/efi/ck/config.json",
        settings("From the ESP", None),
    );

    let loaded = load(&Root::new(dir.path())).expect("loads");
    assert_eq!(loaded.source, Source::Esp);
    assert_eq!(loaded.config.settings.team, "From the ESP");
}

#[test]
fn the_config_partition_wins_over_the_efi_copy() {
    let dir = TempDir::new().expect("temp dir");
    put(
        &dir,
        "ck-config/config.json",
        settings("From CKCONFIG", None),
    );
    put(
        &dir,
        "boot/efi/ck/config.json",
        settings("From the ESP", None),
    );

    let loaded = load(&Root::new(dir.path())).expect("loads");
    assert_eq!(loaded.source, Source::CkConfig);
    assert_eq!(loaded.config.settings.team, "From CKCONFIG");
}

#[test]
fn a_bad_file_on_the_config_partition_is_not_replaced_by_the_old_copy() {
    let dir = TempDir::new().expect("temp dir");
    put(&dir, "ck-config/config.json", "{ not json");
    put(
        &dir,
        "boot/efi/ck/config.json",
        settings("From the ESP", None),
    );

    assert!(matches!(
        load(&Root::new(dir.path())),
        Err(Error::SettingsUnreadable(_))
    ));
}

#[test]
fn with_no_settings_anywhere_the_stick_keeps_the_image_defaults() {
    let dir = TempDir::new().expect("temp dir");
    assert!(matches!(
        load(&Root::new(dir.path())),
        Err(Error::SettingsMissing(_))
    ));
}

#[test]
fn the_wallpaper_comes_from_the_partition_the_settings_came_from() {
    let dir = TempDir::new().expect("temp dir");
    put(
        &dir,
        "ck-config/config.json",
        settings("A", Some("wall.png")),
    );
    put(&dir, "ck-config/wall.png", b"new picture");
    put(&dir, "boot/efi/ck/wall.png", b"old picture");
    put(&dir, "etc/hostname", "icpc\n");
    put(&dir, "etc/hosts", "127.0.0.1 localhost\n");
    put(
        &dir,
        "usr/lib/firefox/distribution/policies.json",
        r#"{"policies": {}}"#,
    );

    let root = Root::new(dir.path());
    let loaded = load(&root).expect("loads");
    apply_branding(&root, &loaded).expect("applies");
    assert_eq!(
        fs::read(dir.path().join("icpc/teamWallpaper.png")).expect("copied"),
        b"new picture"
    );
}
