//! The `VJudge` rule: tells the Hub who submits to which contest problem, and the run id and time
//! `VJudge` gave it.
//!
//! A submit is `POST /contest/submit/{contest}/{problem}` with a form; `VJudge` answers `{"runId": n}`.
//! The page then asks `POST /solution/data/{runId}`, whose answer names the author and `VJudge`'s own
//! submit time. The Hub compares that time with what `VJudge` lists, so the stick's clock is not
//! involved. Only public facts leave this module: no password, cookie, access code or source code.

use std::time::{Duration, Instant};

use ck_crypto::sha256_hex;
use serde_json::{Value, json};

use crate::form::{pick, text};
use crate::{EventKind, Head, Output, Request, Response, Rule};

/// How long an accepted submit waits for the page that names its author and time before it is
/// told without them. The page asks at once, so this only matters when it never does.
const HOLD: Duration = Duration::from_secs(10);
/// Submits that wait for an answer. A contestant sends few at a time.
const MAX_WAITING: usize = 8;

struct Pending {
    path: String,
    contest_id: String,
    problem: String,
    language_id: Option<String>,
    source_sha256: Option<String>,
    source_bytes: usize,
    source_cut: bool,
}

/// A submit `VJudge` took, waiting for the page that says who made it and when.
struct Awaiting {
    sent: Pending,
    run_id: u64,
    since: Instant,
}

/// Watches `VJudge` for submissions.
#[derive(Default)]
pub struct Vjudge {
    user: Option<String>,
    pending: Vec<Pending>,
    awaiting: Vec<Awaiting>,
}

impl Vjudge {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Tells the submits that waited too long, without an author or a time.
    fn flush_stale(&mut self, now: Instant) -> Vec<Output> {
        let (stale, fresh): (Vec<_>, Vec<_>) = std::mem::take(&mut self.awaiting)
            .into_iter()
            .partition(|waiting| now.saturating_duration_since(waiting.since) >= HOLD);
        self.awaiting = fresh;
        // Nothing says whose run it is, and the last user may be another person's: no name.
        stale
            .iter()
            .map(|waiting| event(waiting, None, None))
            .collect()
    }
}

fn event(waiting: &Awaiting, author: Option<&str>, judge_time_ms: Option<i64>) -> Output {
    let sent = &waiting.sent;
    Output::Event {
        kind: EventKind::Submission,
        payload: json!({
            "platform": "vjudge",
            "username": author,
            "contest_id": sent.contest_id,
            "problem": sent.problem,
            "language": sent.language_id,
            "source_sha256": sent.source_sha256,
            "source_bytes": sent.source_bytes,
            "source_cut": sent.source_cut,
            "accepted": true,
            "submission_id": waiting.run_id,
            "judge_time_ms": judge_time_ms,
        }),
    }
}

fn is_vjudge(head: &Head) -> bool {
    let host = head.host();
    host == "vjudge.net" || host.ends_with(".vjudge.net")
}

/// `("871412", "C")` in `/contest/submit/871412/C`.
fn submit_path(path: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    match parts.as_slice() {
        ["contest", "submit", contest, problem]
            if contest.bytes().all(|b| b.is_ascii_digit()) && !contest.is_empty() =>
        {
            Some(((*contest).to_owned(), (*problem).to_owned()))
        }
        _ => None,
    }
}

/// `74477117` in `/solution/data/74477117`.
fn solution_run(path: &str) -> Option<u64> {
    path.strip_prefix("/solution/data/")?.parse().ok()
}

fn submission_from(request: &Request) -> Option<Pending> {
    let head = &request.head;
    if head.method != "POST" || !is_vjudge(head) {
        return None;
    }
    let (contest_id, problem) = submit_path(head.path())?;
    let fields = pick(
        head.header("content-type").unwrap_or(""),
        &request.body,
        &["language", "source"],
    );
    let source = fields
        .iter()
        .find(|(name, value)| name == "source" && !value.is_empty())
        .map(|(_, value)| value.as_slice());
    Some(Pending {
        path: head.path().to_owned(),
        contest_id,
        problem,
        language_id: text(&fields, "language"),
        source_sha256: source.map(sha256_hex),
        source_bytes: source.map_or(0, <[u8]>::len),
        source_cut: request.body_truncated,
    })
}

impl Rule for Vjudge {
    fn on_request(&mut self, request: &Request) -> Vec<Output> {
        let Some(submission) = submission_from(request) else {
            return Vec::new();
        };
        self.pending.retain(|old| old.path != submission.path);
        if self.pending.len() >= MAX_WAITING {
            self.pending.remove(0);
        }
        self.pending.push(submission);
        Vec::new()
    }

    fn on_idle(&mut self) -> Vec<Output> {
        self.flush_stale(Instant::now())
    }

