use std::fs::{self, File, FileTimes};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use ck_agent_uplink::{Error, Queue, RecordingError, Uplink, recorder_state};
use ck_protocol::{
    EventKind, FIRST_PREV_HASH, Heartbeat, RecorderState, RecordingMode, Timestamp, Token,
    verify_chain,
};
use serde_json::json;
use tempfile::TempDir;

const TOKEN: &str = "the-stick-secret";

struct Captured {
    method: String,
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
    fake_hub_replies(
        statuses
            .into_iter()
            .map(|status| (status, Vec::new()))
            .collect(),
    )
}

fn fake_hub_replies(replies: Vec<(u16, Vec<u8>)>) -> FakeHub {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    thread::spawn(move || {
        for (status, body) in replies {
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
            let mut request = lines.next().expect("request line").split_whitespace();
            let method = request.next().expect("method").to_owned();
            let path = request.next().expect("path").to_owned();
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
                method,
                path,
                headers,
                body: data[header_end..header_end + length].to_vec(),
            });
            let location = if status == 307 {
                "Location: /unexpected-upload-recipient\r\n"
            } else {
                ""
            };
            let reply = format!(
                "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n{location}\r\n",
                body.len()
            );
            stream.write_all(reply.as_bytes()).expect("reply");
            stream.write_all(&body).expect("body");
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

fn time() -> Timestamp {
    "2026-11-12T10:00:00Z".parse().expect("time")
}

#[test]
fn an_enrolment_retry_keeps_its_proof_across_a_restart() {
    let hub = fake_hub_replies(vec![
        (307, Vec::new()),
        recording_reply(200, &json!({"stick_id":"03c9f556"})),
    ]);
    let dir = TempDir::new().expect("machine files");
    machine(dir.path(), &hub.base);
    let mut config: serde_json::Value =
        serde_json::from_slice(&fs::read(dir.path().join("etc/ck/config.json")).expect("config"))
            .expect("settings");
    config["schema"] = json!(3);
    config["enrolment_code"] = json!("ABCD-EFGH-JKLM-NPQR-STUV");
    config
        .as_object_mut()
        .expect("settings object")
        .remove("token");
    fs::create_dir_all(dir.path().join("ck-config")).expect("ckconfig folder");
    fs::write(
        dir.path().join("ck-config/config.json"),
        serde_json::to_vec(&config).expect("settings JSON"),
    )
    .expect("enrolment settings");
    let root = ck_agent_config::Root::new(dir.path());
    let settings = ck_agent_config::load(&root).expect("first boot");
    ck_agent_config::apply_basics(&root, &settings).expect("apply settings");

    let first = Uplink::load(dir.path()).expect("load first boot");
    assert!(first.enrolment_pending().expect("pending state"));
    assert!(matches!(
        first.claim_enrolment(),
        Err(Error::EnrolmentRejected(307))
    ));

    let again = Uplink::load(dir.path()).expect("load after restart");
    assert!(again.enrolment_pending().expect("pending after restart"));
    again.claim_enrolment().expect("Hub confirms enrolment");
    let confirmed = Uplink::load(dir.path()).expect("load confirmed state");
    assert!(
        !confirmed
            .enrolment_pending()
            .expect("confirmed after restart")
    );
    confirmed.claim_enrolment().expect("no further claim");

    let requests = hub.requests.lock().expect("requests");
    assert_eq!(requests.len(), 2);
    assert!(
        requests
            .iter()
            .all(|request| request.path == "/api/v1/enrol/claim")
    );
    assert!(
        requests[0].body == requests[1].body,
        "the proof changed on retry"
    );
}

const RECORDING_ID: &str = "0123456789abcdef0123456789abcdef";
const RECORDINGS: &str = "var/lib/ck/recordings";
const PENDING: &str = "var/lib/ck/recordings/upload-ready/03c9f556";

fn recording_reply(status: u16, value: &serde_json::Value) -> (u16, Vec<u8>) {
    (status, serde_json::to_vec(value).expect("reply"))
}

fn recording_status(next_chunk: u32) -> (u16, Vec<u8>) {
    recording_reply(
        200,
        &json!({
            "id": RECORDING_ID, "chunk_size": 4, "next_chunk": next_chunk, "complete": false
        }),
    )
}

fn recording_settings(root: &Path, mode: &str, end: Option<&str>, retention: Option<u32>) {
    let config = root.join("etc/ck/config.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&fs::read(&config).expect("read config")).expect("config");
    value["recording"]["upload"] = json!(mode);
    value["recording"]["retention_days"] = json!(retention);
    value["contest"]["end"] = json!(end);
    fs::write(config, value.to_string()).expect("config");
}

#[test]
fn recordings_resume_with_signed_requests_after_the_hub_fails() {
    let hub = fake_hub_replies(vec![
        recording_status(0),
        recording_reply(200, &json!({"next_chunk": 1})),
        recording_reply(503, &json!({})),
        recording_status(1),
        recording_status(1),
        recording_reply(200, &json!({"next_chunk": 2})),
        recording_reply(200, &json!({"complete": true})),
    ]);
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path();
    machine(root, &hub.base);
    recording_settings(root, "during_contest", None, None);
    let pending = root.join(PENDING);
    fs::create_dir_all(&pending).expect("pending");
    let video = pending.join("screen-1.mkv");
    fs::write(&video, b"abcdefg").expect("video");
    fs::write(pending.join("screen-2.mkv.tmp"), b"still recording").expect("partial");
    assert!(
        Uplink::load(root)
            .expect("load")
            .upload_recordings()
            .is_err()
    );
    assert!(video.exists());
    assert_eq!(
        Uplink::load(root)
            .expect("reload")
            .upload_recordings()
            .expect("resume"),
        1
    );
    assert!(!video.exists());
    assert!(pending.join("screen-2.mkv.tmp").exists());
    let kept = root.join(RECORDINGS).join("uploaded/03c9f556/screen-1.mkv");
    assert_eq!(fs::read(kept).expect("local recording"), b"abcdefg");

    let requests = hub.requests.lock().expect("lock");
    assert_eq!(
        requests
            .iter()
            .map(|request| request.method.as_str())
            .collect::<Vec<_>>(),
        ["POST", "PUT", "PUT", "POST", "GET", "PUT", "POST"]
    );
    let first: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("start body");
    assert_eq!(
        first,
        json!({
            "file": "screen-1.mkv", "kind": "video", "size": 7, "sha256": ck_crypto::sha256_hex(b"abcdefg")
        })
    );
    assert_eq!(
        requests[4].path,
        format!("/api/v1/recordings/{RECORDING_ID}")
    );
    assert_eq!(requests[5].body, b"efg");

    let chunk0 = ck_crypto::sha256_hex(b"abcd");
    let chunk1 = ck_crypto::sha256_hex(b"efg");
    let signed = [
        String::from_utf8(requests[0].body.clone()).expect("start body"),
        format!("ck-recording-chunk\n{RECORDING_ID}\n0\n{chunk0}"),
        format!("ck-recording-chunk\n{RECORDING_ID}\n1\n{chunk1}"),
        String::from_utf8(requests[3].body.clone()).expect("repeated start body"),
        format!("ck-recording-status\n{RECORDING_ID}"),
        format!("ck-recording-chunk\n{RECORDING_ID}\n1\n{chunk1}"),
        format!("ck-recording-complete\n{RECORDING_ID}"),
    ];
    for (request, message) in requests.iter().zip(signed) {
        assert_eq!(request.header("x-ck-stick"), Some("03c9f556"));
        assert_eq!(
            request.header("x-ck-signature"),
            Some(ck_crypto::sign(&Token::new(TOKEN), message.as_bytes()).as_str())
        );
    }
    assert_eq!(
        requests[5].header("x-ck-chunk-sha256"),
        Some(chunk1.as_str())
    );
}

#[test]
fn a_recording_whose_name_was_uploaded_before_is_kept_under_a_new_name() {
    let hub = fake_hub_replies(vec![recording_reply(
        200,
        &json!({"id": RECORDING_ID, "chunk_size": 4, "next_chunk": 2, "complete": true}),
    )]);
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path();
    machine(root, &hub.base);
    recording_settings(root, "during_contest", None, None);
    let pending = root.join(PENDING);
    let uploaded = root.join(RECORDINGS).join("uploaded/03c9f556");
    fs::create_dir_all(&pending).expect("pending");
    fs::create_dir_all(&uploaded).expect("uploaded");
    fs::write(pending.join("screen-1.mkv"), b"abcdefg").expect("new");
    fs::write(uploaded.join("screen-1.mkv"), b"older").expect("old");
    assert_eq!(
        Uplink::load(root)
            .expect("load")
            .upload_recordings()
            .expect("upload"),
        1
    );
    assert_eq!(
        fs::read(uploaded.join("screen-1.mkv")).expect("old"),
        b"older"
    );
    assert_eq!(
        fs::read(uploaded.join("screen-1-1.mkv")).expect("new"),
        b"abcdefg"
    );
    assert!(!pending.join("screen-1.mkv").exists());
}

#[test]
fn recordings_respect_opt_in_and_clean_only_expired_closed_files() {
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path();
    machine(root, "http://127.0.0.1:1");
    let pending = root.join(PENDING);
    fs::create_dir_all(&pending).expect("pending");
    let old_file = pending.join("old.jpg");
    let active = pending.join("capture.jpg.tmp");
    let recent = pending.join("recent.mkv");
    let local_only = root.join(RECORDINGS).join("without-upload.mkv");
    let old_time = SystemTime::now() - Duration::from_secs(3 * 86_400);
    for path in [&old_file, &active, &local_only] {
        let file = File::create(path).expect("file");
        file.set_times(FileTimes::new().set_modified(old_time))
            .expect("mtime");
    }
    fs::write(&recent, b"recent").expect("recent");
    assert_eq!(
        Uplink::load(root)
            .expect("load")
            .upload_recordings()
            .expect("default keep"),
        0
    );
    assert!(old_file.exists() && local_only.exists());
    recording_settings(root, "after_contest", Some("2099-12-31T12:00"), Some(2));
    assert_eq!(
        Uplink::load(root)
            .expect("load")
            .upload_recordings()
            .expect("before end"),
        0
    );
    assert!(!old_file.exists() && !local_only.exists());
    assert!(active.exists() && recent.exists());
    recording_settings(root, "after_contest", None, None);
    assert_eq!(
        Uplink::load(root)
            .expect("load")
            .upload_recordings()
            .expect("end required"),
        0
    );
}

#[test]
fn recordings_can_start_after_the_contest_and_never_follow_a_redirect() {
    let hub = fake_hub_replies(vec![recording_reply(307, &json!({}))]);
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path();
    machine(root, &hub.base);
    recording_settings(root, "after_contest", Some("2001-01-01T00:00"), None);
    let pending = root.join(PENDING);
    fs::create_dir_all(&pending).expect("pending");
    let clip = pending.join("snapshot-1.jpg");
    fs::write(&clip, b"abc").expect("clip");
    let result = Uplink::load(root).expect("load").upload_recordings();
    assert!(matches!(result, Err(RecordingError::Rejected(307))));
    assert!(clip.exists());
    let requests = hub.requests.lock().expect("lock");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/api/v1/recordings");
    let start: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("start");
    assert_eq!(start["kind"], "snapshot");
}

#[test]
fn the_heartbeat_says_what_the_stick_made_of_its_settings_file() {
    use ck_protocol::ConfigTrust;

    let hub = fake_hub(vec![]);
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), &hub.base);
    let mut uplink = Uplink::load(dir.path()).expect("load");
    // A stick whose agent never kept it (an older one) says nothing.
    assert_eq!(
        uplink
            .next_heartbeat(time(), RecorderState::Recording)
            .config_trust,
        None
    );
    let marker = dir.path().join("etc/ck/config.trust");
    for (text, expected) in [
        (
            "verified
",
            Some(ConfigTrust::Verified),
        ),
        (
            "rejected 0123abcd
",
            Some(ConfigTrust::Rejected),
        ),
        (
            "unchecked
",
            Some(ConfigTrust::Unchecked),
        ),
        (
            "something else
",
            None,
        ),
        ("", None),
    ] {
        fs::write(&marker, text).expect("marker");
        let sent = uplink.next_heartbeat(time(), RecorderState::Recording);
        assert_eq!(sent.config_trust, expected, "{text:?}");
    }
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
fn advertises_unlock_only_when_the_keeper_screen_service_is_installed() {
    let dir = TempDir::new().expect("temp dir");
    let root = dir.path();
    machine(root, "http://127.0.0.1:9");
    let mut uplink = Uplink::load(root).expect("load");
    let helper = root.join("usr/local/sbin/ck-screen");
    let unit = root.join("etc/systemd/system/ck-screen@.service");

    assert!(
        !uplink
            .next_heartbeat(time(), RecorderState::Off)
            .screen_unlock
    );

    fs::create_dir_all(helper.parent().expect("helper parent")).expect("helper directory");
    fs::write(helper, "").expect("helper");
    assert!(
        !uplink
            .next_heartbeat(time(), RecorderState::Off)
            .screen_unlock
    );

    fs::create_dir_all(unit.parent().expect("unit parent")).expect("unit directory");
    fs::write(unit, "").expect("unit");
    assert!(
        uplink
            .next_heartbeat(time(), RecorderState::Off)
            .screen_unlock
    );
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
fn a_trailing_slash_in_the_server_address_is_not_doubled() {
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), "https://hub.example.org/");
    let url = Uplink::load(dir.path()).expect("load").url().expect("url");
    assert_eq!(url, "https://hub.example.org/api/v1/heartbeat");
}

#[test]
fn plain_http_to_another_machine_is_not_used() {
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), "http://10.0.2.2:8088/");
    let uplink = Uplink::load(dir.path()).expect("load");
    assert!(uplink.url().is_none());
    assert!(uplink.events_url().is_none());
}

