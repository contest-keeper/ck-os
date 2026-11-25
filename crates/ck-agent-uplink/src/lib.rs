//! Sends the stick's heartbeats to the Hub.
//!
//! A heartbeat says the stick is alive and which settings it runs. It is built from the machine's own
//! files, signed with the stick's secret and sent over a direct connection, never through the
//! proxy settings of the contestant's session.

mod commands;
mod enrolment;
mod queue;
mod recordings;

pub use commands::Executor;
pub use queue::{MAX_PAYLOAD_BYTES, MAX_PENDING, Queue};
pub use recordings::RecordingError;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ck_agent_config::{PendingEnrolment, Root, read_applied_config};
use ck_protocol::{
    ConfigTrust, HEARTBEAT_PATH, Heartbeat, HeartbeatReply, RecorderState, RecorderStatus,
    RecordingMode, SIGNATURE_HEADER, STICK_HEADER, StickConfig, Timestamp,
};

/// The root-only copy of the settings that `ck-agent apply-config` writes at boot.
const CONFIG_COPY: &str = "etc/ck/config.json";
/// Where events go. The Hub answers 409 for one it already has, which counts as sent.
pub const EVENTS_PATH: &str = "/api/v1/events";
/// Where the last heartbeat sequence is kept, in the folder systemd makes writable for the uplink.
/// Events sent in one `flush`: each one is a request with its own connection.
const MAX_PER_FLUSH: usize = 100;
/// Time after which a flush stops starting new requests.
const FLUSH_BUDGET: Duration = Duration::from_secs(20);
const SEQUENCE_FILE: &str = "var/lib/ck/queue/heartbeat.json";
/// The most the Hub may answer with. A few commands are a few kilobytes.
const MAX_ANSWER_BYTES: u64 = 64 * 1024;
/// How long one heartbeat may take. A slow network must not make the next one late.
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("there are no settings at /{CONFIG_COPY}; the agent has not applied them yet")]
    NoSettings,
    #[error("the settings at /{CONFIG_COPY} are not valid: {0}")]
    BadSettings(String),
    #[error("cannot read /{CONFIG_COPY}: {0}")]
    Io(#[source] std::io::Error),
    #[error("the Hub did not accept the heartbeat: HTTP {0}")]
    Rejected(u16),
    #[error("cannot reach the Hub: {0}")]
    Unreachable(String),
    #[error("the event queue: {0}")]
    Queue(String),
    #[error("the Hub address must use HTTPS for enrolment")]
    EnrolmentAddress,
    #[error("the Hub rejected the enrolment: HTTP {0}")]
    EnrolmentRejected(u16),
    #[error("cannot reach the Hub to enrol this stick")]
    EnrolmentConnection,
    #[error("the Hub did not confirm this stick's enrolment")]
    EnrolmentAnswer,
    #[error("this stick has not enrolled yet")]
    EnrolmentPending,
    #[error("cannot use the enrolment state: {0}")]
    EnrolmentState(#[source] ck_agent_config::Error),
}

/// Whether this image carries both the screen helper and the systemd service that runs it.
pub fn screen_unlock_supported(root: &Path) -> bool {
    root.join("usr/local/sbin/ck-screen").is_file()
        && root.join("etc/systemd/system/ck-screen@.service").is_file()
}

/// One stick's reporting state: who it is, which boot this is, and the next sequence number.
pub struct Uplink {
    root: PathBuf,
    config: StickConfig,
    enrolment: Option<PendingEnrolment>,
    boot_id: String,
    seq: u64,
}

impl Uplink {
    /// Reads the settings the agent applied. `root` is `/` on the stick, a folder in tests.
    pub fn load(root: &Path) -> Result<Self, Error> {
        let bytes = fs::read(root.join(CONFIG_COPY)).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                Error::NoSettings
            } else {
                Error::Io(e)
            }
        })?;
        let (config, enrolment) = read_applied_config(&Root::new(root), &bytes)
            .map_err(|error| Error::BadSettings(error.to_string()))?;
        let problems = config.validate();
        if !problems.is_empty() {
            let text: Vec<String> = problems.iter().map(ToString::to_string).collect();
            return Err(Error::BadSettings(text.join("; ")));
        }
        let boot_id = read_trimmed(root, "proc/sys/kernel/random/boot_id").unwrap_or_default();
        // A restart of the service in the middle of a boot carries on the sequence. Starting again
        // at 1 would make the Hub refuse every heartbeat until it passed the old number.
        let seq = saved_sequence(root, &boot_id);
        Ok(Self {
            root: root.to_path_buf(),
            config,
            enrolment,
            boot_id,
            seq,
        })
    }

    /// The address heartbeats go to, or `None` when the admin set no server (no reporting).
    #[must_use]
    pub fn url(&self) -> Option<String> {
        self.address(HEARTBEAT_PATH)
    }

    fn address(&self, path: &str) -> Option<String> {
        let base = self.config.settings.api.url.trim_end_matches('/');
        (!base.is_empty()).then(|| format!("{base}{path}"))
    }

    #[must_use]
    pub fn interval(&self) -> Duration {
        Duration::from_secs(self.config.settings.api.heartbeat_s.max(1))
    }

    #[must_use]
    pub fn recording_mode(&self) -> RecordingMode {
        self.config.settings.recording.mode
    }

    /// The next heartbeat. The sequence number goes up by one each time, starting at 1 every boot,
    /// so the Hub can see when a message was lost.
    pub fn next_heartbeat(&mut self, now: Timestamp, recorder: RecorderState) -> Heartbeat {
        self.seq += 1;
        if let Err(error) = save_sequence(&self.root, &self.boot_id, self.seq) {
            tracing::warn!("the heartbeat sequence cannot be kept: {error}");
        }
        let settings = &self.config.settings;
        Heartbeat {
            screen_unlock: screen_unlock_supported(&self.root),
            stick_id: self.config.stick_id.clone(),
            boot_id: self.boot_id.clone(),
            seq: self.seq,
            sent_at: now,
            uptime_s: uptime_seconds(&self.root),
            hostname: read_trimmed(&self.root, "proc/sys/kernel/hostname").unwrap_or_default(),
            image_version: read_trimmed(&self.root, "etc/ck/image-version").unwrap_or_default(),
            config_sha256: read_trimmed(&self.root, "etc/ck/config.sha256").unwrap_or_default(),
            team: settings.team.trim().to_owned(),
            site: settings.site.trim().to_owned(),
            recording: RecorderStatus {
                mode: settings.recording.mode,
                state: recorder,
            },
            config_trust: config_trust(&self.root),
        }
    }

    /// Signs `heartbeat` with the stick's secret and sends it: [`Error::Rejected`] when the Hub
    /// answers with an error status, [`Error::Unreachable`] when there is no answer.
    pub fn send(&self, url: &str, heartbeat: &Heartbeat) -> Result<(), Error> {
        self.send_heartbeat(url, heartbeat).map(|_| ())
    }

    /// Like [`Uplink::send`], and also returns what the Hub answered: the commands for this stick.
    /// An answer that is empty or not a reply (an older Hub) means no commands, not an error.
    pub fn send_heartbeat(
        &self,
        url: &str,
        heartbeat: &Heartbeat,
    ) -> Result<HeartbeatReply, Error> {
        let body = serde_json::to_vec(heartbeat).expect("a heartbeat always serializes");
        let answer = self.post(url, &body)?;
        if answer.iter().all(u8::is_ascii_whitespace) {
            return Ok(HeartbeatReply::default());
        }
        Ok(serde_json::from_slice(&answer).unwrap_or_else(|error| {
            tracing::warn!("the Hub's answer to the heartbeat is not a reply: {error}");
            HeartbeatReply::default()
        }))
    }

    /// The address events go to, or `None` when the admin set no server.
    #[must_use]
    pub fn events_url(&self) -> Option<String> {
        self.address(EVENTS_PATH)
    }

    /// The stick this agent reports for, and the id of this boot: what the queue needs to chain
    /// events.
    #[must_use]
    pub fn identity(&self) -> (&ck_protocol::StickId, &str) {
        (&self.config.stick_id, &self.boot_id)
    }

    /// Sends the waiting events in order, taking each out of the queue once the Hub has it, and
    /// stops at the first one that does not get through so the order is kept. Returns how many
    /// were sent.
    ///
    /// A 409 means the Hub already has that event (its answer was lost earlier), so it counts as
    /// sent. A 400, 413 or 422 means it never will: the event is set aside as `.refused`. An error is returned only when nothing was sent; events not sent stay in the queue.
    ///
    /// At most `MAX_PER_FLUSH` events go in one call, so a long backlog does not hold up the
    /// heartbeats and commands that share the loop; the next round sends the rest.
    pub fn flush(&self, queue: &Queue, url: &str) -> Result<usize, Error> {
        let mut sent = 0;
        let started = Instant::now();
        for (counter, event) in queue.pending()?.into_iter().take(MAX_PER_FLUSH) {
            // Slow answers must not hold the loop for minutes either.
            if sent > 0 && started.elapsed() >= FLUSH_BUDGET {
                break;
            }
            match self.post(url, &event.canonical_bytes()) {
                Ok(_) | Err(Error::Rejected(409)) => {
                    queue.remove(counter)?;
                    sent += 1;
                }
                // The Hub will never take this one; keeping it would block every later event.
                Err(Error::Rejected(status @ (400 | 413 | 422))) => {
                    tracing::warn!(
                        status,
                        "the Hub refuses an event for good; setting it aside"
                    );
                    queue.set_aside(counter)?;
                }
                Err(error) => return if sent == 0 { Err(error) } else { Ok(sent) },
            }
        }
        Ok(sent)
    }

    /// Posts `body` signed with the stick's secret and returns the answer's body, at most
    /// `MAX_ANSWER_BYTES` of it.
    fn post(&self, url: &str, body: &[u8]) -> Result<Vec<u8>, Error> {
        if self.enrolment_pending()? {
            return Err(Error::EnrolmentPending);
        }
        let signature = ck_crypto::sign(&self.config.token, body);

        // `proxy(None)`: the Hub is reached directly, whatever the session's proxy settings say.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .proxy(None)
            // An answer from somewhere else (a venue's login page) is not the Hub's answer.
            .max_redirects(0)
            .timeout_global(Some(SEND_TIMEOUT))
            .build()
            .into();
        let result = agent
            .post(url)
            .header(STICK_HEADER, self.config.stick_id.as_str())
            .header(SIGNATURE_HEADER, &signature)
            .content_type("application/json")
            .send(body);
        match result {
            Ok(mut response) if response.status().is_success() => response
                .body_mut()
                .with_config()
                .limit(MAX_ANSWER_BYTES)
                .read_to_vec()
                .map_err(|e| Error::Unreachable(e.to_string())),
            Ok(response) => Err(Error::Rejected(response.status().as_u16())),
            Err(ureq::Error::StatusCode(status)) => Err(Error::Rejected(status)),
            Err(other) => Err(Error::Unreachable(other.to_string())),
        }
    }
}

