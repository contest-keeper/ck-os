//! `ck-agent self-test`: looks at the machine after boot and reports what the agent applied.
//!
//! The report is built from the system's own files, not from the settings file the agent read, so
//! it shows what is really on the machine.

use std::fs;
use std::path::Path;
use std::process::Command;

use ck_selftest::{Applied, Report, UnitState};

/// The services the VM test cares about.
const UNITS: [&str; 4] = [
    "ck-config",
    "ck-recorder",
    "ck-uplink",
    "icpc-screen-record",
];

/// A test machine asks for the report with this OEM string, which QEMU can set with
/// `-smbios type=11,value=ck-selftest=1`. A real stick never has it.
const REQUEST: &[u8] = b"ck-selftest=1";

pub fn requested(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root.join("sys/firmware/dmi/entries")) else {
        return false;
    };
    for entry in entries.flatten() {
        // Table type 11 holds the OEM strings.
        if !entry.file_name().to_string_lossy().starts_with("11-") {
            continue;
        }
        if let Ok(raw) = fs::read(entry.path().join("raw"))
            && raw.windows(REQUEST.len()).any(|window| window == REQUEST)
        {
            return true;
        }
    }
    false
}

pub fn collect(root: &Path, unit_state: &dyn Fn(&str) -> UnitState) -> Report {
    Report {
        agent_version: env!("CARGO_PKG_VERSION").to_owned(),
        uptime_seconds: uptime(root),
        applied: applied(root),
        units: UNITS.iter().map(|unit| unit_state(unit)).collect(),
    }
}

/// Asks systemd about a service. A machine without systemd answers "unknown".
pub fn systemctl_state(unit: &str) -> UnitState {
    UnitState {
        name: unit.to_owned(),
        active: systemctl("is-active", unit),
        enabled: systemctl("is-enabled", unit),
    }
}

fn systemctl(question: &str, unit: &str) -> String {
    // The answer is on stdout even when the exit code says "no", for example "inactive".
    match Command::new("systemctl").args([question, unit]).output() {
        Ok(output) => {
            let answer = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if answer.is_empty() {
                "unknown".to_owned()
            } else {
                answer
            }
        }
        Err(_) => "unknown".to_owned(),
    }
}

fn uptime(root: &Path) -> f64 {
    fs::read_to_string(root.join("proc/uptime"))
        .ok()
        .and_then(|text| text.split_whitespace().next()?.parse().ok())
        .unwrap_or(0.0)
}

fn applied(root: &Path) -> Applied {
    let config = fs::read(root.join("etc/ck/config.json")).ok();
    let stored_hash = read_trimmed(root, "etc/ck/config.sha256");
    let hash_matches = match (&config, &stored_hash) {
        (Some(bytes), Some(stored)) => ck_crypto::sha256_hex(bytes) == *stored,
        _ => false,
    };
    let stick_id = config
        .as_deref()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok())
        .and_then(|json| json.get("stick_id")?.as_str().map(str::to_owned));

    let (firefox_bookmarks, firefox_home_page) = firefox(root);
    Applied {
        stick_id,
        team: read_trimmed(root, "etc/ck/team"),
        site: read_trimmed(root, "etc/ck/site"),
        recording_mode: recording_mode(root),
        allowed_domains: allowed_domains(root),
        config_hash_matches: hash_matches,
        computer_name: read_trimmed(root, "etc/hostname"),
        running_hostname: read_trimmed(root, "proc/sys/kernel/hostname"),
        firefox_bookmarks,
        firefox_home_page,
        welcome_text: read_trimmed(root, "etc/ck/welcome.txt"),
        wallpaper_is_team_wallpaper: wallpaper_matches(root),
    }
}

fn read_trimmed(root: &Path, relative: &str) -> Option<String> {
    let text = fs::read_to_string(root.join(relative)).ok()?;
    Some(text.trim().to_owned())
}

fn recording_mode(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join("etc/ck/recording.conf")).ok()?;
    for line in text.lines() {
        if let Some(mode) = line.strip_prefix("MODE=") {
            return Some(mode.trim().to_owned());
        }
    }
    None
}

fn allowed_domains(root: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(root.join("etc/squid/acl_allowed_domains.txt")) else {
        return Vec::new();
    };
    let mut sites = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if !line.is_empty() && !line.starts_with('#') {
            sites.push(line.trim_start_matches('.').to_owned());
        }
    }
    sites
}

fn firefox(root: &Path) -> (Vec<String>, Option<String>) {
    let text = fs::read_to_string(root.join("usr/lib/firefox/distribution/policies.json")).ok();
    let policies = text
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|json| json.get("policies").cloned());
    let Some(policies) = policies else {
        return (Vec::new(), None);
    };
    let mut titles = Vec::new();
    if let Some(bookmarks) = policies.get("Bookmarks").and_then(|b| b.as_array()) {
        for bookmark in bookmarks {
            if let Some(title) = bookmark.get("Title").and_then(|t| t.as_str()) {
                titles.push(title.to_owned());
            }
        }
    }
    let home = policies
        .get("Homepage")
        .and_then(|h| h.get("URL"))
        .and_then(|u| u.as_str())
        .map(str::to_owned);
    (titles, home)
}

/// Whether the picture the settings name, which is on the EFI partition, is the team wallpaper the
/// image shows. `None` when the settings name no wallpaper.
fn wallpaper_matches(root: &Path) -> Option<bool> {
    let config = fs::read(root.join("etc/ck/config.json")).ok()?;
    let json: serde_json::Value = serde_json::from_slice(&config).ok()?;
    let name = json.get("branding")?.get("wallpaper")?.as_str()?.to_owned();
    let on_stick = fs::read(root.join("boot/efi/ck").join(&name)).ok();
    let shown = fs::read(root.join("icpc/teamWallpaper.png")).ok();
    Some(on_stick.is_some() && on_stick == shown)
}
