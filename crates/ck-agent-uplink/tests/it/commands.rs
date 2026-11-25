use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::Mutex;
use std::thread;

use ck_agent_uplink::{Executor, Queue, Uplink};
use ck_protocol::{
    AckResult, Action, Command, CommandAck, EventKind, HeartbeatReply, RecorderState, Refusal,
    SignedCommand, StickId, Timestamp, Token,
};
use tempfile::TempDir;

const TOKEN: &str = "the-stick-secret";

fn at(text: &str) -> Timestamp {
    text.parse().expect("time")
}

/// The stick's clock in every test.
fn now() -> Timestamp {
    at("2026-11-18T10:01:00Z")
}

fn machine(root: &Path, url: &str) {
    let files = [
        (
            "etc/ck/config.json",
            format!(
                r#"{{"schema":2,"stick_id":"03c9f556","created_at":"2026-11-10T15:22","token":"{TOKEN}",
                    "team":"Team A","site":"Lab 1","recording":{{"mode":"low"}},
                    "api":{{"url":"{url}","heartbeat_s":15}}}}"#
            ),
        ),
        ("proc/sys/kernel/random/boot_id", "boot-1\n".to_owned()),
    ];
    for (path, content) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().expect("parent")).expect("folder");
        fs::write(file, content).expect(path);
    }
}

/// Remembers what it was asked to do and can be told to fail.
#[derive(Default)]
struct FakeExecutor {
    ran: Mutex<Vec<Action>>,
    fail: bool,
}

impl Executor for FakeExecutor {
    fn run(&self, action: &Action) -> Result<(), String> {
        self.ran.lock().expect("lock").push(action.clone());
        if self.fail {
            Err("it did not work".to_owned())
        } else {
            Ok(())
        }
    }
}

impl FakeExecutor {
    fn actions(&self) -> Vec<Action> {
        self.ran.lock().expect("lock").clone()
    }
}

fn command(id: &str, action: Action) -> Command {
    Command {
        id: id.to_owned(),
        stick_id: StickId::parse("03c9f556").expect("id"),
        issued_at: at("2026-11-18T10:00:00Z"),
        expires_at: at("2026-11-18T10:05:00Z"),
        action,
    }
}

fn signed(command: Command) -> SignedCommand {
    ck_crypto::sign_command(&Token::new(TOKEN), command)
}

/// The acknowledgements waiting in the queue, oldest first.
fn acks(queue: &Queue) -> Vec<CommandAck> {
    queue
        .pending()
        .expect("pending")
        .into_iter()
        .filter(|(_, event)| event.kind == EventKind::CommandAck)
        .map(|(_, event)| serde_json::from_value(event.payload).expect("an ack"))
        .collect()
}

struct Stick {
    dir: TempDir,
    queue: Queue,
}

fn stick() -> Stick {
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), "http://127.0.0.1:9");
    let queue = Queue::open(dir.path().join("var/lib/ck/queue")).expect("queue");
    Stick { dir, queue }
}

impl Stick {
    fn uplink(&self) -> Uplink {
        Uplink::load(self.dir.path()).expect("load")
    }
}

#[test]
fn a_valid_command_runs_once_and_is_acknowledged() {
    let stick = stick();
    let executor = FakeExecutor::default();
    let lock = signed(command("c-1", Action::LockScreen));

    let ran = stick
        .uplink()
        .process_commands(&stick.queue, &[lock], now(), &executor);
    assert_eq!(ran, 1);
    assert_eq!(executor.actions(), [Action::LockScreen]);
    assert_eq!(
        acks(&stick.queue),
        [CommandAck {
            command_id: "c-1".to_owned(),
            result: AckResult::Ok,
            refusal: None
        }]
    );
}

#[test]
fn a_replayed_command_is_refused_even_after_a_reboot() {
    let stick = stick();
    let executor = FakeExecutor::default();
    let message = signed(command(
        "c-2",
        Action::Message {
            text: "Contest ends in 10 minutes".to_owned(),
        },
    ));

    stick.uplink().process_commands(
        &stick.queue,
        std::slice::from_ref(&message),
        now(),
        &executor,
    );
    // A fresh Uplink stands for the agent after a reboot: the list of run ids is on disk.
    let ran = stick
        .uplink()
        .process_commands(&stick.queue, &[message], now(), &executor);

    assert_eq!(ran, 0);
    assert_eq!(executor.actions().len(), 1, "it ran only the first time");
    let acks = acks(&stick.queue);
    assert_eq!(acks[1].result, AckResult::Refused);
    assert_eq!(acks[1].refusal, Some(Refusal::Replayed));
}

