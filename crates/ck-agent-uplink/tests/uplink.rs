use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use chrono::{DateTime, Utc};
use ck_agent_uplink::{Error, Uplink, recorder_state};
use ck_protocol::{Heartbeat, RecorderState, RecordingMode, Token};
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
    assert!(matches!(
        Uplink::load(dir.path()),
        Err(Error::NoSettings(_))
    ));

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
