use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use chrono::{DateTime, Utc};
use ck_agent_uplink::{Error, Queue, Uplink, recorder_state};
use ck_protocol::{
    EventKind, FIRST_PREV_HASH, Heartbeat, RecorderState, RecordingMode, Token, verify_chain,
};
use serde_json::json;
use tempfile::TempDir;

const TOKEN: &str = "the-stick-secret";

struct Captured {
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Captured {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// A tiny Hub: answers each connection with the next status and keeps what it was sent.
struct FakeHub {
    base: String,
    requests: Arc<Mutex<Vec<Captured>>>,
}

fn fake_hub(statuses: Vec<u16>) -> FakeHub {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    thread::spawn(move || {
        for status in statuses {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut data = Vec::new();
            let mut buffer = [0_u8; 4096];
            let header_end = loop {
                let read = stream.read(&mut buffer).expect("read");
                data.extend_from_slice(&buffer[..read]);
                if let Some(at) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                    break at + 4;
                }
                assert!(read > 0, "the request ended early");
            };
            let head = String::from_utf8_lossy(&data[..header_end]).into_owned();
            let mut lines = head.lines();
            let path = lines
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("")
                .to_owned();
            let headers: Vec<(String, String)> = lines
                .filter_map(|line| line.split_once(':'))
                .map(|(n, v)| (n.trim().to_owned(), v.trim().to_owned()))
                .collect();
            let length: usize = headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, v)| v.parse().ok())
                .unwrap_or(0);
            while data.len() < header_end + length {
                let read = stream.read(&mut buffer).expect("read body");
                data.extend_from_slice(&buffer[..read]);
            }
            seen.lock().expect("lock").push(Captured {
                path,
                headers,
                body: data[header_end..header_end + length].to_vec(),
            });
            let reply =
                format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            stream.write_all(reply.as_bytes()).expect("reply");
        }
    });
    FakeHub { base, requests }
}

fn machine(root: &Path, url: &str) {
    let files = [
        (
            "etc/ck/config.json",
            format!(
                r#"{{"schema":1,"stick_id":"03c9f556","created_at":"2026-11-10T15:22","token":"{TOKEN}",
                    "team":" Team A ","site":"Lab 1","recording":{{"mode":"low"}},
                    "api":{{"url":"{url}","heartbeat_s":15}}}}"#
            ),
        ),
        (
            "etc/ck/image-version",
            "ck-os-base development\n".to_owned(),
        ),
        ("etc/ck/config.sha256", format!("{}\n", "ab".repeat(32))),
        ("proc/uptime", "123.45 400.00\n".to_owned()),
        ("proc/sys/kernel/hostname", "team-a-1\n".to_owned()),
        (
            "proc/sys/kernel/random/boot_id",
            "7f1d2c3e-0b4a-4c55-9a64-1d2e3f405162\n".to_owned(),
        ),
    ];
    for (path, content) in files {
        let file = root.join(path);
        fs::create_dir_all(file.parent().expect("parent")).expect("folder");
        fs::write(file, content).expect(path);
    }
}

fn time() -> DateTime<Utc> {
    "2026-11-12T10:00:00Z".parse().expect("time")
}

#[test]
fn sends_a_signed_heartbeat_built_from_the_machine_files() {
    let hub = fake_hub(vec![200, 200]);
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), &hub.base);
    let mut uplink = Uplink::load(dir.path()).expect("load");
    let url = uplink.url().expect("a server is set");
    assert!(url.ends_with("/api/v1/heartbeat"), "{url}");

    let first = uplink.next_heartbeat(time(), RecorderState::Recording);
    uplink.send(&url, &first).expect("send");
    let second = uplink.next_heartbeat(time(), RecorderState::Recording);
    uplink.send(&url, &second).expect("send again");

    let requests = hub.requests.lock().expect("lock");
    assert_eq!(requests.len(), 2);
    let request = &requests[0];
    assert_eq!(request.path, "/api/v1/heartbeat");
    assert_eq!(request.header("X-CK-Stick"), Some("03c9f556"));
    assert!(
        request
            .header("Content-Type")
            .is_some_and(|t| t.starts_with("application/json"))
    );

    // The signature is what the Hub will check with the stick's secret.
    let signature = request.header("X-CK-Signature").expect("signature");
    assert!(ck_crypto::verify(
        &Token::new(TOKEN),
        &request.body,
        signature
    ));
    assert!(!ck_crypto::verify(
        &Token::new("other"),
        &request.body,
        signature
    ));

    let sent: Heartbeat = serde_json::from_slice(&request.body).expect("a heartbeat");
    assert_eq!(sent.seq, 1);
    assert_eq!(sent.boot_id, "7f1d2c3e-0b4a-4c55-9a64-1d2e3f405162");
    assert_eq!(sent.uptime_s, 123);
    assert_eq!(sent.hostname, "team-a-1");
    assert_eq!(sent.image_version, "ck-os-base development");
    assert_eq!(sent.config_sha256, "ab".repeat(32));
    assert_eq!(
        (sent.team.as_str(), sent.site.as_str()),
        ("Team A", "Lab 1")
    );
    assert_eq!(sent.recording.mode, RecordingMode::Low);

    let again: Heartbeat = serde_json::from_slice(&requests[1].body).expect("a heartbeat");
    assert_eq!(again.seq, 2, "the sequence goes up by one");
    assert_eq!(again.boot_id, sent.boot_id);
    // The secret is only ever used to sign; it is never part of what is sent.
    for request in requests.iter() {
        assert!(!String::from_utf8_lossy(&request.body).contains(TOKEN));
    }
}

