//! The ICAP server: takes connections from Squid, hands each message to the rule, answers
//! "unchanged". It listens on the machine itself only.

use std::io::{BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::icap::{self, IcapRequest};
use crate::{
    Error, EventSink, Output, Request, Response, Rule, parse_request_head, parse_response_head,
};

const MAX_CONNECTIONS: usize = 64;
/// Squid keeps connections open between messages, so this only ends a connection that went silent.
const IDLE_TIMEOUT: Duration = Duration::from_secs(60);

struct Shared {
    rule: Mutex<Box<dyn Rule>>,
    sink: Mutex<Box<dyn Write + Send>>,
    events: Box<dyn EventSink>,
    connections: AtomicUsize,
}

/// Serves until the process is stopped. Each note the rule makes is written to `sink` after
/// `prefix`, so it can be told apart on a port that carries other text. Each event goes to
/// `events`.
pub fn serve(
    listener: &TcpListener,
    rule: Box<dyn Rule>,
    sink: Box<dyn Write + Send>,
    events: Box<dyn EventSink>,
    prefix: &'static str,
) -> Result<(), Error> {
    let shared = Arc::new(Shared {
        rule: Mutex::new(rule),
        sink: Mutex::new(sink),
        events,
        connections: AtomicUsize::new(0),
    });
    for stream in listener.incoming() {
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) => {
                tracing::warn!("a connection failed: {error}");
                continue;
            }
        };
        if shared.connections.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            shared.connections.fetch_sub(1, Ordering::SeqCst);
            continue;
        }
        let shared = Arc::clone(&shared);
        std::thread::spawn(move || {
            if let Err(error) = handle(stream, &shared, prefix) {
                tracing::debug!("connection ended: {error}");
                // Only an observation machine has anywhere to write this.
                let note = serde_json::json!({"event": "icap_error", "error": error.to_string()});
                write_note(&shared, prefix, &note.to_string());
            }
            shared.connections.fetch_sub(1, Ordering::SeqCst);
        });
    }
    Ok(())
}

fn handle(stream: TcpStream, shared: &Shared, prefix: &str) -> Result<(), Error> {
    let timeout = Some(IDLE_TIMEOUT);
    let io = |source| Error::Io {
        context: "setting up a connection",
        source,
    };
    stream.set_read_timeout(timeout).map_err(io)?;
    stream.set_write_timeout(timeout).map_err(io)?;
    let mut writer = stream.try_clone().map_err(io)?;
    let mut reader = BufReader::new(stream);
    loop {
        let mut trace = |what: &str| {
            let note = serde_json::json!({"event": "icap_read", "what": what});
            write_note(shared, prefix, &note.to_string());
        };
        let request = match icap::read_request(&mut reader, &mut trace) {
            Ok(Some(request)) => request,
            Ok(None) => return Ok(()),
            Err(error) => {
                // The proxy's own `bypass` setting lets the traffic go on without us.
                let _ = icap::write_error(&mut writer);
                return Err(error);
            }
        };
        let note = serde_json::json!({
            "event": "icap",
            "method": request.method,
            "preview": request.preview,
            "allow_204": request.allow_204,
            "body_bytes": request.body.len(),
            "body_cut": request.body_truncated,
        });
        write_note(shared, prefix, &note.to_string());
        let outputs = match request.method.as_str() {
            "OPTIONS" => {
                icap::write_options(&mut writer, &request.uri)?;
                continue;
            }
            "REQMOD" | "RESPMOD" => outputs_for(&request, shared),
            _ => {
                icap::write_error(&mut writer)?;
                continue;
            }
        };
        handle_outputs(shared, prefix, outputs);
        icap::write_unchanged(&mut writer, &request)?;
    }
}

fn outputs_for(message: &IcapRequest, shared: &Shared) -> Vec<Output> {
    let Ok(mut rule) = shared.rule.lock() else {
        return Vec::new();
    };
    let Some(request_head) = parse_request_head(&message.req_head) else {
        return Vec::new();
    };
    if message.method == "REQMOD" {
        let request = Request {
            head: request_head,
            body: message.body.clone(),
            body_truncated: message.body_truncated,
        };
        return rule.on_request(&request);
    }
    let Some(head) = parse_response_head(&message.res_head) else {
        return Vec::new();
    };
    let response = Response {
        status: head.status,
        head,
        body: message.body.clone(),
        body_truncated: message.body_truncated,
    };
    rule.on_response(&request_head, &response)
}

fn handle_outputs(shared: &Shared, prefix: &str, outputs: Vec<Output>) {
    for output in outputs {
        match output {
            Output::Note(line) => write_note(shared, prefix, &line),
            Output::Event { kind, payload } => {
                if let Err(error) = shared.events.send(kind, payload) {
                    tracing::warn!("an event could not be queued: {error}");
                }
            }
        }
    }
}