#[test]
fn refuses_what_is_expired_forged_changed_or_for_another_stick() {
    let stick = stick();
    let executor = FakeExecutor::default();

    let mut expired = command("c-3", Action::Shutdown);
    expired.expires_at = at("2026-11-18T10:00:30Z");
    let forged = ck_crypto::sign_command(
        &Token::new("not-the-secret"),
        command("c-4", Action::Shutdown),
    );
    let mut changed = signed(command("c-5", Action::LockScreen));
    changed.command.action = Action::Shutdown;
    let mut elsewhere = command("c-6", Action::Shutdown);
    elsewhere.stick_id = StickId::parse("another").expect("id");

    let commands = [signed(expired), forged, changed, signed(elsewhere)];
    let ran = stick
        .uplink()
        .process_commands(&stick.queue, &commands, now(), &executor);

    assert_eq!(ran, 0);
    assert_eq!(executor.actions(), [], "nothing ran");
    let reasons: Vec<Option<Refusal>> = acks(&stick.queue).iter().map(|a| a.refusal).collect();
    assert_eq!(
        reasons,
        [
            Some(Refusal::Expired),
            Some(Refusal::BadSignature),
            Some(Refusal::BadSignature),
            Some(Refusal::WrongStick)
        ]
    );
}

#[test]
fn a_command_that_fails_is_acknowledged_as_failed_and_not_run_again() {
    let stick = stick();
    let executor = FakeExecutor {
        fail: true,
        ..FakeExecutor::default()
    };
    let lock = signed(command("c-7", Action::LockScreen));

    let uplink = stick.uplink();
    uplink.process_commands(&stick.queue, std::slice::from_ref(&lock), now(), &executor);
    uplink.process_commands(&stick.queue, &[lock], now(), &executor);

    assert_eq!(executor.actions().len(), 1);
    let acks = acks(&stick.queue);
    assert_eq!(acks[0].result, AckResult::Failed);
    assert_eq!(acks[1].refusal, Some(Refusal::Replayed));
}

#[test]
fn the_ack_never_holds_the_text_of_a_message() {
    let stick = stick();
    let executor = FakeExecutor::default();
    let message = signed(command(
        "c-8",
        Action::Message {
            text: "secret announcement".to_owned(),
        },
    ));
    stick
        .uplink()
        .process_commands(&stick.queue, &[message], now(), &executor);

    for (_, event) in stick.queue.pending().expect("pending") {
        assert!(!event.payload.to_string().contains("announcement"));
    }
}

/// A Hub that answers one request with `body`.
fn hub_answering(body: &str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    let body = body.to_owned();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buffer = [0_u8; 8192];
        let mut data = Vec::new();
        let header_end = loop {
            let read = stream.read(&mut buffer).expect("read");
            data.extend_from_slice(&buffer[..read]);
            if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
            assert!(read > 0, "the request ended early");
        };
        // Read the whole request, or closing the connection with unread data resets it.
        let head = String::from_utf8_lossy(&data[..header_end]).to_lowercase();
        let length: usize = head
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(0);
        while data.len() < header_end + length {
            let read = stream.read(&mut buffer).expect("read body");
            data.extend_from_slice(&buffer[..read]);
        }
        let reply = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(reply.as_bytes()).expect("reply");
    });
    base
}

fn heartbeat_to(url: &str, dir: &TempDir) -> HeartbeatReply {
    machine(dir.path(), url);
    let mut uplink = Uplink::load(dir.path()).expect("load");
    let heartbeat = uplink.next_heartbeat(now(), RecorderState::Off);
    let target = uplink.url().expect("url");
    uplink.send_heartbeat(&target, &heartbeat).expect("sent")
}

#[test]
fn the_heartbeat_reply_carries_the_commands() {
    let sent = signed(command("c-9", Action::Shutdown));
    let body = serde_json::to_string(&HeartbeatReply {
        commands: vec![sent.clone()],
    })
    .expect("json");
    let dir = TempDir::new().expect("temp dir");
    let reply = heartbeat_to(&hub_answering(&body), &dir);
    assert_eq!(reply.commands, [sent]);
}

#[test]
fn an_empty_or_unknown_answer_means_no_commands() {
    for body in ["", "{}", "OK", r#"{"commands":[],"something":"new"}"#] {
        let dir = TempDir::new().expect("temp dir");
        let reply = heartbeat_to(&hub_answering(body), &dir);
        assert_eq!(reply.commands, [], "answer {body:?}");
    }
}

#[test]
fn only_a_few_commands_of_one_reply_run() {
    let stick = stick();
    let executor = FakeExecutor::default();
    let many: Vec<_> = (0..25)
        .map(|n| signed(command(&format!("many-{n}"), Action::LockScreen)))
        .collect();
    let ran = stick
        .uplink()
        .process_commands(&stick.queue, &many, now(), &executor);
    assert_eq!(ran, 10);
}
