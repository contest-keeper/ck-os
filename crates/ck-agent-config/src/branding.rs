//! What contestants see on the machine: its name, the wallpaper, the Firefox bookmarks and the
//! welcome text.
//!
//! Like the rest of the agent's work, every step starts from what the admin chose and overwrites
//! its own files, so running it again at the next boot gives the same result.

use std::fs;
use std::io::ErrorKind;

use ck_protocol::{ESP_CONFIG_FILE, StickConfig};
use serde_json::{Value, json};

use crate::apply::write;
use crate::error::Error;
use crate::load::Loaded;
use crate::root::Root;

const HOSTNAME: &str = "/etc/hostname";
const HOSTS: &str = "/etc/hosts";
const EFI_FOLDER: &str = "/boot/efi/ck";
/// The patched `set_wallpaper.sh`, which the desktop runs at login, shows this file when it exists
/// and the image's default wallpaper otherwise.
const TEAM_WALLPAPER: &str = "/icpc/teamWallpaper.png";
const FIREFOX_POLICIES: &str = "/usr/lib/firefox/distribution/policies.json";
/// `ck-welcome`, which the image starts with the desktop, shows these two files when the text exists.
const WELCOME_TEXT: &str = "/etc/ck/welcome.txt";
const WELCOME_TITLE: &str = "/etc/ck/welcome-title";
/// A welcome text is a few lines; a bigger one is not what the admin meant to show in a window.
const MAX_WELCOME_BYTES: usize = 16 * 1024;
/// A wallpaper is a picture; anything bigger than this is not one the admin meant to use.
const MAX_WALLPAPER_BYTES: u64 = 16 * 1024 * 1024;

/// Fails when a file cannot be written, or the Firefox policies file is not valid JSON.
pub fn apply_branding(root: &Root, loaded: &Loaded) -> Result<(), Error> {
    let config = &loaded.config;
    apply_computer_name(root, config)?;
    apply_wallpaper(root, config)?;
    apply_firefox(root, config)?;
    apply_welcome(root, config)
}

fn apply_computer_name(root: &Root, config: &StickConfig) -> Result<(), Error> {
    let name = config.computer_name();
    write(root, HOSTNAME, format!("{name}\n").as_bytes(), 0o644)?;

    // The machine must resolve its own name, or sudo and many programs wait for a DNS timeout.
    let hosts = match fs::read_to_string(root.path(HOSTS)) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => "127.0.0.1 localhost\n".to_owned(),
        Err(e) => return Err(Error::io(format!("reading {HOSTS}"), e)),
    };
    write(
        root,
        HOSTS,
        hosts_with_name(&hosts, &name).as_bytes(),
        0o644,
    )
}

/// `hosts` with the `127.0.1.1` line set to `name`; the line is added when there is none.
fn hosts_with_name(hosts: &str, name: &str) -> String {
    let line = format!("127.0.1.1 {name}");
    let mut result = String::new();
    let mut replaced = false;
    for existing in hosts.lines() {
        if existing.split_whitespace().next() == Some("127.0.1.1") {
            if !replaced {
                result.push_str(&line);
                result.push('\n');
                replaced = true;
            }
        } else {
            result.push_str(existing);
            result.push('\n');
        }
    }
    if !replaced {
        result.push_str(&line);
        result.push('\n');
    }
    result
}

fn apply_wallpaper(root: &Root, config: &StickConfig) -> Result<(), Error> {
    let Some(name) = &config.settings.branding.wallpaper else {
        // No wallpaper chosen: take away one from an earlier boot, so the image's own shows again.
        return remove(root, TEAM_WALLPAPER);
    };

    // The settings file holds the stick's secret and the wallpaper is readable by everyone, so it
    // is never copied out under that name. FAT ignores case.
    if name.eq_ignore_ascii_case(ESP_CONFIG_FILE) {
        tracing::warn!("the wallpaper cannot be the settings file; keeping the current wallpaper");
        return Ok(());
    }
    // The settings check makes `name` a plain file name, so it cannot point outside the folder.
    let source = root.path(&format!("{EFI_FOLDER}/{name}"));
    let size = match fs::metadata(&source) {
        Ok(meta) if meta.is_file() => meta.len(),
        Ok(_) => {
            tracing::warn!("the wallpaper {name} is not a file; keeping the current wallpaper");
            return Ok(());
        }
        Err(e) if e.kind() == ErrorKind::NotFound => {
            tracing::warn!(
                "the wallpaper {name} is not on the stick; keeping the current wallpaper"
            );
            return Ok(());
        }
        Err(e) => return Err(Error::io(format!("reading the wallpaper {name}"), e)),
    };
    if size > MAX_WALLPAPER_BYTES {
        tracing::warn!("the wallpaper {name} is larger than {MAX_WALLPAPER_BYTES} bytes; not used");
        return Ok(());
    }
    let picture =
        fs::read(&source).map_err(|e| Error::io(format!("reading the wallpaper {name}"), e))?;
    write(root, TEAM_WALLPAPER, &picture, 0o644)
}

