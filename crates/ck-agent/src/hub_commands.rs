//! What a Hub command does to this machine. The checks that decide whether it may run are in
//! `ck-agent-uplink`; by the time a command gets here it is signed, current and new.

use std::fs;
use std::path::Path;
use std::process::Command;

use ck_agent_uplink::Executor;
use ck_protocol::Action;

/// Runs the commands with the system's own tools: `loginctl`, `systemctl` and `notify-send`. Every
/// argument is passed as an argument, never through a shell, so the text of a message cannot run
/// anything.
pub struct SystemExecutor;

impl Executor for SystemExecutor {
    fn run(&self, action: &Action) -> Result<(), String> {
        match action {
            Action::Message { text } => notify(text),
            Action::LockScreen => screen_command(false),
            Action::UnlockScreen => screen_command(true),
            Action::Shutdown => run("systemctl", &["poweroff"]),
            _ => Err("this agent does not know that command".to_owned()),
        }
    }
}

fn screen_command(unlock: bool) -> Result<(), String> {
    let root = Path::new("/");
    if ck_agent_uplink::screen_unlock_supported(root) {
        let unit = if unlock {
            "ck-screen@unlock.service"
        } else {
            "ck-screen@lock.service"
        };
        return run("systemctl", &["--no-ask-password", "start", unit]);
    }
    let keeper_image = fs::read_to_string(root.join("etc/ck/image-version"))
        .is_ok_and(|version| version.starts_with("keeper-os "));
    if unlock || keeper_image {
        return Err("this image has no Keeper screen service".to_owned());
    }
    // Keep the stock desktop lock behavior on images without the Keeper screen service.
    run("loginctl", &["lock-sessions"])
}

fn run(program: &str, arguments: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(arguments)
        .status()
        .map_err(|e| format!("cannot run {program}: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} ended with {status}"))
    }
}

/// Shows `text` on the screen of the person at the machine. The agent runs as root, so it speaks to
/// the session bus of the logged-in user.
fn notify(text: &str) -> Result<(), String> {
    let output = Command::new("loginctl")
        .args(["list-sessions", "--no-legend"])
        .output()
        .map_err(|e| format!("cannot run loginctl: {e}"))?;
    let listing = String::from_utf8_lossy(&output.stdout);
    let (uid, user) =
        desktop_session(&listing).ok_or_else(|| "nobody is at the screen".to_owned())?;
    let bus = format!("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus");
    run(
        "runuser",
        &[
            "-u",
            &user,
            "--",
            "env",
            &bus,
            "notify-send",
            "--app-name=Contest Keeper",
            "--urgency=critical",
            "--",
            "Message from the organisers",
            text,
        ],
    )
}

/// The user at the screen, from `loginctl list-sessions --no-legend` (columns: session, uid, user,
/// seat, tty). A graphical login has a seat; a remote or system session has none. Root and system
/// users are skipped.
fn desktop_session(listing: &str) -> Option<(u32, String)> {
    for line in listing.lines() {
        let columns: Vec<&str> = line.split_whitespace().collect();
        let [_, uid, user, seat, ..] = columns.as_slice() else {
            continue;
        };
        let Ok(uid) = uid.parse::<u32>() else {
            continue;
        };
        if uid >= 1000 && seat.starts_with("seat") {
            return Some((uid, (*user).to_owned()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_user_at_the_screen() {
        let listing = "  c1 1000 contestant seat0 tty7\n  c2 0 root - pts/0\n";
        assert_eq!(
            desktop_session(listing),
            Some((1000, "contestant".to_owned()))
        );
    }

    #[test]
    fn skips_remote_system_and_unreadable_sessions() {
        assert_eq!(desktop_session("  c2 0 root seat0 tty1\n"), None);
        assert_eq!(desktop_session("  c3 1001 ana - pts/1\n"), None);
        assert_eq!(desktop_session("garbage\n\n"), None);
        assert_eq!(desktop_session(""), None);
    }
}
