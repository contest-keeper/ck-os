//! `ck-agent`: the program on the contest stick. Each service of the image runs one of its
//! commands. Log lines go to stderr, where the journal collects them.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;

use ck_agent_config::{
    Error, Root, Trust, apply_basics, apply_branding, bluetooth_unblock_needed,
    finish_bluetooth_unblock, load_for_boot, write_trust,
};
use ck_agent_uplink::{Queue, Uplink, recorder_state};
use clap::{Parser, Subcommand};

mod admin_account;
mod hub_commands;
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
    about = "The Contest Keeper agent on the stick",
    long_about = "The Contest Keeper agent on the stick. Supports Keeper OS enrolment schema 3. Checks settings signatures (signed settings 1)."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read the admin's settings from the CKCONFIG partition and apply them to this machine
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
    /// Upload a batch of completed recordings and apply local retention
    UploadRecordings {
        /// The folder that stands for / in tests
        #[arg(long, default_value = "/")]
        root: PathBuf,
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
    /// Serve the proxy's ICAP requests: tell the Hub who logs in and what they submit, and on a
    /// machine started for observation also write what the traffic looks like, never its values
    Capture {
        /// The folder that stands for `/` (only for tests; the stick uses `/`)
        #[arg(long, default_value = "/")]
        root: PathBuf,
        /// Where to listen; only this machine can connect
        #[arg(long, default_value = "127.0.0.1:1344")]
        listen: String,
        /// Where the observation lines go, for example the serial port; the journal when left out
        #[arg(long)]
        write_to: Option<PathBuf>,
        /// Write the observation lines only on a machine started with the OEM string `ck-observe=1`
        #[arg(long)]
        observe_if_requested: bool,
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

/// Squid's own access log, in its default format.
const PROXY_LOG: &str = "var/log/squid/access.log";
/// Where the place reached in that log is kept, so a restart does not send it again.
const NETWORK_STATE: &str = "var/lib/ck/netlog/offset";
const NETWORK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);

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
        Command::UploadRecordings { root } => upload_recordings(&root),
        Command::Enqueue {
            root,
            kind,
            payload,
        } => enqueue(&root, &kind, &payload),
        Command::Capture {
            root,
            listen,
            write_to,
            observe_if_requested,
        } => capture(&root, &listen, write_to.as_deref(), observe_if_requested),
        Command::SelfTest {
            root,
            only_if_requested,
            serial,
        } => self_test(&root, only_if_requested, serial.as_deref()),
    }
}

fn apply_config(root: &Root) -> ExitCode {
    let booted = match load_for_boot(root) {
        Ok(booted) => booted,
        Err(Error::SettingsMissing(path)) => {
            tracing::error!("no settings at {path}; keeping the image defaults");
            return ExitCode::from(EXIT_NO_SETTINGS);
        }
        Err(Error::SettingsRejected { sha256, reason }) => {
            tracing::error!(
                "the settings file was refused ({reason}) and no settings were verified before it; keeping the image defaults"
            );
            record_trust(root, &Trust::Rejected(sha256));
            return ExitCode::from(EXIT_NO_SETTINGS);
        }
        Err(error @ Error::KeysMissing(_)) => {
            tracing::error!("no settings can be checked, so none are applied: {error}");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            tracing::error!("cannot use the settings, keeping the image defaults: {error}");
            return ExitCode::FAILURE;
        }
    };
    record_trust(root, &booted.trust);
    let loaded = booted.loaded;
    // The stick id is not secret; the token and the file content are never logged.
    tracing::info!(stick = %loaded.config.stick_id, sha256 = %loaded.sha256, "settings read");

    // A failing step must not skip the ones after it: the admin password, the hostname and the radio
    // blocks do not depend on the files written before them. The exit code still says it failed.
    let mut failed = false;
    if let Err(error) = apply_basics(root, &loaded) {
        tracing::error!("applying the settings failed: {error}");
        failed = true;
    }
    if let Err(error) = apply_branding(root, &loaded) {
        tracing::error!("applying the branding failed: {error}");
        failed = true;
    }
    if root.is_real_root() {
        set_running_hostname(&loaded.config.computer_name());
        if loaded.config.settings.block_bluetooth {
            block_bluetooth();
        } else if bluetooth_unblock_needed(root) {
            if unblock_bluetooth() {
                if let Err(error) = finish_bluetooth_unblock(root) {
                    tracing::error!("cannot note that Bluetooth is on again: {error}");
                    failed = true;
                }
            } else {
                // The note stays, so the next boot tries again.
                failed = true;
            }
        }
        if loaded.config.settings.block_usb_storage {
            apply_usb_storage_rules();
        }
        set_admin_password(loaded.config.settings.admin_password_hash.as_deref());
        // Also when the Wi-Fi was removed: the old connection file is gone and NetworkManager must forget it.
        reload_network_connections();
    }
    if failed {
        return ExitCode::FAILURE;
    }
    tracing::info!("settings applied");
    ExitCode::SUCCESS
}

