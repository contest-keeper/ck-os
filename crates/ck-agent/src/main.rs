//! `ck-agent`: the program on the contest stick.
//!
//! For now it has one command, `apply-config`, which `ck-config.service` runs at every boot.
//! Log lines go to stderr, where the journal collects them.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use ck_agent_config::{Error, Root, apply_basics, apply_branding, load};
use ck_agent_uplink::{Queue, Uplink, recorder_state};
use clap::{Parser, Subcommand};

mod self_test;

/// The settings file is missing, so the image keeps its defaults. The service treats this
/// differently from a real failure.
const EXIT_NO_SETTINGS: u8 = 3;
/// Where waiting events are kept, below the root.
const QUEUE_FOLDER: &str = "var/lib/ck/queue";

#[derive(Parser)]
#[command(
    name = "ck-agent",
    version,
    about = "The Contest Keeper agent on the stick"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read the admin's settings from the EFI partition and apply them to this machine
    ApplyConfig {
        /// The folder that stands for `/` (only for tests; the stick uses `/`)
        #[arg(long, default_value = "/")]
        root: PathBuf,
    },
    /// Send a heartbeat to the Hub every few seconds (the settings say how often)
    Uplink {
        /// The folder that stands for `/` (only for tests; the stick uses `/`)
        #[arg(long, default_value = "/")]
        root: PathBuf,
        /// Send one heartbeat and stop, for tests
        #[arg(long)]
        once: bool,
    },
    /// Add an event to the queue `uplink` sends from, for tools and tests
    Enqueue {
        /// The folder that stands for `/` (only for tests; the stick uses `/`)
        #[arg(long, default_value = "/")]
        root: PathBuf,
        /// What happened: `login` or `submission`
        kind: String,
        /// The details, as JSON. Never a password, a cookie or a session id
        payload: String,
    },
    /// Report what is applied on this machine, for the VM test
    SelfTest {
        /// The folder that stands for `/` (only for tests; the stick uses `/`)
        #[arg(long, default_value = "/")]
        root: PathBuf,
        /// Do nothing unless the machine was started as a test machine
        #[arg(long)]
        only_if_requested: bool,
        /// Write the report to this serial port instead of printing it
        #[arg(long)]
        serial: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .without_time()
        .with_ansi(false)
        .init();

    match cli.command {
        Command::ApplyConfig { root } => apply_config(&Root::new(root)),
        Command::Uplink { root, once } => uplink(&root, once),
        Command::Enqueue {
            root,
            kind,
            payload,
        } => enqueue(&root, &kind, &payload),
        Command::SelfTest {
            root,
            only_if_requested,
            serial,
        } => self_test(&root, only_if_requested, serial.as_deref()),
    }
}

fn apply_config(root: &Root) -> ExitCode {
    let loaded = match load(root) {
        Ok(loaded) => loaded,
        Err(Error::SettingsMissing(path)) => {
            tracing::error!("no settings at {path}; keeping the image defaults");
            return ExitCode::from(EXIT_NO_SETTINGS);
        }
        Err(error) => {
            tracing::error!("cannot use the settings, keeping the image defaults: {error}");
            return ExitCode::FAILURE;
        }
    };
    // The stick id is not secret; the token and the file content are never logged.
    tracing::info!(stick = %loaded.config.stick_id, sha256 = %loaded.sha256, "settings read");

    if let Err(error) = apply_basics(root, &loaded).and_then(|()| apply_branding(root, &loaded)) {
        tracing::error!("applying the settings failed: {error}");
        return ExitCode::FAILURE;
    }
    if root.is_real_root() {
        set_running_hostname(&loaded.config.computer_name());
    }
    tracing::info!("settings applied");
    ExitCode::SUCCESS
}