#[test]
fn refused_files_are_capped_and_the_newest_are_kept() {
    let hub = fake_hub(vec![]);
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), &hub.base);
    let uplink = Uplink::load(dir.path()).expect("load");
    let folder = dir.path().join("queue");
    let queue = Queue::open(&folder).expect("open");
    for n in 2..250 {
        fs::write(folder.join(format!("event-{n:012}.refused")), "{}").expect("old file");
    }
    let event = push(&queue, &uplink, "ana");
    assert_eq!(event.seq, 1);
    let (counter, _) = queue.pending().expect("pending").remove(0);
    queue.set_aside(counter).expect("set aside");
    // The cap is applied when the queue is read next.
    queue.pending().expect("pending");
    let refused = fs::read_dir(&folder)
        .expect("read")
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "refused"))
        .count();
    assert_eq!(refused, 200);
    assert!(folder.join("event-000000000249.refused").exists());
    assert!(!folder.join("event-000000000002.refused").exists());
}

#[test]
fn an_event_the_hub_took_stays_on_the_stick_as_it_was_sent_and_old_copies_are_capped() {
    let hub = fake_hub(vec![200]);
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), &hub.base);
    let uplink = Uplink::load(dir.path()).expect("load");
    let folder = dir.path().join("queue");
    let queue = Queue::open(&folder).expect("open");
    push(&queue, &uplink, "ana");
    let (counter, _) = queue.pending().expect("pending").remove(0);
    let before = fs::read(folder.join(format!("event-{counter:012}.json"))).expect("file");

    let url = uplink.events_url().expect("url");
    assert_eq!(uplink.flush(&queue, &url).expect("flush"), 1);
    assert_eq!(queue.pending().expect("pending").len(), 0);
    let copy = folder.join(format!("event-{counter:012}.sent"));
    assert_eq!(fs::read(&copy).expect("the local copy"), before);

    // Far more copies than the cap: the oldest go, the newest stay.
    for n in 1_000_000..1_020_001_u64 {
        fs::write(folder.join(format!("event-{n:012}.sent")), "{}").expect("old copy");
    }
    queue.pending().expect("pending");
    let kept = fs::read_dir(&folder)
        .expect("read")
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "sent"))
        .count();
    assert_eq!(kept, 20_000);
    assert!(!copy.exists());
    assert!(folder.join("event-000001020000.sent").exists());
}

