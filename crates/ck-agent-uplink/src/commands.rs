//! Commands the Hub sends to the stick (message, lock and unlock the screen, shut down).
//!
//! They arrive in the reply to a heartbeat. Each one is checked before anything runs: the signature,
//! the stick, the time window, and that its id was not run before. The design is in ck-protocol's
//! `docs/commands.md`. What a command does to the machine is behind [`Executor`], so the checks are
//! tested without touching a real system.

use std::fs;
use std::path::Path;

use ck_protocol::{AckResult, Action, CommandAck, EventKind, Refusal, SignedCommand, Timestamp};
use serde::{Deserialize, Serialize};

use crate::queue::write_atomically;
use crate::{Error, Queue, Uplink};

/// Where the ids of the commands already run are kept, next to the event queue.
const SEEN_FILE: &str = "var/lib/ck/queue/commands-seen.json";

/// More ids than this means something is flooding the stick with commands: it runs no more.
const MAX_SEEN: usize = 1000;

/// Does what a command says. The real one runs system tools; a test uses a fake.
pub trait Executor {
    /// # Errors
    /// A short reason, shown in the log only. It is never sent to the Hub.
    fn run(&self, action: &Action) -> Result<(), String>;
}

#[derive(Serialize, Deserialize)]
struct Seen {
    id: String,
    /// The command is refused as expired after this time, so the id need not be kept longer.
    expires_at: Timestamp,
}

impl Uplink {
    /// Checks and runs the commands of one heartbeat reply, one after the other, and queues an
    /// acknowledgement event for each. Returns how many ran without a problem.
    ///
    /// `now` is the stick's clock.
    pub fn process_commands(
        &self,
        queue: &Queue,
        commands: &[SignedCommand],
        now: Timestamp,
        executor: &dyn Executor,
    ) -> usize {
        let mut ran = 0;
        for signed in commands {
            let ack = self.run_command(signed, now, executor);
            if ack.result == AckResult::Ok {
                ran += 1;
            }
            let payload = serde_json::to_value(&ack).expect("an ack always serializes");
            let (stick, boot) = self.identity();
            if let Err(error) = queue.push(stick, boot, EventKind::CommandAck, payload, now) {
                tracing::warn!(command = %ack.command_id, "the acknowledgement cannot be queued: {error}");
            }
        }
        ran
    }

    fn run_command(
        &self,
        signed: &SignedCommand,
        now: Timestamp,
        executor: &dyn Executor,
    ) -> CommandAck {
        let command = &signed.command;
        let refuse = |reason: Refusal| {
            tracing::warn!(command = %command.id, %reason, "a command was refused");
            CommandAck {
                command_id: command.id.clone(),
                result: AckResult::Refused,
                refusal: Some(reason),
            }
        };
        let failed = || CommandAck {
            command_id: command.id.clone(),
            result: AckResult::Failed,
            refusal: None,
        };

        if let Err(reason) = ck_crypto::verify_command(&self.config.token, signed) {
            return refuse(reason);
        }
        if let Err(reason) = command.check(&self.config.stick_id, now) {
            return refuse(reason);
        }

        let mut seen = load_seen(&self.root, now);
        if seen.iter().any(|entry| entry.id == command.id) {
            return refuse(Refusal::Replayed);
        }
        if seen.len() >= MAX_SEEN {
            tracing::warn!(
                "too many commands in the last hour, not running {}",
                command.id
            );
            return failed();
        }
        // The id is kept before the command runs. A shutdown never returns, and a crash after the
        // run must not let the same command run again after the reboot.
        seen.push(Seen {
            id: command.id.clone(),
            expires_at: command.expires_at,
        });
        if let Err(error) = save_seen(&self.root, &seen) {
            tracing::warn!(
                "the list of run commands cannot be kept, so {} is not run: {error}",
                command.id
            );
            return failed();
        }

        // The kind is logged, never the text of a message.
        tracing::info!(command = %command.id, kind = kind_of(&command.action), "running a Hub command");
        match executor.run(&command.action) {
            Ok(()) => CommandAck {
                command_id: command.id.clone(),
                result: AckResult::Ok,
                refusal: None,
            },
            Err(error) => {
                tracing::warn!(command = %command.id, "the command failed: {error}");
                failed()
            }
        }
    }
}

fn kind_of(action: &Action) -> &'static str {
    match action {
        Action::Message { .. } => "message",
        Action::LockScreen => "lock_screen",
        Action::UnlockScreen => "unlock_screen",
        Action::Shutdown => "shutdown",
        _ => "unknown",
    }
}

/// The ids still worth remembering. An unreadable file counts as empty, because refusing every
/// command for ever would be worse for the contest than the small chance of one repeat.
fn load_seen(root: &Path, now: Timestamp) -> Vec<Seen> {
    let seen: Vec<Seen> = fs::read(root.join(SEEN_FILE))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    seen.into_iter()
        .filter(|entry| entry.expires_at > now)
        .collect()
}

fn save_seen(root: &Path, seen: &[Seen]) -> Result<(), Error> {
    let path = root.join(SEEN_FILE);
    if let Some(folder) = path.parent() {
        fs::create_dir_all(folder).map_err(|e| Error::Queue(e.to_string()))?;
    }
    write_atomically(
        &path,
        &serde_json::to_vec(seen).expect("the list always serializes"),
    )
}
