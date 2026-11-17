//! The network history: what the proxy carried or refused, read from Squid's access log.
//!
//! Each line becomes one entry with the time, the method, the site, the path without its query,
//! the answer, the size and the type. The query, the cookies and the bodies are never in the log
//! line or in an entry, so a password or a session id cannot travel with it.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};

use crate::{EventKind, EventSink};

/// Entries in one event. The queue refuses a payload over 16 KB; with the length limits below this
/// stays well under it.
const ENTRIES_PER_EVENT: usize = 30;
/// How much of the log one round reads, so a burst of traffic goes out over a few rounds and stays
/// under the Hub's limit on messages per minute.
const READ_PER_ROUND: u64 = 64 * 1024;
const MAX_HOST: usize = 80;
const MAX_PATH: usize = 100;
const MAX_TYPE: usize = 40;

/// One line of Squid's `squid` log format, or `None` for a line that is not one:
/// `time elapsed client result/status bytes method url user hierarchy/peer type`.
pub fn parse_line(line: &str) -> Option<Value> {
    let mut parts = line.split_whitespace();
    let time = parts.next()?.split('.').next()?.parse::<i64>().ok()?;
    let elapsed = parts.next()?.parse::<u64>().ok()?;
    let _client = parts.next()?;
    let (result, status) = parts.next()?.split_once('/')?;
    let status = status.parse::<u16>().ok()?;
    let bytes = parts.next()?.parse::<u64>().ok()?;
    let method = parts.next()?;
    let url = parts.next()?;
    let _user = parts.next()?;
    let _peer = parts.next()?;
    let kind = parts.next().unwrap_or("-");
    let (host, path) = site_and_path(method, url);
    Some(json!({
        "t": time,
        "ms": elapsed,
        "method": cut(method, 16),
        "host": cut(host, MAX_HOST),
        "path": cut(path, MAX_PATH),
        "status": status,
        "result": cut(result, 32),
        "bytes": bytes,
        "type": cut(kind, MAX_TYPE),
    }))
}

/// The site and the path of a URL, without the user name and password in it and without the query.
fn site_and_path<'a>(method: &str, url: &'a str) -> (&'a str, &'a str) {
    // A tunnel is asked for as `host:port`.
    if method == "CONNECT" {
        return (url, "");
    }
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    let (authority, path) = match rest.find('/') {
        Some(at) => rest.split_at(at),
        None => (rest, "/"),
    };
    let host = authority.rsplit('@').next().unwrap_or(authority);
    let path = path.split(['?', '#']).next().unwrap_or(path);
    (host, path)
}

fn cut(text: &str, longest: usize) -> String {
    text.chars().take(longest).collect()
}

/// Reads the lines added to a log, from where the last read stopped.
pub struct Tail {
    path: PathBuf,
    offset: u64,
    /// Lines from before this time (seconds since 1970) are from an earlier boot.
    since: i64,
}