    fn on_response(&mut self, request: &Head, response: &Response) -> Vec<Output> {
        if !is_vjudge(request) || request.method != "POST" {
            return Vec::new();
        }
        let answer: Value = serde_json::from_slice(&response.body).unwrap_or(Value::Null);
        if let Some(at) = self.pending.iter().position(|p| p.path == request.path()) {
            let sent = self.pending.remove(at);
            // `VJudge` answers a taken submit with its run id; anything else was refused.
            let Some(run_id) = answer.get("runId").and_then(Value::as_u64) else {
                let waiting = Awaiting {
                    sent,
                    run_id: 0,
                    since: Instant::now(),
                };
                let mut refused = event(&waiting, None, None);
                if let Output::Event { payload, .. } = &mut refused {
                    payload["accepted"] = json!(false);
                    payload["submission_id"] = Value::Null;
                }
                return vec![refused];
            };
            // The oldest is told without author or time rather than lost.
            let evicted = (self.awaiting.len() >= MAX_WAITING)
                .then(|| self.awaiting.remove(0))
                .map(|waiting| event(&waiting, None, None));
            self.awaiting.push(Awaiting {
                sent,
                run_id,
                since: Instant::now(),
            });
            return evicted.into_iter().collect();
        }
        let Some(run_id) = solution_run(request.path()) else {
            return Vec::new();
        };
        let Some(at) = self.awaiting.iter().position(|w| w.run_id == run_id) else {
            return Vec::new();
        };
        // An error page or an empty answer names nothing: keep waiting for the next poll or the hold.
        if answer.get("runId").and_then(Value::as_u64) != Some(run_id) {
            return Vec::new();
        }
        let author = answer
            .get("author")
            .and_then(Value::as_str)
            .filter(|name| !name.is_empty() && name.len() <= 64)
            .map(str::to_owned);
        let judge_time_ms = answer.get("submitTime").and_then(Value::as_i64);
        let waiting = self.awaiting.remove(at);
        let mut outputs = Vec::new();
        if let Some(name) = &author
            && self.user.as_deref() != Some(name)
        {
            self.user = Some(name.clone());
            outputs.push(Output::Event {
                kind: EventKind::Login,
                payload: json!({ "platform": "vjudge", "username": name }),
            });
        }
        outputs.push(event(&waiting, author.as_deref(), judge_time_ms));
        outputs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_request_head, parse_response_head};

    const NO_EVENTS: [Value; 0] = [];
    const ACCESS: &str = "secret-access-code";

    fn events(outputs: Vec<Output>) -> Vec<Value> {
        outputs
            .into_iter()
            .filter_map(|output| match output {
                Output::Event { payload, .. } => Some(payload),
                Output::Note(_) => None,
            })
            .collect()
    }

    fn submit(path: &str, code: &str) -> (Request, Head) {
        let body =
            format!("method=0&language=6116&open=0&source={code}&accessCode={ACCESS}&version=a09");
        let head = parse_request_head(
            format!(
                "POST https://vjudge.net{path} HTTP/1.1\r\n\
                 Content-Type: application/x-www-form-urlencoded; charset=UTF-8\r\n\
                 Cookie: JSESSIONID=hunter2-cookie\r\n\r\n"
            )
            .as_bytes(),
        )
        .unwrap();
        let request = Request {
            head: head.clone(),
            body: body.into_bytes(),
            body_truncated: false,
        };
        (request, head)
    }

    fn json_answer(status: u16, body: &str) -> Response {
        Response {
            status,
            head: parse_response_head(
                format!("HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n\r\n").as_bytes(),
            )
            .unwrap(),
            body: body.as_bytes().to_vec(),
            body_truncated: false,
        }
    }

