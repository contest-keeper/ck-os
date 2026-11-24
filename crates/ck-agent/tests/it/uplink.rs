use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::{Command, Output};
use std::sync::mpsc;
use std::thread;

use tempfile::TempDir;

fn stick(root: &Path, url: &str) {
    let file = root.join("etc/ck/config.json");
    fs::create_dir_all(file.parent().expect("parent")).expect("folder");
    fs::write(
        file,
        format!(
            r#"{{"schema":1,"stick_id":"03c9f556","created_at":"2026-11-10T15:22","token":"the-secret",
                "team":"Team A","site":"Lab 1","api":{{"url":"{url}","heartbeat_s":15}}}}"#
        ),
    )
    .expect("settings");
}

fn uplink(root: &Path, proxy: Option<&str>) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ck-agent"));
    command.args(["uplink", "--once", "--root"]).arg(root);
    if let Some(proxy) = proxy {
        for variable in ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
            command.env(variable, proxy);
        }
    }
    command.output().expect("run ck-agent")
}

/// Answers one request with `status` and hands back what arrived.
fn one_shot_hub(status: u16) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let url = format!("http://{}", listener.local_addr().expect("address"));
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut data = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let read = stream.read(&mut buffer).expect("read");
            data.extend_from_slice(&buffer[..read]);
            if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&data[..end]).to_lowercase();
                let length: usize = head
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                if data.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let reply =
            format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        stream.write_all(reply.as_bytes()).expect("reply");
        sender
            .send(String::from_utf8_lossy(&data).into_owned())
            .expect("send");
    });
    (url, receiver)
}

#[test]
fn sends_one_signed_heartbeat_and_ignores_the_proxy_settings() {
    let (url, received) = one_shot_hub(200);
    let dir = TempDir::new().expect("temp dir");
    stick(dir.path(), &url);

    // A proxy that nothing listens on: the heartbeat must still arrive, because it goes direct.
    let result = uplink(dir.path(), Some("http://127.0.0.1:9"));
    assert_eq!(
        result.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );

    let request = received.recv().expect("a request reached the Hub");
    assert!(request.starts_with("POST /api/v1/heartbeat "), "{request}");
    assert!(request.to_lowercase().contains("x-ck-stick: 03c9f556"));
    assert!(request.to_lowercase().contains("x-ck-signature: "));
    assert!(
        !request.contains("the-secret"),
        "the secret is only used to sign"
    );
    let log = String::from_utf8_lossy(&result.stderr);
    assert!(log.contains("first heartbeat was accepted"), "{log}");
    assert!(!log.contains("the-secret"));
}

#[test]
fn a_refusing_hub_is_a_failure_and_a_stick_with_no_server_or_settings_stops_quietly() {
    let (url, _received) = one_shot_hub(401);
    let dir = TempDir::new().expect("temp dir");
    stick(dir.path(), &url);
    assert_eq!(uplink(dir.path(), None).status.code(), Some(1));

    let no_server = TempDir::new().expect("temp dir");
    stick(no_server.path(), "");
    let result = uplink(no_server.path(), None);
    assert_eq!(result.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&result.stderr).contains("no server"));

    let unconfigured = TempDir::new().expect("temp dir");
    assert_eq!(uplink(unconfigured.path(), None).status.code(), Some(0));

    let broken = TempDir::new().expect("temp dir");
    let file = broken.path().join("etc/ck/config.json");
    fs::create_dir_all(file.parent().expect("parent")).expect("folder");
    fs::write(file, "{ not json").expect("write");
    assert_eq!(uplink(broken.path(), None).status.code(), Some(1));
}