/// Keeps what the stick made of its settings file, which every heartbeat reports. Failing to keep it
/// never stops the settings from being applied.
fn record_trust(root: &Root, trust: &Trust) {
    if let Err(error) = write_trust(root, trust) {
        tracing::error!("cannot keep what the settings check found: {error}");
    }
}

/// Gives `ckadmin` the password the admin chose. Without a hash the account is locked, also when an
/// earlier boot had set a password. A failed `chpasswd`
/// leaves it locked (nobody can use sudo); a failed lock is logged, and the rest of the settings still apply.
fn set_admin_password(hash: Option<&str>) {
    match admin_account::set_password(
        std::path::Path::new("chpasswd"),
        std::path::Path::new("passwd"),
        hash,
    ) {
        Ok(admin_account::Outcome::Set) => tracing::info!("admin password set"),
        Ok(admin_account::Outcome::Locked) => {
            tracing::info!("no admin password in the settings; the admin account is locked");
        }
        Err(error) => {
            tracing::error!("cannot set the admin password, the account stays locked: {error}");
        }
    }
}

/// Makes udev read the rules `apply-config` wrote and run them on the devices that are already there.
fn apply_usb_storage_rules() {
    let mut activated = true;
    for arguments in [
        &["control", "--reload"][..],
        &["trigger", "--subsystem-match=usb", "--action=add"][..],
        &["settle", "--timeout=20"][..],
    ] {
        match std::process::Command::new("udevadm")
            .args(arguments)
            .status()
        {
            Ok(status) if status.success() => {}
            Ok(status) => {
                activated = false;
                tracing::warn!("udevadm {} failed ({status})", arguments[0]);
            }
            Err(error) => {
                activated = false;
                tracing::warn!("cannot run udevadm, USB storage is not blocked: {error}");
            }
        }
        // Without a running udev the later steps would only wait for events that never come.
        if !activated {
            break;
        }
        // Without a running udev the later steps would only wait for events that never come.
        if !activated {
            break;
        }
    }
    if activated {
        tracing::info!("USB storage blocked, except the stick itself");
    }
}

/// Switches Bluetooth off for this boot. It runs at every boot, because the radio's state is not
/// kept across reboots on every machine. A machine without the tool or a radio is not an error.
fn block_bluetooth() {
    match std::process::Command::new("rfkill")
        .args(["block", "bluetooth"])
        .status()
    {
        Ok(status) if status.success() => tracing::info!("Bluetooth blocked"),
        Ok(status) => tracing::warn!("rfkill could not block Bluetooth ({status})"),
        Err(error) => tracing::warn!("cannot run rfkill, Bluetooth is not blocked: {error}"),
    }
}

/// Switches Bluetooth on again when an earlier boot blocked it and the settings no longer ask for it.
/// `true` when the radio is on again (or there is none to switch).
fn unblock_bluetooth() -> bool {
    match std::process::Command::new("rfkill")
        .args(["unblock", "bluetooth"])
        .status()
    {
        Ok(status) if status.success() => {
            tracing::info!("Bluetooth unblocked");
            true
        }
        Ok(status) => {
            tracing::warn!("rfkill could not unblock Bluetooth ({status})");
            false
        }
        Err(error) => {
            tracing::warn!("cannot run rfkill, Bluetooth stays as it is: {error}");
            false
        }
    }
}