/// Where `ck-recorder` writes what it is doing: `waiting`, `recording` or `lowdisk`.
pub const RECORDER_STATE_FILE: &str = "run/ck-recorder/state";

/// What the recorder is doing, from the mode the admin chose and whether `ck-recorder` runs.
#[must_use]
pub fn recorder_state(root: &Path, mode: RecordingMode, service_active: bool) -> RecorderState {
    if mode == RecordingMode::Off {
        return RecorderState::Off;
    }
    if !service_active {
        return RecorderState::Waiting;
    }
    // The service is active while it waits for its start time, so it says what it does itself. A
    // recorder that says nothing is not claimed to record.
    match read_trimmed(root, RECORDER_STATE_FILE).as_deref() {
        Some("recording") => RecorderState::Recording,
        Some("lowdisk") => RecorderState::LowDisk,
        _ => RecorderState::Waiting,
    }
}

/// What the stick made of its settings file at boot, as `ck-agent apply-config` kept it. A stick whose
/// agent never kept it says nothing.
fn config_trust(root: &Path) -> Option<ConfigTrust> {
    let text = read_trimmed(root, "etc/ck/config.trust")?;
    match text.split_whitespace().next()? {
        "verified" => Some(ConfigTrust::Verified),
        "rejected" => Some(ConfigTrust::Rejected),
        "unchecked" => Some(ConfigTrust::Unchecked),
        _ => None,
    }
}