impl Tail {
    pub fn new(path: PathBuf, offset: u64, since: i64) -> Self {
        Self {
            path,
            offset,
            since,
        }
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// The entries of the lines added since the last call, at most a round's worth. A line that is
    /// not finished yet waits for the next call. A log that got shorter was rotated and is read
    /// again from its start.
    pub fn read_new(&mut self) -> std::io::Result<Vec<Value>> {
        let mut file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        if file.metadata()?.len() < self.offset {
            self.offset = 0;
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut chunk = Vec::new();
        file.take(READ_PER_ROUND).read_to_end(&mut chunk)?;
        let Some(end) = chunk.iter().rposition(|&byte| byte == b'\n') else {
            return Ok(Vec::new());
        };
        self.offset += end as u64 + 1;
        let text = String::from_utf8_lossy(&chunk[..end]);
        let mut entries = Vec::new();
        for line in text.lines() {
            if let Some(entry) = parse_line(line)
                && entry["t"].as_i64().unwrap_or(0) >= self.since
            {
                entries.push(entry);
            }
        }
        Ok(entries)
    }
}

/// Follows the log for ever: every `interval` the new entries go to `events` in batches, and the
/// place reached is written to `state` so a restart of the agent does not send them twice.
pub fn follow(
    log: &Path,
    state: &Path,
    since: i64,
    events: &Arc<dyn EventSink>,
    interval: Duration,
) {
    let offset = std::fs::read_to_string(state)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0);
    let mut tail = Tail::new(log.to_owned(), offset, since);
    loop {
        match tail.read_new() {
            Ok(entries) if entries.is_empty() => {}
            Ok(entries) => {
                for batch in entries.chunks(ENTRIES_PER_EVENT) {
                    if let Err(error) = events.send(EventKind::Network, json!({ "entries": batch }))
                    {
                        tracing::warn!("network entries could not be queued: {error}");
                    }
                }
                if let Err(error) = std::fs::write(state, tail.offset().to_string()) {
                    tracing::warn!("cannot remember how far the log was read: {error}");
                }
            }
            Err(error) => tracing::warn!("cannot read the proxy log: {error}"),
        }
        std::thread::sleep(interval);
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    const GET: &str = "1763312345.123    456 10.0.2.15 TCP_MISS/200 1234 GET \
                       https://user:pw@codeforces.com/contest/2285/submit?csrf_token=SECRET - \
                       HIER_DIRECT/1.2.3.4 text/html";

    #[test]
    fn a_get_gives_the_site_and_the_path_but_no_query_and_no_user() {
        let entry = parse_line(GET).unwrap();
        assert_eq!(entry["t"], 1_763_312_345);
        assert_eq!(entry["ms"], 456);
        assert_eq!(entry["method"], "GET");
        assert_eq!(entry["host"], "codeforces.com");
        assert_eq!(entry["path"], "/contest/2285/submit");
        assert_eq!(entry["status"], 200);
        assert_eq!(entry["result"], "TCP_MISS");
        assert_eq!(entry["bytes"], 1234);
        assert_eq!(entry["type"], "text/html");
        let text = entry.to_string();
        assert!(!text.contains("SECRET") && !text.contains("pw@"));
    }

    #[test]
    fn a_tunnel_and_a_refusal_are_entries_too() {
        let tunnel = "1763312346.001 5000 10.0.2.15 TCP_TUNNEL/200 8000 CONNECT \
                      codeforces.com:443 - HIER_DIRECT/1.2.3.4 -";
        let entry = parse_line(tunnel).unwrap();
        assert_eq!(entry["host"], "codeforces.com:443");
        assert_eq!(entry["path"], "");
        assert_eq!(entry["result"], "TCP_TUNNEL");

        let refused = "1763312347.500 0 10.0.2.15 TCP_DENIED/403 3900 GET \
                       http://docs.python.org/3/ - HIER_NONE/- text/html";
        let entry = parse_line(refused).unwrap();
        assert_eq!(entry["host"], "docs.python.org");
        assert_eq!(entry["status"], 403);
        assert_eq!(entry["result"], "TCP_DENIED");
    }

    #[test]
    fn a_line_that_is_not_a_log_line_is_left_out() {
        assert!(parse_line("").is_none());
        assert!(parse_line("not a log line at all").is_none());
        assert!(parse_line("1763312345.1 1 a TCP_MISS 1 GET http://x/ - - -").is_none());
    }

    #[test]
    fn long_values_are_cut() {
        let long = "a".repeat(500);
        let line = format!("1.0 1 c TCP_MISS/200 1 GET http://{long}.com/{long} - - {long}");
        let entry = parse_line(&line).unwrap();
        assert_eq!(entry["host"].as_str().unwrap().len(), MAX_HOST);
        assert_eq!(entry["path"].as_str().unwrap().len(), MAX_PATH);
        assert_eq!(entry["type"].as_str().unwrap().len(), MAX_TYPE);
    }

    #[test]
    fn the_tail_gives_each_line_once_and_waits_for_an_unfinished_one() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("access.log");
        let mut tail = Tail::new(log.clone(), 0, 0);
        assert_eq!(tail.read_new().unwrap().len(), 0, "no log yet");

        let mut file = File::create(&log).unwrap();
        writeln!(file, "{GET}").unwrap();
        write!(file, "1763312346.001 5000 10.0.2.15 TCP_TUNNEL/200").unwrap();
        assert_eq!(tail.read_new().unwrap().len(), 1);
        assert_eq!(tail.read_new().unwrap().len(), 0);

        writeln!(
            file,
            " 8000 CONNECT codeforces.com:443 - HIER_DIRECT/1.2.3.4 -"
        )
        .unwrap();
        let entries = tail.read_new().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["method"], "CONNECT");
    }

    #[test]
    fn lines_from_before_the_boot_are_skipped_and_a_rotated_log_is_read_again() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("access.log");
        std::fs::write(&log, format!("{GET}\n{GET}\n")).unwrap();
        let mut tail = Tail::new(log.clone(), 0, 1_763_312_346);
        assert_eq!(tail.read_new().unwrap().len(), 0, "older than the boot");

        std::fs::write(
            &log,
            "1763312400.000 1 c TCP_MISS/200 1 GET http://a.org/ - - -\n",
        )
        .unwrap();
        assert_eq!(
            tail.read_new().unwrap().len(),
            1,
            "a shorter log starts over"
        );
    }

    struct Recorded(std::sync::Mutex<Vec<Value>>);

    impl EventSink for Recorded {
        fn send(&self, kind: EventKind, payload: Value) -> Result<(), String> {
            assert_eq!(kind, EventKind::Network);
            self.0.lock().unwrap().push(payload);
            Ok(())
        }
    }

    #[test]
    fn follow_sends_batches_and_remembers_the_place() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("access.log");
        let state = dir.path().join("offset");
        let lines = format!("{GET}\n").repeat(ENTRIES_PER_EVENT + 5);
        std::fs::write(&log, lines).unwrap();
        let sink = Arc::new(Recorded(std::sync::Mutex::new(Vec::new())));
        let events: Arc<dyn EventSink> = sink.clone();
        let (log2, state2) = (log.clone(), state.clone());
        std::thread::spawn(move || follow(&log2, &state2, 0, &events, Duration::from_millis(10)));
        for _ in 0..200 {
            if sink.0.lock().unwrap().len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let sent = sink.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 2);
        assert_eq!(
            sent[0]["entries"].as_array().unwrap().len(),
            ENTRIES_PER_EVENT
        );
        assert_eq!(sent[1]["entries"].as_array().unwrap().len(), 5);
        std::thread::sleep(Duration::from_millis(50));
        let saved: u64 = std::fs::read_to_string(&state)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert_eq!(saved, std::fs::metadata(&log).unwrap().len());
    }
}