    fn solution(run: u64) -> Head {
        parse_request_head(
            format!("POST https://vjudge.net/solution/data/{run} HTTP/1.1\r\n\r\n").as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn a_taken_submit_is_told_with_run_id_author_and_the_judges_time_and_never_secrets() {
        let mut rule = Vjudge::new();
        let (post, head) = submit("/contest/submit/871412/C", "int+main()");
        assert_eq!(rule.on_request(&post), Vec::<Output>::new());
        assert_eq!(
            events(rule.on_response(&head, &json_answer(200, r#"{"runId":74477117}"#))),
            NO_EVENTS
        );
        let told = events(rule.on_response(
            &solution(74_477_117),
            &json_answer(200, r#"{"author":"robbertp","submitTime":1795789024000,"runId":74477117,"code":"int main()"}"#),
        ));
        assert_eq!(told.len(), 2, "a login and the submission");
        assert_eq!(
            told[0],
            json!({"platform": "vjudge", "username": "robbertp"})
        );
        let event = &told[1];
        assert_eq!(event["platform"], "vjudge");
        assert_eq!(event["username"], "robbertp");
        assert_eq!(event["contest_id"], "871412");
        assert_eq!(event["problem"], "C");
        assert_eq!(event["language"], "6116");
        assert_eq!(event["submission_id"], 74_477_117);
        assert_eq!(event["judge_time_ms"], 1_795_789_024_000_i64);
        assert_eq!(event["accepted"], true);
        let line = event.to_string();
        for secret in [ACCESS, "hunter2-cookie", "int main"] {
            assert!(!line.contains(secret), "{secret} leaked into {line}");
        }
    }

    #[test]
    fn a_submit_whose_page_never_comes_is_told_after_the_hold_without_author_or_time() {
        let mut rule = Vjudge::new();
        let (post, head) = submit("/contest/submit/871412/C", "x");
        let _ = rule.on_request(&post);
        let _ = rule.on_response(&head, &json_answer(200, r#"{"runId":9}"#));
        assert_eq!(
            events(rule.flush_stale(Instant::now())),
            NO_EVENTS,
            "not yet"
        );
        let told = events(rule.flush_stale(Instant::now() + HOLD));
        assert_eq!(told.len(), 1);
        assert_eq!(told[0]["submission_id"], 9);
        assert_eq!(told[0]["judge_time_ms"], Value::Null);
        assert_eq!(
            events(rule.flush_stale(Instant::now() + HOLD)),
            NO_EVENTS,
            "only once"
        );
    }

    #[test]
    fn an_unusable_answer_keeps_the_run_waiting_and_the_last_user_is_never_its_author() {
        let mut rule = Vjudge::new();
        let (post, head) = submit("/contest/submit/871412/C", "x");
        let _ = rule.on_request(&post);
        let _ = rule.on_response(&head, &json_answer(200, r#"{"runId":9}"#));
        // A 503 with no body names nothing: the run stays, and a later good poll tells it.
        assert_eq!(
            events(rule.on_response(&solution(9), &json_answer(503, ""))),
            NO_EVENTS
        );
        let told = events(rule.on_response(
            &solution(9),
            &json_answer(
                200,
                r#"{"author":"ana","submitTime":1795789024000,"runId":9}"#,
            ),
        ));
        assert_eq!(told.len(), 2, "a login and the submission");
        // Another person's run whose page never comes is told with no name, though `ana` is known.
        let (post, head) = submit("/contest/submit/871412/D", "y");
        let _ = rule.on_request(&post);
        let _ = rule.on_response(&head, &json_answer(200, r#"{"runId":10}"#));
        let told = events(rule.flush_stale(Instant::now() + HOLD));
        assert_eq!(told.len(), 1);
        assert_eq!(told[0]["username"], Value::Null);
        assert_eq!(told[0]["submission_id"], 10);
    }

    #[test]
    fn the_oldest_waiting_run_is_told_not_lost_when_too_many_wait() {
        let mut rule = Vjudge::new();
        let mut told = Vec::new();
        for run in 1..=(MAX_WAITING as u64 + 1) {
            let (post, head) = submit("/contest/submit/871412/C", "x");
            let _ = rule.on_request(&post);
            told.extend(events(rule.on_response(
                &head,
                &json_answer(200, &format!(r#"{{"runId":{run}}}"#)),
            )));
        }
        assert_eq!(told.len(), 1);
        assert_eq!(told[0]["submission_id"], 1);
        assert_eq!(told[0]["username"], Value::Null);
    }

    #[test]
    fn an_answer_without_a_run_id_is_a_refused_submit_told_at_once() {
        let mut rule = Vjudge::new();
        let (post, head) = submit("/contest/submit/871412/C", "x");
        let _ = rule.on_request(&post);
        let told = events(rule.on_response(&head, &json_answer(200, r#"{"error":"wait"}"#)));
        assert_eq!(told.len(), 1);
        assert_eq!(told[0]["accepted"], false);
        assert_eq!(told[0]["submission_id"], Value::Null);
    }

    #[test]
    fn other_posts_other_sites_and_other_runs_are_ignored() {
        let mut rule = Vjudge::new();
        let other =
            parse_request_head(b"POST https://example.org/contest/submit/1/A HTTP/1.1\r\n\r\n")
                .unwrap();
        let request = Request {
            head: other.clone(),
            body: b"source=x".to_vec(),
            body_truncated: false,
        };
        assert_eq!(rule.on_request(&request), Vec::<Output>::new());
        assert_eq!(
            events(rule.on_response(&other, &json_answer(200, r#"{"runId":1}"#))),
            NO_EVENTS
        );
        // A run nobody here submitted.
        assert_eq!(
            events(rule.on_response(&solution(5), &json_answer(200, r#"{"author":"x"}"#))),
            NO_EVENTS
        );
    }

    #[test]
    fn the_contest_and_problem_come_from_the_path() {
        assert_eq!(
            submit_path("/contest/submit/871412/C"),
            Some(("871412".to_owned(), "C".to_owned()))
        );
        assert_eq!(submit_path("/contest/submit/abc/C"), None);
        assert_eq!(submit_path("/contest/871412"), None);
        assert_eq!(solution_run("/solution/data/74477117"), Some(74_477_117));
        assert_eq!(solution_run("/solution/data/x"), None);
    }
}
