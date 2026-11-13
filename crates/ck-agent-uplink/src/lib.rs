//! Sends the stick's heartbeats to the Hub.
//!
//! A heartbeat says the stick is alive and which settings it runs. It is built from the machine's own
//! files, signed with the stick's secret and sent over a direct connection, never through the
//! proxy settings of the contestant's session.

mod queue;

pub use queue::{MAX_PAYLOAD_BYTES, MAX_PENDING, Queue};

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use ck_protocol::{
    HEARTBEAT_PATH, Heartbeat, RecorderState, RecorderStatus, RecordingMode, SIGNATURE_HEADER,
    STICK_HEADER, StickConfig,
};

/// The root-only copy of the settings that `ck-agent apply-config` writes at boot.
const CONFIG_COPY: &str = "etc/ck/config.json";
/// Where events go. The Hub answers 409 for one it already has, which counts as sent.
pub const EVENTS_PATH: &str = "/api/v1/events";
/// Where the last heartbeat sequence is kept, in the folder systemd makes writable for the uplink.
const SEQUENCE_FILE: &str = "var/lib/ck/queue/heartbeat.json";
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
}

/// One stick's reporting state: who it is, which boot this is, and the next sequence number.
pub struct Uplink {
    root: PathBuf,
    config: StickConfig,
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
        let config: StickConfig =
            serde_json::from_slice(&bytes).map_err(|e| Error::BadSettings(e.to_string()))?;
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
    pub fn next_heartbeat(&mut self, now: DateTime<Utc>, recorder: RecorderState) -> Heartbeat {
        self.seq += 1;
        if let Err(error) = save_sequence(&self.root, &self.boot_id, self.seq) {
            tracing::warn!("the heartbeat sequence cannot be kept: {error}");
        }
        let settings = &self.config.settings;
        Heartbeat {
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
        }
    }

    /// Signs `heartbeat` with the stick's secret and sends it: [`Error::Rejected`] when the Hub
    /// answers with an error status, [`Error::Unreachable`] when there is no answer.
    pub fn send(&self, url: &str, heartbeat: &Heartbeat) -> Result<(), Error> {
        let body = serde_json::to_vec(heartbeat).expect("a heartbeat always serializes");
        self.post(url, &body)
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
    /// sent. An error is returned only when nothing was sent; events not sent stay in the queue.
    pub fn flush(&self, queue: &Queue, url: &str) -> Result<usize, Error> {
        let mut sent = 0;
        for (counter, event) in queue.pending()? {
            match self.post(url, &event.canonical_bytes()) {
                Ok(()) | Err(Error::Rejected(409)) => {
                    queue.remove(counter)?;
                    sent += 1;
                }
                Err(error) => return if sent == 0 { Err(error) } else { Ok(sent) },
            }
        }
        Ok(sent)
    }

    fn post(&self, url: &str, body: &[u8]) -> Result<(), Error> {
        let signature = ck_crypto::sign(&self.config.token, body);

        // `proxy(None)`: the Hub is reached directly, whatever the session's proxy settings say.
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .proxy(None)
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
            Ok(_) => Ok(()),
            Err(ureq::Error::StatusCode(status)) => Err(Error::Rejected(status)),
            Err(other) => Err(Error::Unreachable(other.to_string())),
        }
    }
}

/// What the recorder is doing, from the mode the admin chose and whether `ck-recorder` runs.
/// "Waiting" covers the start delay and a recorder that has stopped; the agent cannot tell them
/// apart from outside.
#[must_use]
pub fn recorder_state(mode: RecordingMode, service_active: bool) -> RecorderState {
    if mode == RecordingMode::Off {
        RecorderState::Off
    } else if service_active {
        RecorderState::Recording
    } else {
        RecorderState::Waiting
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