#[test]
fn the_recorder_state_follows_the_mode_and_the_service() {
    let dir = tempfile::tempdir().expect("dir");
    let root = dir.path();
    let state = root.join("run/ck-recorder/state");
    fs::create_dir_all(state.parent().expect("parent")).expect("folder");

    assert_eq!(
        recorder_state(root, RecordingMode::Off, true),
        RecorderState::Off
    );
    assert_eq!(
        recorder_state(root, RecordingMode::Standard, false),
        RecorderState::Waiting
    );
    // An active service that says nothing, or says it waits, is not recording.
    for (written, expected) in [
        (None, RecorderState::Waiting),
        (Some("waiting"), RecorderState::Waiting),
        (Some("recording"), RecorderState::Recording),
        (Some("lowdisk"), RecorderState::LowDisk),
    ] {
        match written {
            Some(text) => fs::write(&state, format!("{text}\n")).expect("write"),
            None => drop(fs::remove_file(&state)),
        }
        assert_eq!(recorder_state(root, RecordingMode::Low, true), expected);
    }
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
fn writers_adding_events_at_the_same_time_lose_none() {
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), "http://127.0.0.1:1");
    let uplink = Uplink::load(dir.path()).expect("load");
    let queue_dir = dir.path().join("queue");
    Queue::open(&queue_dir).expect("open");
    let (stick, boot) = uplink.identity();
    let (stick, boot) = (stick.clone(), boot.to_owned());

    // Capture, the uplink and `enqueue` are separate processes, each with its own `Queue`.
    let writers: Vec<_> = (0..8)
        .map(|writer| {
            let (stick, boot, queue_dir) = (stick.clone(), boot.clone(), queue_dir.clone());
            thread::spawn(move || {
                let queue = Queue::open(queue_dir).expect("open");
                for n in 0..25 {
                    queue
                        .push(
                            &stick,
                            &boot,
                            EventKind::Login,
                            json!({ "writer": writer, "n": n }),
                            time(),
                        )
                        .expect("push");
                }
            })
        })
        .collect();
    for writer in writers {
        writer.join().expect("writer");
    }

    let waiting = Queue::open(&queue_dir)
        .expect("open")
        .pending()
        .expect("pending");
    let mut seqs: Vec<u64> = waiting.iter().map(|(_, event)| event.seq).collect();
    seqs.sort_unstable();
    assert_eq!(
        seqs,
        (1..=200).collect::<Vec<u64>>(),
        "no event lost or doubled"
    );
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
fn an_answer_that_redirects_is_not_the_hubs_and_the_event_stays_queued() {
    // A venue's login page answers with a redirect; the second answer would be a 200.
    let hub = fake_hub(vec![307, 200]);
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), &hub.base);
    let uplink = Uplink::load(dir.path()).expect("load");
    let queue = Queue::open(dir.path().join("queue")).expect("open");
    push(&queue, &uplink, "ana");
    let url = uplink.events_url().expect("url");

    assert!(uplink.flush(&queue, &url).is_err());
    assert_eq!(queue.pending().expect("pending").len(), 1, "not lost");
    assert_eq!(
        hub.requests.lock().expect("lock").len(),
        1,
        "the redirect was not followed"
    );
}

