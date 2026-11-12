use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

const SETTINGS: &str = r#"{
    "schema": 1,
    "stick_id": "03c9f556",
    "created_at": "2026-11-10T15:22",
    "token": "the-secret-token",
    "team": "Team A",
    "site": "Lab 1",
    "allowed_domains": ["example.org"]
}"#;

fn apply(root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ck-agent"))
        .args(["apply-config", "--root"])
        .arg(root)
        .output()
        .expect("run ck-agent")
}

fn put_settings(root: &Path, text: &str) {
    let folder = root.join("boot/efi/ck");
    fs::create_dir_all(&folder).expect("efi folder");
    fs::write(folder.join("config.json"), text).expect("settings");
}

#[test]
fn applies_the_settings_and_never_logs_the_secret() {
    let dir = TempDir::new().expect("temp dir");
    put_settings(dir.path(), SETTINGS);

    let result = apply(dir.path());
    assert_eq!(result.status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(dir.path().join("etc/ck/team")).expect("team"),
        "Team A\n"
    );
    let log = String::from_utf8_lossy(&result.stderr);
    assert!(log.contains("settings applied"), "{log}");
    assert!(log.contains("03c9f556"), "{log}");
    assert!(!log.contains("the-secret-token"), "{log}");
}

#[test]
fn missing_settings_exit_with_3_and_change_nothing() {
    let dir = TempDir::new().expect("temp dir");
    let result = apply(dir.path());
    assert_eq!(result.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&result.stderr).contains("keeping the image defaults"));
    assert!(!dir.path().join("etc").exists());
}

#[test]
fn invalid_settings_exit_with_1_and_change_nothing() {
    let dir = TempDir::new().expect("temp dir");
    put_settings(dir.path(), r#"{"team":"T"}"#);
    let result = apply(dir.path());
    assert_eq!(result.status.code(), Some(1));
    assert!(!dir.path().join("etc").exists());
}
