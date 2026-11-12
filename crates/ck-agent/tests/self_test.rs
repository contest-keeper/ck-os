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
    "allowed_domains": ["Example.org", ".docs.example.org"],
    "recording": {"mode": "low", "start_delay_s": 30}
}"#;

fn agent(args: &[&str], root: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ck-agent"))
        .args(args)
        .arg("--root")
        .arg(root)
        .output()
        .expect("run ck-agent")
}

fn configured_root() -> TempDir {
    let dir = TempDir::new().expect("temp dir");
    let efi = dir.path().join("boot/efi/ck");
    fs::create_dir_all(&efi).expect("efi folder");
    fs::write(efi.join("config.json"), SETTINGS).expect("settings");
    assert_eq!(agent(&["apply-config"], dir.path()).status.code(), Some(0));
    dir
}

fn report_of(output: &Output) -> ck_selftest::Report {
    let text = String::from_utf8_lossy(&output.stdout);
    ck_selftest::find_report(&text)
        .expect("a report between the markers")
        .expect("valid json")
}

#[test]
fn reports_what_apply_config_wrote_and_never_the_secret() {
    let dir = configured_root();
    fs::create_dir_all(dir.path().join("proc")).expect("proc");
    fs::write(dir.path().join("proc/uptime"), "41.50 100.00\n").expect("uptime");

    let result = agent(&["self-test"], dir.path());
    assert_eq!(result.status.code(), Some(0));
    let report = report_of(&result);

    assert_eq!(report.applied.stick_id.as_deref(), Some("03c9f556"));
    assert_eq!(report.applied.team.as_deref(), Some("Team A"));
    assert_eq!(report.applied.site.as_deref(), Some("Lab 1"));
    assert_eq!(report.applied.recording_mode.as_deref(), Some("low"));
    assert_eq!(
        report.applied.allowed_domains,
        ["example.org", "docs.example.org"]
    );
    assert!(report.applied.config_hash_matches);
    assert!((report.uptime_seconds - 41.5).abs() < f64::EPSILON);
    let names: Vec<&str> = report.units.iter().map(|u| u.name.as_str()).collect();
    assert_eq!(names, ["ck-config", "ck-recorder", "icpc-screen-record"]);
    assert!(!String::from_utf8_lossy(&result.stdout).contains("the-secret-token"));
}

#[test]
fn a_changed_settings_copy_no_longer_matches_its_hash() {
    let dir = configured_root();
    fs::write(dir.path().join("etc/ck/config.json"), "{}").expect("tamper");
    let report = report_of(&agent(&["self-test"], dir.path()));
    assert!(!report.applied.config_hash_matches);
}

#[test]
fn an_unconfigured_machine_reports_nothing_applied() {
    let dir = TempDir::new().expect("temp dir");
    let report = report_of(&agent(&["self-test"], dir.path()));
    assert_eq!(report.applied.team, None);
    assert_eq!(report.applied.stick_id, None);
    assert_eq!(report.applied.allowed_domains.len(), 0);
    assert!(!report.applied.config_hash_matches);
}

#[test]
fn only_a_machine_that_asks_for_it_writes_a_report() {
    let dir = configured_root();
    let quiet = agent(&["self-test", "--only-if-requested"], dir.path());
    assert_eq!(quiet.status.code(), Some(0));
    assert_eq!(quiet.stdout.len(), 0, "a real stick prints no report");

    let table = dir.path().join("sys/firmware/dmi/entries/11-0");
    fs::create_dir_all(&table).expect("dmi table");
    fs::write(
        table.join("raw"),
        b"\x0b\x05\x00\x01\x01\x00ck-selftest=1\x00\x00",
    )
    .expect("raw");
    let asked = agent(&["self-test", "--only-if-requested"], dir.path());
    assert_eq!(report_of(&asked).applied.team.as_deref(), Some("Team A"));
}

#[test]
fn writes_the_report_to_a_serial_port_file_when_asked() {
    let dir = configured_root();
    let port = dir.path().join("ttyS0");
    fs::write(&port, "").expect("port");
    let result = agent(
        &["self-test", "--serial", port.to_str().expect("utf-8 path")],
        dir.path(),
    );
    assert_eq!(result.status.code(), Some(0));
    assert_eq!(result.stdout.len(), 0);
    let written = fs::read_to_string(&port).expect("read port");
    let report = ck_selftest::find_report(&written)
        .expect("a report")
        .expect("valid");
    assert_eq!(report.applied.site.as_deref(), Some("Lab 1"));
}