fn self_test(
    root: &std::path::Path,
    only_if_requested: bool,
    serial: Option<&std::path::Path>,
) -> ExitCode {
    if only_if_requested && !self_test::requested(root) {
        tracing::info!("not a test machine, no self-test report");
        return ExitCode::SUCCESS;
    }
    let report = self_test::collect(root, &self_test::systemctl_state);
    let block = ck_selftest::to_serial_block(&report);
    let written = match serial {
        Some(port) => OpenOptions::new()
            .write(true)
            .open(port)
            .and_then(|mut port| port.write_all(block.as_bytes())),
        None => std::io::stdout().write_all(block.as_bytes()),
    };
    if let Err(error) = written {
        tracing::error!("cannot write the self-test report: {error}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// `/etc/hostname` is only read when the machine starts, which is before this runs, so the running
/// machine is told its new name as well.
fn set_running_hostname(name: &str) {
    match std::process::Command::new("hostname").arg(name).status() {
        Ok(status) if status.success() => tracing::info!("computer name set to {name}"),
        Ok(status) => tracing::warn!("could not set the computer name ({status})"),
        Err(error) => tracing::warn!("could not set the computer name: {error}"),
    }
}

fn uplink(root: &std::path::Path, once: bool) -> ExitCode {
    let mut uplink = match Uplink::load(root) {
        Ok(uplink) => uplink,
        // An unconfigured stick has nothing to report; stopping quietly keeps the service from
        // being restarted again and again.
        Err(error @ ck_agent_uplink::Error::NoSettings(_)) => {
            tracing::info!("not reporting: {error}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            tracing::error!("cannot start reporting: {error}");
            return ExitCode::FAILURE;
        }
    };
    let Some(url) = uplink.url() else {
        tracing::info!("the settings name no server, so nothing is reported");
        return ExitCode::SUCCESS;
    };
    tracing::info!(%url, "reporting to the Hub");
    let queue = match Queue::open(root.join(QUEUE_FOLDER)) {
        Ok(queue) => Some(queue),
        Err(error) => {
            tracing::warn!("events cannot be queued, only heartbeats are sent: {error}");
            None
        }
    };

    let mut failing = false;
    loop {
        let running = self_test::systemctl_state("ck-recorder").active == "active";
        let heartbeat = uplink.next_heartbeat(
            chrono::Utc::now(),
            recorder_state(uplink.recording_mode(), running),
        );
        let result = uplink.send(&url, &heartbeat);
        // Say it once when reporting starts to fail and once when it works again, not every 15 s.
        match &result {
            Ok(()) if failing => {
                failing = false;
                tracing::info!(seq = heartbeat.seq, "reporting to the Hub again");
            }
            Ok(()) if heartbeat.seq == 1 => tracing::info!("the first heartbeat was accepted"),
            Err(error) if !failing => {
                failing = true;
                tracing::warn!("the heartbeat failed, trying again every interval: {error}");
            }
            Ok(()) | Err(_) => {}
        }
        if let (Some(queue), Some(events_url)) = (&queue, uplink.events_url()) {
            match uplink.flush(queue, &events_url) {
                Ok(0) => {}
                Ok(sent) => tracing::info!(sent, "events delivered to the Hub"),
                Err(error) => {
                    tracing::warn!("events are waiting, the Hub did not take them: {error}");
                }
            }
        }
        if once {
            return if result.is_ok() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            };
        }
        std::thread::sleep(uplink.interval());
    }
}

fn enqueue(root: &std::path::Path, kind: &str, payload: &str) -> ExitCode {
    let kind = match kind {
        "login" => ck_protocol::EventKind::Login,
        "submission" => ck_protocol::EventKind::Submission,
        other => {
            tracing::error!("unknown event kind '{other}': use login or submission");
            return ExitCode::FAILURE;
        }
    };
    let payload: serde_json::Value = match serde_json::from_str(payload) {
        Ok(value) => value,
        Err(error) => {
            tracing::error!("the payload is not JSON: {error}");
            return ExitCode::FAILURE;
        }
    };
    let result = Uplink::load(root).and_then(|uplink| {
        let queue = Queue::open(root.join(QUEUE_FOLDER))?;
        let (stick, boot) = uplink.identity();
        queue.push(stick, boot, kind, payload, chrono::Utc::now())
    });
    match result {
        Ok(event) => {
            tracing::info!(seq = event.seq, "event queued");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!("cannot queue the event: {error}");
            ExitCode::FAILURE
        }
    }
}
