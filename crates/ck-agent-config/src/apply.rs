use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use ck_protocol::Settings;

use crate::error::Error;
use crate::load::Loaded;
use crate::root::Root;

const CONFIG_COPY: &str = "/etc/ck/config.json";
const CONFIG_HASH: &str = "/etc/ck/config.sha256";
const RECORDING_CONF: &str = "/etc/ck/recording.conf";
const TEAM: &str = "/etc/ck/team";
const SITE: &str = "/etc/ck/site";
const SQUID_ALLOWLIST: &str = "/etc/squid/acl_allowed_domains.txt";

const GENERATED: &str = "# Written at boot by ck-agent from the admin tool settings\n";

/// Applies the settings that need no restart of anything: the root-only copy of the settings and
/// its hash, team and site, the recorder settings and the proxy allowlist.
///
/// Each file is replaced on its own, so a crash part-way leaves a mix of old and new files. That is
/// fine because the agent runs this at every boot and every step overwrites its file: the next
/// run brings all of them in line.
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

/// One site per line with a leading dot, so Squid also allows the subdomains. Names are lowercased
/// and each one is listed once.
fn allowlist(settings: &Settings) -> Option<String> {
    let mut sites: Vec<String> = Vec::new();
    for site in &settings.network.allowed_domains {
        let site = site.trim_start_matches('.').to_ascii_lowercase();
        if !sites.contains(&site) {
            sites.push(site);
        }
    }
    if sites.is_empty() {
        return None;
    }
    let mut list = GENERATED.to_owned();
    for site in sites {
        let _ = writeln!(list, ".{site}");
    }
    Some(list)
}

/// Replaces the file in one step, so a crash never leaves a half-written file: the content goes to
/// a new file next to it, is flushed to disk, and is then renamed over the old one.
pub(crate) fn write(root: &Root, absolute: &str, content: &[u8], mode: u32) -> Result<(), Error> {
    let path = root.path(absolute);
    let folder = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(folder)
        .map_err(|e| Error::io(format!("creating the folder of {absolute}"), e))?;

    let mut temporary_name = path.as_os_str().to_owned();
    temporary_name.push(".ck-tmp");
    let temporary = PathBuf::from(temporary_name);

    let result =
        write_new_file(&temporary, content, mode).and_then(|()| fs::rename(&temporary, &path));
    if let Err(e) = result {
        let _ = fs::remove_file(&temporary);
        return Err(Error::io(format!("writing {absolute}"), e));
    }
    sync_folder(folder);
    Ok(())
}

/// Writes a new file that has its final `mode` from the start, so a root-only file is never readable
/// by anyone else, not even for a moment. A leftover from an interrupted run is replaced.
fn write_new_file(path: &Path, content: &[u8], mode: u32) -> std::io::Result<()> {
    let _ = fs::remove_file(path);
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    set_creation_mode(&mut options, mode);
    let mut file = options.open(path)?;
    file.write_all(content)?;
    file.sync_all()
}

#[cfg(unix)]
fn set_creation_mode(options: &mut OpenOptions, mode: u32) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(mode);
}

// Permissions only exist on the stick (Linux); this keeps the crate building on Windows.
#[cfg(not(unix))]
fn set_creation_mode(_options: &mut OpenOptions, _mode: u32) {}

/// Makes the rename survive a power loss. Best effort: not every system can flush a folder.
#[cfg(unix)]
fn sync_folder(folder: &Path) {
    if let Ok(handle) = fs::File::open(folder) {
        let _ = handle.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_folder(_folder: &Path) {}