fn write_note(shared: &Shared, prefix: &str, line: &str) {
    let Ok(mut sink) = shared.sink.lock() else {
        return;
    };
    if let Err(error) = writeln!(sink, "{prefix}{line}").and_then(|()| sink.flush()) {
        tracing::warn!("cannot write what was observed: {error}");
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    use super::*;
    use crate::{EventKind, EventSink, NoEvents, Observer, Output, Request, Response, Rule};

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn read_reply(stream: &mut TcpStream) -> String {
        let mut reply = Vec::new();
        let mut byte = [0u8; 1];
        while !reply.ends_with(b"\r\n\r\n") {
            stream.read_exact(&mut byte).unwrap();
            reply.push(byte[0]);
        }
        String::from_utf8(reply).unwrap()
    }

    #[test]
    fn answers_options_and_says_unchanged_for_a_login_while_noting_its_shape() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let seen = Capture::default();
        let sink = seen.clone();
        std::thread::spawn(move || {
            serve(
                &listener,
                Box::new(Observer),
                Box::new(sink),
                Box::new(NoEvents),
                "CKCAP ",
            )
        });

        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .write_all(
                b"OPTIONS icap://127.0.0.1/reqmod ICAP/1.0\r\nEncapsulated: null-body=0\r\n\r\n",
            )
            .unwrap();
        let options = read_reply(&mut stream);
        assert!(options.starts_with("ICAP/1.0 200 OK"));
        assert!(options.contains("Methods: REQMOD"));

        let head = "POST https://codeforces.com/enter HTTP/1.1\r\n\
                    Content-Type: application/x-www-form-urlencoded\r\n\r\n";
        let body = "password=hunter2&handleOrEmail=ana";
        let message = format!(
            "REQMOD icap://127.0.0.1/reqmod ICAP/1.0\r\nAllow: 204\r\nPreview: 4096\r\n\
             Encapsulated: req-hdr=0, req-body={}\r\n\r\n{head}{:x}\r\n{body}\r\n0; ieof\r\n\r\n",
            head.len(),
            body.len()
        );
        stream.write_all(message.as_bytes()).unwrap();
        assert!(read_reply(&mut stream).starts_with("ICAP/1.0 204"));

        // The same connection carries the next message.
        stream.write_all(message.as_bytes()).unwrap();
        assert!(read_reply(&mut stream).starts_with("ICAP/1.0 204"));

        let written = String::from_utf8(seen.0.lock().unwrap().clone()).unwrap();
        // Each message also leaves a line about the ICAP exchange itself.
        assert_eq!(written.matches(r#""event":"request""#).count(), 2);
        assert_eq!(written.matches(r#""event":"icap""#).count(), 3);
        assert!(written.starts_with("CKCAP {"));
        assert!(written.contains(r#""fields":["password","handleOrEmail"]"#));
        assert!(!written.contains("hunter2"));
    }

    #[test]
    fn a_broken_message_gets_an_error_reply_and_the_server_goes_on() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            serve(
                &listener,
                Box::new(Observer),
                Box::new(Capture::default()),
                Box::new(NoEvents),
                "",
            )
        });
        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(b"nonsense\r\n\r\n").unwrap();
        assert!(read_reply(&mut stream).starts_with("ICAP/1.0 500"));

        let mut again = TcpStream::connect(address).unwrap();
        again
            .write_all(b"OPTIONS icap://x/respmod ICAP/1.0\r\nEncapsulated: null-body=0\r\n\r\n")
            .unwrap();
        assert!(read_reply(&mut again).contains("Methods: RESPMOD"));
    }

    struct Recorded(Arc<Mutex<Vec<(EventKind, serde_json::Value)>>>);

    impl EventSink for Recorded {
        fn send(&self, kind: EventKind, payload: serde_json::Value) -> Result<(), String> {
            self.0.lock().unwrap().push((kind, payload));
            Ok(())
        }
    }

    /// A rule that says "login" for every POST to /enter, to check the way an event travels.
    struct LoginOnEnter;

    impl Rule for LoginOnEnter {
        fn on_request(&mut self, request: &Request) -> Vec<Output> {
            if request.head.method == "POST" && request.head.path() == "/enter" {
                let payload = serde_json::json!({ "username": "ana", "platform": "test" });
                return vec![Output::Event {
                    kind: EventKind::Login,
                    payload,
                }];
            }
            Vec::new()
        }

        fn on_response(&mut self, _request: &crate::Head, _response: &Response) -> Vec<Output> {
            Vec::new()
        }
    }

    #[test]
    fn an_event_from_a_rule_reaches_the_event_sink() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Recorded(Arc::clone(&events));
        std::thread::spawn(move || {
            serve(
                &listener,
                Box::new(LoginOnEnter),
                Box::new(Capture::default()),
                Box::new(sink),
                "",
            )
        });
        let head = "POST https://example.org/enter HTTP/1.1

";
        let message = format!(
            "REQMOD icap://x/reqmod ICAP/1.0
Allow: 204
Encapsulated: req-hdr=0, null-body={}

{head}",
            head.len()
        );
        let mut stream = TcpStream::connect(address).unwrap();
        stream.write_all(message.as_bytes()).unwrap();
        assert!(read_reply(&mut stream).starts_with("ICAP/1.0 204"));
        let seen = events.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, EventKind::Login);
        assert_eq!(seen[0].1["username"], "ana");
    }
}
