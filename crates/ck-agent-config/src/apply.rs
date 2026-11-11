use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use ck_protocol::Settings;

use crate::error::Error;
use crate::load::Loaded;
use crate::root::Root;

const CONFIG_COPY: &str = "/etc/ck/config.json";
const CONFIG_HASH: &str = "/etc/ck/config.sha256";
const RECORDING_CONF: &str = "/etc/ck/recording.conf";
const TEAM: &str = "/icpc/TEAM";
const SITE: &str = "/icpc/SITE";
const SQUID_ALLOWLIST: &str = "/etc/squid/acl_allowed_domains.txt";

const GENERATED: &str = "# Written at boot by ck-agent from the admin tool settings\n";

/// Applies the settings that need no restart of anything: the root-only copy of the settings and
/// its hash, team and site, the recorder settings and the proxy allowlist.
///
/// # Errors
/// Fails when a file cannot be written.
pub fn apply_basics(root: &Root, loaded: &Loaded) -> Result<(), Error> {
    let settings = &loaded.config.settings;

    write(root, CONFIG_COPY, &loaded.raw, 0o600)?;
    write(
        root,
        CONFIG_HASH,
        format!("{}\n", loaded.sha256).as_bytes(),
        0o644,
    )?;
    write(
        root,
        TEAM,
        format!("{}\n", settings.team.trim()).as_bytes(),
        0o644,
    )?;
    write(
        root,
        SITE,
        format!("{}\n", settings.site.trim()).as_bytes(),
        0o644,
    )?;
    write(
        root,
        RECORDING_CONF,
        recording_conf(settings).as_bytes(),
        0o644,
    )?;

    // No sites chosen: keep the image's own list rather than block everything.
    if let Some(list) = allowlist(settings) {
        write(root, SQUID_ALLOWLIST, list.as_bytes(), 0o644)?;
    } else {
        tracing::warn!("no allowed sites in the settings; keeping the image's list");
    }
    Ok(())
}

/// Shell variables that `ck-recorder` reads with `.`.
fn recording_conf(settings: &Settings) -> String {
    let recording = &settings.recording;
    let mut conf = format!(
        "{GENERATED}MODE={}\nSTART_DELAY={}\n",
        recording.mode.as_str(),
        recording.start_delay_s
    );
    if let Some(start_at) = recording.start_at {
        let _ = writeln!(conf, "START_AT=\"{start_at}\"");
    }
    if let Some(interval) = recording.snapshot_interval_s {
        let _ = writeln!(conf, "SNAPSHOT_INTERVAL={interval}");
    }
    conf
}

/// One site per line with a leading dot, so Squid also allows the subdomains.
fn allowlist(settings: &Settings) -> Option<String> {
    let sites = &settings.network.allowed_domains;
    if sites.is_empty() {
        return None;
    }
    let mut list = GENERATED.to_owned();
    for site in sites {
        let _ = writeln!(list, ".{}", site.trim_start_matches('.'));
    }
    Some(list)
}

/// Replaces the file in one step, so a crash never leaves a half-written file.
fn write(root: &Root, absolute: &str, content: &[u8], mode: u32) -> Result<(), Error> {
    let path = root.path(absolute);
    let describe = |action: &str| format!("{action} {absolute}");
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder).map_err(|e| Error::io(describe("creating the folder of"), e))?;
    }
    let temporary = path.with_extension("ck-tmp");
    fs::write(&temporary, content).map_err(|e| Error::io(describe("writing"), e))?;
    set_mode(&temporary, mode).map_err(|e| Error::io(describe("setting permissions of"), e))?;
    fs::rename(&temporary, &path).map_err(|e| Error::io(describe("replacing"), e))?;
    Ok(())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

// Permissions only exist on the stick (Linux); this keeps the crate building on Windows.
#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}
