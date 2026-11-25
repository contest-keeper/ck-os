use std::fmt::Write as _;
use std::fs::{self, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use ck_protocol::{Settings, StickConfig, UploadMode, Wifi};

use crate::error::Error;
use crate::load::Loaded;
use crate::root::Root;
use crate::usb_storage::{BLOCK_USB_STORAGE, RULES as USB_STORAGE_RULES, boot_usb_devices, rules};

const CONFIG_COPY: &str = "/etc/ck/config.json";
const CONFIG_HASH: &str = "/etc/ck/config.sha256";
const RECORDING_CONF: &str = "/etc/ck/recording.conf";
const TEAM: &str = "/etc/ck/team";
const SITE: &str = "/etc/ck/site";
/// The Wi-Fi network of the venue, as a `NetworkManager` connection that only root can read.
pub const WIFI_CONNECTION: &str = "/etc/NetworkManager/system-connections/ck-wifi.nmconnection";
/// Exists while the settings ask for Bluetooth to be blocked. The self-test reads it back.
pub const BLOCK_BLUETOOTH: &str = "/etc/ck/block-bluetooth";
/// Whether an earlier boot asked for Bluetooth to be blocked (the marker the self-test reads back).
#[must_use]
pub fn bluetooth_marked_blocked(root: &Root) -> bool {
    root.path(BLOCK_BLUETOOTH).exists()
}
const SQUID_ALLOWLIST: &str = "/etc/squid/acl_allowed_domains.txt";

const GENERATED: &str = "# Written at boot by ck-agent from the admin tool settings\n";

/// Applies the settings that need no restart of anything: the root-only copy of the settings and
/// its hash, team and site, the recorder settings and the proxy allowlist.
///
/// Each file is replaced on its own, so a crash part-way leaves a mix of old and new files. That is
/// fine because the agent runs this at every boot and every step overwrites its file: the next
/// run brings all of them in line. The copy of the settings and its hash come last, so a failure
/// part-way never reports settings as applied that are not.
pub fn apply_basics(root: &Root, loaded: &Loaded) -> Result<(), Error> {
    let settings = &loaded.config.settings;

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
        recording_conf(&loaded.config).as_bytes(),
        0o644,
    )?;

    if settings.block_bluetooth {
        write(
            root,
            BLOCK_BLUETOOTH,
            b"1
",
            0o644,
        )?;
    } else {
        remove(root, BLOCK_BLUETOOTH)?;
    }

    if settings.block_usb_storage {
        write(root, BLOCK_USB_STORAGE, b"1\n", 0o644)?;
        let keep = boot_usb_devices(root);
        write(root, USB_STORAGE_RULES, rules(&keep).as_bytes(), 0o644)?;
    } else {
        remove(root, BLOCK_USB_STORAGE)?;
        remove(root, USB_STORAGE_RULES)?;
    }
    match &settings.wifi {
        Some(wifi) => write(
            root,
            WIFI_CONNECTION,
            wifi_connection(wifi).as_bytes(),
            0o600,
        )?,
        None => remove(root, WIFI_CONNECTION)?,
    }

    // No sites chosen: keep the image's own list rather than block everything.
    if let Some(list) = allowlist(settings) {
        keep_image_copy(root, SQUID_ALLOWLIST)?;
        write(root, SQUID_ALLOWLIST, list.as_bytes(), 0o644)?;
    } else {
        // A list written for an earlier boot is replaced by the image's own, not kept.
        tracing::warn!("no allowed sites in the settings; using the image's list");
        restore_image_copy(root, SQUID_ALLOWLIST)?;
    }

    // Last: the copy and its hash say "these settings are applied" (the self-test compares the hash, and a
    // refused file falls back to the copy), so they must not move ahead of the files they describe.
    write(root, CONFIG_COPY, &loaded.raw, 0o600)?;
    write(
        root,
        CONFIG_HASH,
        format!("{}\n", loaded.sha256).as_bytes(),
        0o644,
    )?;
    Ok(())
}

/// The `NetworkManager` keyfile for the venue network. The password is only in this file, which is
/// written with mode 0600 from the start, and in the root-only copy of the settings.
fn wifi_connection(wifi: &Wifi) -> String {
    let mut lines = vec![
        "[connection]".to_owned(),
        "id=ck-wifi".to_owned(),
        "uuid=6f0a8d52-3c1e-4f6b-9a5d-2f7e8b1c4a90".to_owned(),
        "type=wifi".to_owned(),
        "autoconnect=true".to_owned(),
        String::new(),
        "[wifi]".to_owned(),
        "mode=infrastructure".to_owned(),
        format!("ssid={}", keyfile_value(&wifi.ssid)),
        String::new(),
    ];
    if let Some(password) = &wifi.password {
        lines.push("[wifi-security]".to_owned());
        lines.push("key-mgmt=wpa-psk".to_owned());
        lines.push(format!("psk={}", keyfile_value(password.expose())));
        lines.push(String::new());
    }
    for line in ["[ipv4]", "method=auto", "", "[ipv6]", "method=auto"] {
        lines.push(line.to_owned());
    }
    lines.join("\n") + "\n"
}

/// A value as the keyfile reader wants it: a backslash is written twice and a space at either end
/// as a backslash and `s`, because the reader would drop the space.
fn keyfile_value(value: &str) -> String {
    let doubled = value.replace('\\', "\\\\");
    let core = doubled.trim_matches(' ');
    let leading = doubled.chars().take_while(|c| *c == ' ').count();
    let trailing = if core.is_empty() {
        0
    } else {
        doubled.chars().rev().take_while(|c| *c == ' ').count()
    };
    let space = "\\s";
    let mut out = space.repeat(leading);
    if core.is_empty() {
        return out;
    }
    out.push_str(core);
    out.push_str(&space.repeat(trailing));
    out
}

/// Where the image's own version of a file the settings can change is kept, the first time it is changed.
fn image_copy_path(absolute: &str) -> String {
    format!("{absolute}.ck-image")
}

/// Keeps the image's own version of `absolute` before the settings first replace it, so a setting that
/// is later taken away can bring it back. Only the first copy is kept: it is the image's, never ours.
pub(crate) fn keep_image_copy(root: &Root, absolute: &str) -> Result<(), Error> {
    let copy = image_copy_path(absolute);
    if root.path(&copy).exists() {
        return Ok(());
    }
    match fs::read(root.path(absolute)) {
        Ok(original) => write(root, &copy, &original, 0o644),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(format!("reading {absolute}"), e)),
    }
}

/// Puts the image's own version of `absolute` back, when the settings changed it before. Nothing to do
/// when they never did.
pub(crate) fn restore_image_copy(root: &Root, absolute: &str) -> Result<(), Error> {
    match fs::read(root.path(&image_copy_path(absolute))) {
        Ok(original) => write(root, absolute, &original, 0o644),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(format!("reading the image's {absolute}"), e)),
    }
}

/// Removes a file written by an earlier boot. A file that is not there is fine.
fn remove(root: &Root, absolute: &str) -> Result<(), Error> {
    match fs::remove_file(root.path(absolute)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(format!("removing {absolute}"), e)),
    }
}

/// Shell variables that `ck-recorder` reads with `.`.
fn recording_conf(config: &StickConfig) -> String {
    let recording = &config.settings.recording;
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
    if matches!(
        recording.upload,
        UploadMode::AfterContest | UploadMode::DuringContest
    ) {
        let _ = writeln!(conf, "UPLOAD_STICK={}", config.stick_id);
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