fn read_trimmed(root: &Path, relative: &str) -> Option<String> {
    let text = fs::read_to_string(root.join(relative)).ok()?;
    Some(text.trim().to_owned())
}

fn uptime_seconds(root: &Path) -> u64 {
    read_trimmed(root, "proc/uptime")
        .and_then(|text| {
            text.split_whitespace()
                .next()?
                .split('.')
                .next()?
                .parse()
                .ok()
        })
        .unwrap_or(0)
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SavedSequence {
    boot_id: String,
    seq: u64,
}

/// The last sequence sent in this boot, or 0 when there is none: a new boot starts at 1.
fn saved_sequence(root: &Path, boot_id: &str) -> u64 {
    fs::read(root.join(SEQUENCE_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SavedSequence>(&bytes).ok())
        .filter(|saved| saved.boot_id == boot_id)
        .map_or(0, |saved| saved.seq)
}

fn save_sequence(root: &Path, boot_id: &str, seq: u64) -> Result<(), Error> {
    let path = root.join(SEQUENCE_FILE);
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder).map_err(|e| Error::Queue(e.to_string()))?;
    }
    let saved = SavedSequence {
        boot_id: boot_id.to_owned(),
        seq,
    };
    queue::write_atomically(
        &path,
        &serde_json::to_vec(&saved).expect("a sequence serializes"),
    )
}