#[test]
fn a_refusal_and_an_unreachable_hub_are_different_errors() {
    let hub = fake_hub(vec![401, 409, 500]);
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), &hub.base);
    let mut uplink = Uplink::load(dir.path()).expect("load");
    let url = uplink.url().expect("url");
    for expected in [401, 409, 500] {
        let heartbeat = uplink.next_heartbeat(time(), RecorderState::Waiting);
        assert!(matches!(
            uplink.send(&url, &heartbeat),
            Err(Error::Rejected(status)) if status == expected
        ));
    }

    // Nothing listens here any more.
    let closed = TcpListener::bind("127.0.0.1:0").expect("bind");
    let dead = format!(
        "http://{}/api/v1/heartbeat",
        closed.local_addr().expect("address")
    );
    drop(closed);
    let heartbeat = uplink.next_heartbeat(time(), RecorderState::Waiting);
    assert!(matches!(
        uplink.send(&dead, &heartbeat),
        Err(Error::Unreachable(_))
    ));
}

#[test]
fn no_server_address_means_no_reporting() {
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), "");
    assert_eq!(Uplink::load(dir.path()).expect("load").url(), None);
}

#[test]
fn a_trailing_slash_in_the_server_address_is_not_doubled() {
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), "http://10.0.2.2:8088/");
    let url = Uplink::load(dir.path()).expect("load").url().expect("url");
    assert_eq!(url, "http://10.0.2.2:8088/api/v1/heartbeat");
}

#[test]
fn missing_or_broken_settings_are_explained() {
    let dir = TempDir::new().expect("temp dir");
    assert!(matches!(Uplink::load(dir.path()), Err(Error::NoSettings)));

    let file = dir.path().join("etc/ck/config.json");
    fs::create_dir_all(file.parent().expect("parent")).expect("folder");
    fs::write(&file, "{ not json").expect("write");
    assert!(matches!(
        Uplink::load(dir.path()),
        Err(Error::BadSettings(_))
    ));
}

#[test]
fn the_recorder_state_follows_the_mode_and_the_service() {
    assert_eq!(recorder_state(RecordingMode::Off, true), RecorderState::Off);
    assert_eq!(
        recorder_state(RecordingMode::Low, true),
        RecorderState::Recording
    );
    assert_eq!(
        recorder_state(RecordingMode::Standard, false),
        RecorderState::Waiting
    );
}

fn sha(bytes: &[u8]) -> String {
    ck_crypto::sha256_hex(bytes)
}

fn push(queue: &Queue, uplink: &Uplink, user: &str) -> ck_protocol::Event {
    let (stick, boot) = uplink.identity();
    queue
        .push(
            stick,
            boot,
            EventKind::Login,
            json!({ "username": user }),
            time(),
        )
        .expect("push")
}

#[test]
fn queued_events_form_a_chain_that_carries_on_after_a_restart_and_starts_again_each_boot() {
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), "http://127.0.0.1:1");
    let uplink = Uplink::load(dir.path()).expect("load");
    let queue_dir = dir.path().join("var/lib/ck/queue");

    let queue = Queue::open(&queue_dir).expect("open");
    push(&queue, &uplink, "ana");
    push(&queue, &uplink, "bob");
    // The agent restarts: a new Queue on the same folder carries on the same chain.
    let queue = Queue::open(&queue_dir).expect("reopen");
    push(&queue, &uplink, "cris");

    let events: Vec<_> = queue
        .pending()
        .expect("pending")
        .into_iter()
        .map(|(_, e)| e)
        .collect();
    assert_eq!(events.iter().map(|e| e.seq).collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(events[0].prev_hash, FIRST_PREV_HASH);
    assert_eq!(verify_chain(&events, 1, FIRST_PREV_HASH, sha), Ok(()));

    // After a reboot the boot id differs and the chain starts again at 1, behind the old events.
    let (stick, _) = uplink.identity();
    let next = queue
        .push(stick, "another-boot", EventKind::Login, json!({}), time())
        .expect("push");
    assert_eq!((next.seq, next.prev_hash.as_str()), (1, FIRST_PREV_HASH));
    assert_eq!(
        queue.pending().expect("pending").len(),
        4,
        "the old boot's events wait too"
    );
}