#[test]
fn one_flush_sends_a_bounded_number_of_events() {
    let hub = fake_hub(vec![200; 101]);
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), &hub.base);
    let uplink = Uplink::load(dir.path()).expect("load");
    let queue = Queue::open(dir.path().join("queue")).expect("open");
    for n in 0..101 {
        push(&queue, &uplink, &format!("user{n}"));
    }
    let url = uplink.events_url().expect("url");
    assert_eq!(uplink.flush(&queue, &url).expect("flush"), 100);
    assert_eq!(queue.pending().expect("pending").len(), 1, "the rest waits");
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
    fs::write(queue_dir.join("event-000000000009.json"), "{ not an event").expect("damage");
    push(&queue, &uplink, "bob");

    let events = queue.pending().expect("pending");
    assert_eq!(
        events.len(),
        2,
        "the damaged file does not block the others"
    );
    assert!(queue_dir.join("event-000000000009.bad").exists());

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

#[test]
fn an_event_the_hub_refuses_for_good_is_set_aside_and_the_next_one_is_sent() {
    let hub = fake_hub(vec![400, 200]);
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), &hub.base);
    let uplink = Uplink::load(dir.path()).expect("load");
    let queue = Queue::open(dir.path().join("queue")).expect("open");
    push(&queue, &uplink, "ana");
    push(&queue, &uplink, "bob");
    let url = uplink.events_url().expect("url");

    assert_eq!(uplink.flush(&queue, &url).expect("flush"), 1);
    assert_eq!(queue.pending().expect("pending").len(), 0);
    let refused = fs::read_dir(dir.path().join("queue"))
        .expect("queue")
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "refused"))
        .count();
    assert_eq!(refused, 1, "kept for the organizer, out of the way");
}