/// Makes `NetworkManager` read the connection files again, so the venue network is known even when
/// `NetworkManager` started before the agent. No password is on the command line.
fn reload_network_connections() {
    match std::process::Command::new("nmcli")
        .args(["connection", "reload"])
        .status()
    {
        Ok(status) if status.success() => tracing::info!("network connections reloaded"),
        Ok(status) => tracing::warn!("nmcli could not reload the connections ({status})"),
        Err(error) => tracing::warn!("cannot run nmcli: {error}"),
    }
}

/// The login and the submissions go to the Hub on every stick. The shape of the traffic is written
/// only on a test machine that was started on purpose, so a real stick never records even that.
fn capture(
    root: &std::path::Path,
    listen: &str,
    write_to: Option<&std::path::Path>,
    observe_if_requested: bool,
) -> ExitCode {
    let observe = !observe_if_requested || self_test::oem_string_present(root, "ck-observe=1");
    let listener = match std::net::TcpListener::bind(listen) {
        Ok(listener) => listener,
        Err(error) => {
            tracing::error!("cannot listen on {listen}: {error}");
            return ExitCode::FAILURE;
        }
    };
    // Without observation the rule makes no notes, so this is never written to.
    let sink: Box<dyn Write + Send> = match (observe, write_to) {
        (false, _) => Box::new(std::io::sink()),
        (true, Some(path)) => match OpenOptions::new().append(true).create(true).open(path) {
            Ok(file) => Box::new(file),
            Err(error) => {
                tracing::error!("cannot write to {}: {error}", path.display());
                return ExitCode::FAILURE;
            }
        },
        (true, None) => Box::new(std::io::stderr()),
    };
    tracing::info!(%listen, observe, "watching the contest site for logins and submissions");
    let events = queued_events(root);
    follow_proxy_log(root, &events);
    match ck_capture::serve(
        &listener,
        Box::new(ck_capture::Codeforces::new(observe)),
        sink,
        Box::new(Shared(events)),
        "CKCAP ",
    ) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("capture stopped: {error}");
            ExitCode::FAILURE
        }
    }
}

/// The proxy's log of what it carried and refused goes to the Hub as network events, on every stick.
/// It runs in this process because the queue's chain is only kept in order by one writer.
fn follow_proxy_log(root: &std::path::Path, events: &std::sync::Arc<dyn ck_capture::EventSink>) {
    let log = root.join(PROXY_LOG);
    let state = root.join(NETWORK_STATE);
    let events = std::sync::Arc::clone(events);
    let since = time::OffsetDateTime::now_utc().unix_timestamp() - uptime_seconds(root);
    std::thread::spawn(move || {
        ck_capture::follow_network(&log, &state, since, &events, NETWORK_INTERVAL);
    });
}

/// The stick's clock, in UTC.
fn now() -> ck_protocol::Timestamp {
    ck_protocol::Timestamp::from(time::OffsetDateTime::now_utc())
}

/// Seconds since the machine started, so lines of an earlier boot in the log are not reported.
fn uptime_seconds(root: &std::path::Path) -> i64 {
    let text = std::fs::read_to_string(root.join("proc/uptime")).unwrap_or_default();
    // "12345.67 54321.00": the first number is the uptime.
    let seconds = text
        .split_whitespace()
        .next()
        .and_then(|n| n.split('.').next());
    seconds.and_then(|n| n.parse().ok()).unwrap_or(0)
}

/// Lets two threads use the one queue.
struct Shared(std::sync::Arc<dyn ck_capture::EventSink>);

impl ck_capture::EventSink for Shared {
    fn send(&self, kind: ck_capture::EventKind, payload: serde_json::Value) -> Result<(), String> {
        self.0.send(kind, payload)
    }
}

