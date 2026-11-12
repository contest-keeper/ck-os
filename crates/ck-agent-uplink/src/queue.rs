//! The events waiting to be sent, kept on disk so they survive a reboot or a network drop.
//!
//! One file per event, named by a counter that only goes up, so the order of sending is the order
//! of happening even across boots. A file is written under a temporary name and renamed, so a
//! power cut never leaves half an event. The chain state (the sequence number and the hash of the
//! last event) is kept next to them, so a restarted agent carries on the same chain.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use ck_protocol::{Event, EventKind, FIRST_PREV_HASH, StickId};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Error;

/// More than this many waiting events means the Hub has been out of reach for a very long time.
/// New events are refused then, so the older ones, which are the evidence, stay whole.
pub const MAX_PENDING: usize = 10_000;
/// An event is a few lines of text. A bigger payload is not one.
pub const MAX_PAYLOAD_BYTES: usize = 16 * 1024;

const STATE_FILE: &str = "state.json";

#[derive(Default, Serialize, Deserialize)]
struct State {
    /// The number of the last event file; the next one gets one more.
    counter: u64,
    boot_id: String,
    last_seq: u64,
    last_hash: String,
}

pub struct Queue {
    dir: PathBuf,
}

impl Queue {
    /// Opens the queue in `dir`, creating the folder when it is not there.
    ///
    /// # Errors
    /// Fails when the folder cannot be created.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, Error> {
        let dir = dir.into();
        fs::create_dir_all(&dir).map_err(|e| queue_error("creating the queue folder", &e))?;
        Ok(Self { dir })
    }

    /// Adds an event to the end of the chain of this boot and returns it. A new boot starts a new
    /// chain at sequence 1.
    ///
    /// # Errors
    /// [`Error::Queue`] when the queue is full, the payload is too big, or the disk fails.
    pub fn push(
        &self,
        stick_id: &StickId,
        boot_id: &str,
        kind: EventKind,
        payload: Value,
        now: DateTime<Utc>,
    ) -> Result<Event, Error> {
        let size = serde_json::to_vec(&payload).map_or(0, |bytes| bytes.len());
        if size > MAX_PAYLOAD_BYTES {
            return Err(Error::Queue(format!(
                "the payload is {size} bytes, at most {MAX_PAYLOAD_BYTES} are allowed"
            )));
        }
        if self.pending()?.len() >= MAX_PENDING {
            return Err(Error::Queue(format!(
                "{MAX_PENDING} events are already waiting; the Hub has been unreachable for too long"
            )));
        }

        let mut state = self.state()?;
        if state.boot_id != boot_id {
            boot_id.clone_into(&mut state.boot_id);
            state.last_seq = 0;
            FIRST_PREV_HASH.clone_into(&mut state.last_hash);
        }
        let event = Event {
            stick_id: stick_id.clone(),
            boot_id: boot_id.to_owned(),
            seq: state.last_seq + 1,
            prev_hash: state.last_hash.clone(),
            sent_at: now,
            kind,
            payload,
        };
        state.counter += 1;
        state.last_seq = event.seq;
        state.last_hash = ck_crypto::sha256_hex(&event.canonical_bytes());

        // The event first, then the state. A crash between the two repeats this sequence number
        // with the same previous hash, and the file is simply written again.
        write_atomically(
            &self.dir.join(event_file(state.counter)),
            &serde_json::to_vec(&event).expect("an event always serializes"),
        )?;
        write_atomically(
            &self.dir.join(STATE_FILE),
            &serde_json::to_vec(&state).expect("the state always serializes"),
        )?;
        Ok(event)
    }

    /// The waiting events, oldest first, each with the number of its file. A file that cannot be
    /// read is set aside as `.bad` so it does not block the ones after it.
    ///
    /// # Errors
    /// Fails when the folder cannot be read.
    pub fn pending(&self) -> Result<Vec<(u64, Event)>, Error> {
        let mut found = Vec::new();
        let entries = fs::read_dir(&self.dir).map_err(|e| queue_error("reading the queue", &e))?;
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(counter) = name
                .strip_prefix("event-")
                .and_then(|rest| rest.strip_suffix(".json"))
                .and_then(|digits| digits.parse::<u64>().ok())
            else {
                continue;
            };
            let event = fs::read(entry.path())
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Event>(&bytes).ok());
            if let Some(event) = event {
                found.push((counter, event));
            } else {
                tracing::warn!("the queued file {name} is not an event; setting it aside");
                let _ = fs::rename(entry.path(), entry.path().with_extension("bad"));
            }
        }
        found.sort_by_key(|(counter, _)| *counter);
        Ok(found)
    }

    /// Takes a sent event out of the queue.
    ///
    /// # Errors
    /// Fails when the file cannot be removed.
    pub fn remove(&self, counter: u64) -> Result<(), Error> {
        match fs::remove_file(self.dir.join(event_file(counter))) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(queue_error("removing a sent event", &e)),
        }
    }

    fn state(&self) -> Result<State, Error> {
        match fs::read(self.dir.join(STATE_FILE)) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|e| Error::Queue(format!("the queue state is not valid: {e}"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
            Err(e) => Err(queue_error("reading the queue state", &e)),
        }
    }
}

fn event_file(counter: u64) -> String {
    format!("event-{counter:012}.json")
}

fn queue_error(what: &str, error: &std::io::Error) -> Error {
    Error::Queue(format!("{what}: {error}"))
}

pub(crate) fn write_atomically(path: &Path, content: &[u8]) -> Result<(), Error> {
    let temporary = path.with_extension("tmp");
    let result = (|| -> std::io::Result<()> {
        let mut file: File = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temporary)?;
        file.write_all(content)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    result.map_err(|e| {
        let _ = fs::remove_file(&temporary);
        queue_error("writing the queue", &e)
    })
}