/// Sets the bookmarks and the home page. Everything else in the image's policies, such as the proxy
/// certificate, stays as it is. Settings the admin left empty keep the image's own.
fn apply_firefox(root: &Root, config: &StickConfig) -> Result<(), Error> {
    let branding = &config.settings.branding;
    if branding.bookmarks.is_empty() && branding.home_page.is_none() {
        return Ok(());
    }
    let text = match fs::read_to_string(root.path(FIREFOX_POLICIES)) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            tracing::warn!(
                "this image has no Firefox policies file; bookmarks and home page not set"
            );
            return Ok(());
        }
        Err(e) => return Err(Error::io(format!("reading {FIREFOX_POLICIES}"), e)),
    };
    let mut document: Value =
        serde_json::from_str(&text).map_err(|e| Error::PoliciesUnreadable(e.to_string()))?;
    let policies = document
        .get_mut("policies")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| Error::PoliciesUnreadable("there is no \"policies\" object".to_owned()))?;

    if !branding.bookmarks.is_empty() {
        let bookmarks: Vec<Value> = branding
            .bookmarks
            .iter()
            .map(|b| json!({ "Placement": "toolbar", "Title": b.title, "URL": b.url }))
            .collect();
        policies.insert("Bookmarks".to_owned(), Value::Array(bookmarks));
    }
    if let Some(url) = &branding.home_page {
        let home = policies
            .entry("Homepage")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| Error::PoliciesUnreadable("\"Homepage\" is not an object".to_owned()))?;
        home.insert("URL".to_owned(), json!(url));
        home.insert("StartPage".to_owned(), json!("homepage"));
        home.insert("Locked".to_owned(), json!(true));
    }

    let mut output = serde_json::to_string_pretty(&document)
        .map_err(|e| Error::PoliciesUnreadable(e.to_string()))?;
    output.push('\n');
    write(root, FIREFOX_POLICIES, output.as_bytes(), 0o644)
}

/// Writes the welcome text and its title, or takes them away when the admin chose no text, so a
/// text from an earlier boot does not stay.
fn apply_welcome(root: &Root, config: &StickConfig) -> Result<(), Error> {
    let branding = &config.settings.branding;
    let text = branding.welcome_text.trim();
    if text.len() > MAX_WELCOME_BYTES {
        tracing::warn!("the welcome text is longer than {MAX_WELCOME_BYTES} bytes; not shown");
    }
    if text.is_empty() || text.len() > MAX_WELCOME_BYTES {
        remove(root, WELCOME_TEXT)?;
        return remove(root, WELCOME_TITLE);
    }
    // The title is one line: the first line of the display name, or a plain "Welcome".
    let title = branding.display_name.lines().next().unwrap_or("").trim();
    let title = if title.is_empty() { "Welcome" } else { title };
    write(root, WELCOME_TEXT, format!("{text}\n").as_bytes(), 0o644)?;
    write(root, WELCOME_TITLE, format!("{title}\n").as_bytes(), 0o644)
}

fn remove(root: &Root, absolute: &str) -> Result<(), Error> {
    match fs::remove_file(root.path(absolute)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Error::io(format!("removing {absolute}"), e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_the_name_in_the_hosts_file_and_keeps_the_rest() {
        let hosts = "127.0.0.1 localhost\n127.0.1.1 icpc\n\n::1 ip6-localhost\n";
        assert_eq!(
            hosts_with_name(hosts, "team-a-3"),
            "127.0.0.1 localhost\n127.0.1.1 team-a-3\n\n::1 ip6-localhost\n"
        );
    }

    #[test]
    fn adds_the_line_when_the_hosts_file_has_none_and_never_twice() {
        assert_eq!(
            hosts_with_name("127.0.0.1 localhost\n", "x"),
            "127.0.0.1 localhost\n127.0.1.1 x\n"
        );
        assert_eq!(
            hosts_with_name("127.0.1.1 a\n127.0.1.1 b\n", "x"),
            "127.0.1.1 x\n"
        );
    }
}