/// Events a rule makes go into the queue the uplink sends from, in the chain of this boot. A stick
/// with no settings has no identity to sign them with, so its events are dropped.
struct QueuedEvents {
    queue: Queue,
    stick: ck_protocol::StickId,
    boot: String,
    /// The queue's chain state is read and written again for each event: one at a time.
    one_at_a_time: std::sync::Mutex<()>,
}

impl ck_capture::EventSink for QueuedEvents {
    fn send(&self, kind: ck_capture::EventKind, payload: serde_json::Value) -> Result<(), String> {
        let kind = match kind {
            ck_capture::EventKind::Login => ck_protocol::EventKind::Login,
            ck_capture::EventKind::Submission => ck_protocol::EventKind::Submission,
            ck_capture::EventKind::Network => ck_protocol::EventKind::Network,
        };
        let _guard = self.one_at_a_time.lock().map_err(|e| e.to_string())?;
        self.queue
            .push(&self.stick, &self.boot, kind, payload, now())
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

fn queued_events(root: &std::path::Path) -> std::sync::Arc<dyn ck_capture::EventSink> {
    let queue = Queue::open(root.join(QUEUE_FOLDER));
    match (Uplink::load(root), queue) {
        (Ok(uplink), Ok(queue)) => {
            let (stick, boot) = uplink.identity();
            std::sync::Arc::new(QueuedEvents {
                queue,
                stick: stick.clone(),
                boot: boot.to_owned(),
                one_at_a_time: std::sync::Mutex::new(()),
            })
        }
        (uplink, queue) => {
            tracing::warn!(
                "events cannot be queued, so they are dropped ({:?}, {:?})",
                uplink.err().map(|e| e.to_string()),
                queue.err().map(|e| e.to_string())
            );
            std::sync::Arc::new(ck_capture::NoEvents)
        }
    }
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

fn upload_recordings(root: &std::path::Path) -> ExitCode {
    let uplink = match Uplink::load(root) {
        Ok(uplink) => uplink,
        Err(ck_agent_uplink::Error::NoSettings) => return ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!("cannot read recording settings: {error}");
            return ExitCode::FAILURE;
        }
    };
    match uplink.upload_recordings() {
        Ok(sent) => {
            if sent > 0 {
                tracing::info!(sent, "recordings confirmed by the Hub");
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::warn!("recording upload or retention will retry later: {error}");
            ExitCode::FAILURE
        }
    }
}

fn uplink(root: &std::path::Path, once: bool) -> ExitCode {
    let mut uplink = match Uplink::load(root) {
        Ok(uplink) => uplink,
        // An unconfigured stick has nothing to report; stopping quietly keeps the service from
        // being restarted again and again.
        Err(error @ ck_agent_uplink::Error::NoSettings) => {
            tracing::info!("not reporting: {error}");
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            tracing::error!("cannot start reporting: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(error) = uplink.claim_enrolment() {
        tracing::warn!("the stick could not enrol: {error}");
        return ExitCode::FAILURE;
    }
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
            now(),
            recorder_state(root, uplink.recording_mode(), running),
        );
        let result = uplink.send_heartbeat(&url, &heartbeat);
        // Say it once when reporting starts to fail and once when it works again, not every 15 s.
        match &result {
            Ok(_) if failing => {
                failing = false;
                tracing::info!(seq = heartbeat.seq, "reporting to the Hub again");
            }
            Ok(_) if heartbeat.seq == 1 => tracing::info!("the first heartbeat was accepted"),
            Err(error) if !failing => {
                failing = true;
                tracing::warn!("the heartbeat failed, trying again every interval: {error}");
            }
            Ok(_) | Err(_) => {}
        }
        // The Hub's commands come with its answer. They run before the events are sent, so the
        // acknowledgements leave in the same round.
        if let (Ok(reply), Some(queue)) = (&result, &queue)
            && !reply.commands.is_empty()
        {
            let ran = uplink.process_commands(
                queue,
                &reply.commands,
                now(),
                &hub_commands::SystemExecutor,
            );
            tracing::info!(received = reply.commands.len(), ran, "Hub commands handled");
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
        queue.push(stick, boot, kind, payload, now())
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