#[test]
fn a_crash_between_the_state_and_the_event_leaves_a_gap_not_a_reused_number() {
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), "http://127.0.0.1:1");
    let uplink = Uplink::load(dir.path()).expect("load");
    let queue_dir = dir.path().join("queue");
    let queue = Queue::open(&queue_dir).expect("open");
    push(&queue, &uplink, "ana");
    // The crash: the state moved on to event 2 but its file was never written.
    let state = fs::read_to_string(queue_dir.join("state.json")).expect("state");
    let moved = state.replace("\"counter\":1", "\"counter\":2");
    assert_ne!(state, moved, "the state names the counter");
    fs::write(queue_dir.join("state.json"), moved).expect("write");

    let next = push(&queue, &uplink, "bob");
    let files: Vec<u64> = queue
        .pending()
        .expect("pending")
        .iter()
        .map(|(counter, _)| *counter)
        .collect();
    assert_eq!(files, [1, 3], "the first event is not overwritten");
    assert_eq!(next.seq, 2);
}

#[test]
fn heartbeats_events_and_recordings_refuse_plain_http_to_another_machine() {
    let dir = TempDir::new().expect("temp dir");
    machine(dir.path(), "http://10.0.2.2:8088/");
    recording_settings(dir.path(), "during_contest", None, None);
    let pending = dir.path().join(PENDING);
    fs::create_dir_all(&pending).expect("pending");
    fs::write(pending.join("screen-1.mkv"), b"abcdefg").expect("video");
    let mut uplink = Uplink::load(dir.path()).expect("load");
    let heartbeat = uplink.next_heartbeat(time(), RecorderState::Waiting);
    assert!(matches!(
        uplink.send("http://10.0.2.2:8088/api/v1/heartbeat", &heartbeat),
        Err(Error::InsecureAddress)
    ));
    let queue = Queue::open(dir.path().join("queue")).expect("open");
    push(&queue, &uplink, "ana");
    assert!(matches!(
        uplink.flush(&queue, "http://10.0.2.2:8088/api/v1/events"),
        Err(Error::InsecureAddress)
    ));
    assert_eq!(queue.pending().expect("pending").len(), 1);
    assert_eq!(uplink.upload_recordings().expect("no upload"), 0);
    assert!(pending.join("screen-1.mkv").exists());
}