#[test]
fn events_are_sent_in_order_signed_and_taken_out_of_the_queue() {
    let hub = fake_hub(vec![200, 200, 200]);
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), &hub.base);
    let uplink = Uplink::load(dir.path()).expect("load");
    let queue = Queue::open(dir.path().join("queue")).expect("open");
    for user in ["ana", "bob", "cris"] {
        push(&queue, &uplink, user);
    }

    let url = uplink.events_url().expect("url");
    assert!(url.ends_with("/api/v1/events"), "{url}");
    assert_eq!(uplink.flush(&queue, &url).expect("flush"), 3);
    assert_eq!(queue.pending().expect("pending").len(), 0);

    let requests = hub.requests.lock().expect("lock");
    let users: Vec<String> = requests
        .iter()
        .map(|r| {
            let event: ck_protocol::Event = serde_json::from_slice(&r.body).expect("an event");
            event.payload["username"].as_str().expect("user").to_owned()
        })
        .collect();
    assert_eq!(users, ["ana", "bob", "cris"]);
    for request in requests.iter() {
        assert_eq!(request.path, "/api/v1/events");
        let signature = request.header("X-CK-Signature").expect("signature");
        assert!(ck_crypto::verify(
            &Token::new(TOKEN),
            &request.body,
            signature
        ));
    }
}

#[test]
fn a_failure_keeps_the_rest_in_order_and_a_later_flush_sends_them() {
    let hub = fake_hub(vec![200, 500, 200, 200]);
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), &hub.base);
    let uplink = Uplink::load(dir.path()).expect("load");
    let queue = Queue::open(dir.path().join("queue")).expect("open");
    for user in ["ana", "bob", "cris"] {
        push(&queue, &uplink, user);
    }
    let url = uplink.events_url().expect("url");

    assert_eq!(
        uplink.flush(&queue, &url).expect("first flush"),
        1,
        "stopped at the 500"
    );
    let waiting: Vec<u64> = queue
        .pending()
        .expect("pending")
        .iter()
        .map(|(_, e)| e.seq)
        .collect();
    assert_eq!(waiting, [2, 3], "nothing lost, nothing out of order");

    assert_eq!(uplink.flush(&queue, &url).expect("second flush"), 2);
    assert_eq!(queue.pending().expect("pending").len(), 0);
}

#[test]
fn an_event_the_hub_already_has_counts_as_sent_and_an_unreachable_hub_loses_nothing() {
    let hub = fake_hub(vec![409]);
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), &hub.base);
    let uplink = Uplink::load(dir.path()).expect("load");
    let queue = Queue::open(dir.path().join("queue")).expect("open");
    push(&queue, &uplink, "ana");
    let url = uplink.events_url().expect("url");
    assert_eq!(uplink.flush(&queue, &url).expect("flush"), 1);
    assert_eq!(queue.pending().expect("pending").len(), 0);

    push(&queue, &uplink, "bob");
    let closed = TcpListener::bind("127.0.0.1:0").expect("bind");
    let dead = format!(
        "http://{}/api/v1/events",
        closed.local_addr().expect("address")
    );
    drop(closed);
    assert!(matches!(
        uplink.flush(&queue, &dead),
        Err(Error::Unreachable(_))
    ));
    assert_eq!(queue.pending().expect("pending").len(), 1);
}

#[test]
fn a_damaged_file_is_set_aside_and_a_huge_payload_is_refused() {
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), "http://127.0.0.1:1");
    let uplink = Uplink::load(dir.path()).expect("load");
    let queue_dir = dir.path().join("queue");
    let queue = Queue::open(&queue_dir).expect("open");
    push(&queue, &uplink, "ana");
    fs::write(queue_dir.join("event-000000000002.json"), "{ not an event").expect("damage");
    push(&queue, &uplink, "bob");

    let events = queue.pending().expect("pending");
    assert_eq!(
        events.len(),
        2,
        "the damaged file does not block the others"
    );
    assert!(queue_dir.join("event-000000000002.bad").exists());

    let (stick, boot) = uplink.identity();
    let huge = json!({ "text": "x".repeat(20_000) });
    let refused = queue.push(stick, boot, EventKind::Login, huge, time());
    assert!(matches!(refused, Err(Error::Queue(_))), "{refused:?}");
    assert_eq!(queue.pending().expect("pending").len(), 2);
}

#[test]
fn a_restart_in_the_same_boot_carries_on_the_heartbeat_sequence_and_a_new_boot_starts_at_1() {
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), "http://127.0.0.1:1");
    let mut first = Uplink::load(dir.path()).expect("load");
    assert_eq!(
        first.next_heartbeat(time(), RecorderState::Recording).seq,
        1
    );
    assert_eq!(
        first.next_heartbeat(time(), RecorderState::Recording).seq,
        2
    );

    // The service restarts: a new Uplink in the same boot goes on from 3, so the Hub keeps
    // accepting heartbeats.
    let mut restarted = Uplink::load(dir.path()).expect("reload");
    assert_eq!(
        restarted
            .next_heartbeat(time(), RecorderState::Recording)
            .seq,
        3
    );

    // A reboot gives a new boot id, and the sequence starts again at 1.
    fs::write(
        dir.path().join("proc/sys/kernel/random/boot_id"),
        "a-different-boot
",
    )
    .expect("new boot");
    let mut rebooted = Uplink::load(dir.path()).expect("after reboot");
    assert_eq!(
        rebooted
            .next_heartbeat(time(), RecorderState::Recording)
            .seq,
        1
    );
}
