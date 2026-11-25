//! The Codeforces rule: tells the Hub who is logged in and what they submit.
//!
//! The proxy decrypts the traffic of the allowed sites, so a page and a form are seen as they are.
//! Only public facts leave this module: the handle, the contest, the problem, the language, and a
//! hash and a size of the source (the same code on two sticks shows without anyone's code being
//! kept). No password, cookie, token, e-mail address or source code is ever put in an event.

use std::time::{SystemTime, UNIX_EPOCH};

use ck_crypto::sha256_hex;
use serde_json::json;

use crate::form::{pick, text};
use crate::{EventKind, Head, Observer, Output, Request, Response, Rule};

const SUBMIT_ACTION: &str = "submitSolutionFormSubmitted";
/// Submissions that wait for the site's answer. A contestant sends few at a time.
const MAX_PENDING: usize = 8;
/// The top of a page holds the name of the logged-in user; nothing further down is read for it.
const HEADER_LOOKAHEAD: usize = 3000;

struct Pending {
    path: String,
    contest_id: Option<String>,
    problem: Option<String>,
    language_id: Option<String>,
    source_sha256: Option<String>,
    /// The hash of the source after [`normalize_source`], to compare with what Codeforces shows.
    source_sha256_norm: Option<String>,
    source_bytes: usize,
    source_cut: bool,
    /// Unix seconds when the submit was seen: the Hub compares this with the judge's time, not
    /// the time the event was queued, which can be later.
    submitted_at: u64,
}

/// An accepted submission, already told, that waits for the page of submissions with its id.
struct Awaiting {
    sent: Pending,
    user: Option<String>,
}

/// Watches Codeforces. With `observe` it also writes what the traffic looks like (see [`Observer`]).
pub struct Codeforces {
    observer: Option<Observer>,
    user: Option<String>,
    pending: Vec<Pending>,
    awaiting: Option<Awaiting>,
}

impl Codeforces {
    #[must_use]
    pub fn new(observe: bool) -> Self {
        Self {
            observer: observe.then_some(Observer),
            user: None,
            pending: Vec::new(),
            awaiting: None,
        }
    }

    /// Tells the id of the submission that waited for it, when the page names it. The submission
    /// itself was told at the redirect, so a closed browser never loses it.
    fn finish_awaiting(&mut self, id: Option<u64>) -> Vec<Output> {
        let waiting = self.awaiting.take();
        match (waiting, id) {
            (Some(waiting), Some(id)) => vec![submission_event(
                &waiting.sent,
                waiting.user.as_deref(),
                true,
                Some(id),
            )],
            _ => Vec::new(),
        }
    }
}

/// The source as Codeforces shows it: one kind of line end, no trailing spaces, no trailing blank
/// lines. The Hub applies the same steps to the page it reads, so the two hashes can be compared.
fn normalize_source(source: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(source)
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let lines: Vec<&str> = text
        .split('\n')
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect();
    lines.join("\n").trim_end_matches('\n').as_bytes().to_vec()
}

/// The id of the newest submission in a page of submissions, when it is the one of `sent`.
fn newest_submission_id(page: &str, sent: &Pending) -> Option<u64> {
    const MARK: &str = "data-submission-id=\"";
    const ROW_LOOKAHEAD: usize = 4000;
    let after = page.split(MARK).nth(1)?;
    let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
    let id = digits.parse().ok()?;
    let row: String = after.chars().take(ROW_LOOKAHEAD).collect();
    let row = row.split("</tr>").next().unwrap_or(&row);
    let same_problem = sent
        .problem
        .as_ref()
        .is_none_or(|problem| row.contains(&format!("/{problem}\"")));
    let same_contest = sent
        .contest_id
        .as_ref()
        .is_none_or(|contest| row.contains(&format!("/{contest}/")));
    (same_problem && same_contest).then_some(id)
}

/// With an id this is the follow-up of an earlier event for the same submit (same `submitted_at`).
fn submission_event(sent: &Pending, user: Option<&str>, accepted: bool, id: Option<u64>) -> Output {
    Output::Event {
        kind: EventKind::Submission,
        payload: json!({
            "platform": "codeforces",
            "username": user,
            "contest_id": sent.contest_id,
            "problem": sent.problem,
            "language": sent.language_id,
            "source_sha256": sent.source_sha256,
            "source_sha256_norm": sent.source_sha256_norm,
            "source_bytes": sent.source_bytes,
            "source_cut": sent.source_cut,
            "accepted": accepted,
            "submission_id": id,
            "submitted_at": sent.submitted_at,
            "follow_up": id.is_some(),
        }),
    }
}

fn is_codeforces(head: &Head) -> bool {
    let host = head.host();
    host == "codeforces.com" || host.ends_with(".codeforces.com")
}

/// What a page says about its reader.
enum Reader {
    LoggedIn(String),
    Visitor,
}

/// The reader a page shows, or nothing for a page that tells nothing (a fragment, a file, an error).
fn page_reader(content_type: &str, body: &[u8]) -> Option<Reader> {
    const BLOCK: &str = "class=\"lang-chooser\"";
    const PROFILE: &str = "href=\"/profile/";
    if !content_type.to_ascii_lowercase().starts_with("text/html") {
        return None;
    }
    let page = String::from_utf8_lossy(body);
    let start = page.find(BLOCK)?;
    let header: String = page[start..].chars().take(HEADER_LOOKAHEAD).collect();
    if header.contains("href=\"/enter") {
        return Some(Reader::Visitor);
    }
    let after = header.split(PROFILE).nth(1)?;
    let handle: String = after
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .take(32)
        .collect();
    (!handle.is_empty()).then_some(Reader::LoggedIn(handle))
}

/// A submission form posts to the problem's page or to a submit page, with this action field.
fn submission_from(request: &Request) -> Option<Pending> {
    let head = &request.head;
    if head.method != "POST" || !is_codeforces(head) {
        return None;
    }
    let content_type = head.header("content-type").unwrap_or("");
    let fields = pick(
        content_type,
        &request.body,
        &[
            "action",
            "contestId",
            "submittedProblemIndex",
            "programTypeId",
            "source",
            "sourceFile",
        ],
    );
    if text(&fields, "action").as_deref() != Some(SUBMIT_ACTION) {
        // A body cut by the preview before the action field is not seen as a submission.
        if request.body_truncated && !request.body.is_empty() {
            tracing::debug!("a POST to codeforces was cut before its action field; not recorded");
        }
        return None;
    }
    // The code is in a text field or in an uploaded file.
    let source = fields
        .iter()
        .find(|(name, value)| (name == "source" || name == "sourceFile") && !value.is_empty())
        .map(|(_, value)| value.as_slice());
    let path = head.path();
    Some(Pending {
        path: path.to_owned(),
        contest_id: text(&fields, "contestId").or_else(|| contest_of_path(path)),
        problem: text(&fields, "submittedProblemIndex"),
        language_id: text(&fields, "programTypeId"),
        source_sha256: source.map(sha256_hex),
        // A cut source cannot match the whole one, so it gets no comparable hash.
        source_sha256_norm: source
            .filter(|_| !request.body_truncated)
            .map(|code| sha256_hex(&normalize_source(code))),
        source_bytes: source.map_or(0, <[u8]>::len),
        source_cut: request.body_truncated,
        submitted_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs()),
    })
}

/// 2285 in `/contest/2285/problem/A`, `/contest/2285/submit` and `/problemset/problem/2285/A`.
fn contest_of_path(path: &str) -> Option<String> {
    let parts: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    let id = match parts.as_slice() {
        ["contest" | "gym", id, ..] | ["problemset", "problem", id, ..] => *id,
        _ => return None,
    };
    id.bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| id.to_owned())
}

impl Rule for Codeforces {
    fn on_request(&mut self, request: &Request) -> Vec<Output> {
        let outputs = match &mut self.observer {
            Some(observer) => observer.on_request(request),
            None => Vec::new(),
        };
        if let Some(submission) = submission_from(request) {
            // A new submit comes first: the page of submissions would name this one, not the earlier.
            self.awaiting = None;
            self.pending.retain(|old| old.path != submission.path);
            if self.pending.len() >= MAX_PENDING {
                self.pending.remove(0);
            }
            self.pending.push(submission);
        }
        outputs
    }

    fn on_response(&mut self, request: &Head, response: &Response) -> Vec<Output> {
        let mut outputs = match &mut self.observer {
            Some(observer) => observer.on_response(request, response),
            None => Vec::new(),
        };
        if !is_codeforces(request) {
            return outputs;
        }
        if request.method == "POST"
            && let Some(at) = self.pending.iter().position(|p| p.path == request.path())
        {
            let sent = self.pending.remove(at);
            // The site answers a taken submission with a redirect to the list of submissions.
            let accepted = response.status / 100 == 3;
            outputs.push(submission_event(
                &sent,
                self.user.as_deref(),
                accepted,
                None,
            ));
            self.awaiting = accepted.then(|| Awaiting {
                sent,
                user: self.user.clone(),
            });
            return outputs;
        }
        let content_type = response.head.header("content-type").unwrap_or("");
        if request.method == "GET"
            && content_type.to_ascii_lowercase().starts_with("text/html")
            && let Some(waiting) = &self.awaiting
        {
            // The first page after the redirect tells the id; any other page means it was missed.
            let id = newest_submission_id(&String::from_utf8_lossy(&response.body), &waiting.sent);
            outputs.extend(self.finish_awaiting(id));
        }
        match page_reader(content_type, &response.body) {
            Some(Reader::LoggedIn(handle)) if self.user.as_deref() != Some(&handle) => {
                self.user = Some(handle.clone());
                outputs.push(Output::Event {
                    kind: EventKind::Login,
                    payload: json!({ "platform": "codeforces", "username": handle }),
                });
            }
            // Logged out: the next login, even of the same person, is told again.
            Some(Reader::Visitor) => self.user = None,
            _ => {}
        }
        outputs
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_request_head, parse_response_head};

    const NO_EVENTS: [serde_json::Value; 0] = [];

    fn events(outputs: Vec<Output>) -> Vec<serde_json::Value> {
        outputs
            .into_iter()
            .filter_map(|output| match output {
                Output::Event { payload, .. } => Some(payload),
                Output::Note(_) => None,
            })
            .collect()
    }

    fn page(body: &str) -> (Head, Response) {
        let request = parse_request_head(b"GET https://codeforces.com/ HTTP/1.1\r\n\r\n").unwrap();
        let response = Response {
            status: 200,
            head: parse_response_head(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n")
                .unwrap(),
            body: body.as_bytes().to_vec(),
            body_truncated: false,
        };
        (request, response)
    }

    const LOGGED_IN: &str = "<div class=\"lang-chooser\"><a href=\"?locale=en\">en</a>\
        <a href=\"/profile/ana_2\" class=\"rated-user\">ana_2</a> | <a href=\"/settings/general\">Settings</a>\
        | <a href=\"/logout\">Logout</a></div>";
    const LOGGED_OUT: &str = "<div class=\"lang-chooser\"><a href=\"/enter?back=%2F\">Enter</a> | \
        <a href=\"/register\">Register</a></div><a href=\"/profile/tourist\">tourist</a>";

    #[test]
    fn a_login_is_told_once_and_again_after_a_logout() {
        let mut rule = Codeforces::new(false);
        let (request, in_page) = page(LOGGED_IN);
        let first = events(rule.on_response(&request, &in_page));
        assert_eq!(
            first,
            [json!({"platform": "codeforces", "username": "ana_2"})]
        );
        assert_eq!(events(rule.on_response(&request, &in_page)), NO_EVENTS);

        let (_, out_page) = page(LOGGED_OUT);
        assert_eq!(events(rule.on_response(&request, &out_page)), NO_EVENTS);
        assert_eq!(events(rule.on_response(&request, &in_page)).len(), 1);
    }

    #[test]
    fn a_visitor_page_with_other_profile_links_is_not_a_login() {
        let mut rule = Codeforces::new(false);
        let (request, out_page) = page(LOGGED_OUT);
        assert_eq!(events(rule.on_response(&request, &out_page)), NO_EVENTS);
        assert_eq!(rule.user, None);
    }

    #[test]
    fn other_sites_and_other_content_are_ignored() {
        let mut rule = Codeforces::new(false);
        let (_, in_page) = page(LOGGED_IN);
        let other = parse_request_head(b"GET https://example.org/ HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(events(rule.on_response(&other, &in_page)), NO_EVENTS);
        let (request, mut css) = page(LOGGED_IN);
        css.head =
            parse_response_head(b"HTTP/1.1 200 OK\r\nContent-Type: text/css\r\n\r\n").unwrap();
        assert_eq!(events(rule.on_response(&request, &css)), NO_EVENTS);
    }

    const PASSWORD: &str = "hunter2-pass";
    const TOKEN: &str = "tok3n-value-xyz";

    fn submit(path: &str, code: &str) -> (Request, Head) {
        let body = format!(
            "--b\r\nContent-Disposition: form-data; name=\"csrf_token\"\r\n\r\n{TOKEN}\r\n\
--b\r\nContent-Disposition: form-data; name=\"action\"\r\n\r\n{SUBMIT_ACTION}\r\n\
--b\r\nContent-Disposition: form-data; name=\"contestId\"\r\n\r\n2285\r\n\
--b\r\nContent-Disposition: form-data; name=\"submittedProblemIndex\"\r\n\r\nB\r\n\
--b\r\nContent-Disposition: form-data; name=\"programTypeId\"\r\n\r\n54\r\n\
--b\r\nContent-Disposition: form-data; name=\"source\"\r\n\r\n{code}\r\n--b--\r\n"
        );
        let head = parse_request_head(
            format!(
                "POST https://codeforces.com{path}?csrf_token={TOKEN} HTTP/1.1\r\n\
                 Content-Type: multipart/form-data; boundary=b\r\nCookie: JSESSIONID={PASSWORD}\r\n\r\n"
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

    fn answer(status: u16) -> Response {
        Response {
            status,
            head: parse_response_head(format!("HTTP/1.1 {status} X\r\n\r\n").as_bytes()).unwrap(),
            body: Vec::new(),
            body_truncated: false,
        }
    }

    #[test]
    fn a_taken_submission_is_told_with_its_facts_and_never_its_code_or_secrets() {
        let mut rule = Codeforces::new(false);
        let (request, in_page) = page(LOGGED_IN);
        let _ = rule.on_response(&request, &in_page);

        let (post, head) = submit("/contest/2285/problem/B", "int main() { return 0; }");
        assert_eq!(rule.on_request(&post), Vec::<Output>::new());
        // The redirect tells the submit at once, with no id: no later traffic is needed.
        let first = events(rule.on_response(&head, &answer(302)));
        assert_eq!(first.len(), 1);
        assert_eq!(first[0]["submission_id"], serde_json::Value::Null);
        assert_eq!(first[0]["follow_up"], false);
        assert!(first[0]["submitted_at"].as_u64().unwrap() > 1_700_000_000);
        // The page of submissions follows with the id, for the same submit.
        let (request, list) = my_page(&my_row(245_000_123, "/contest/2285/problem/B"));
        let told = events(rule.on_response(&request, &list));
        assert_eq!(told.len(), 1);
        let event = &told[0];
        assert_eq!(event["follow_up"], true);
        assert_eq!(event["submitted_at"], first[0]["submitted_at"]);
        assert_eq!(event["username"], "ana_2");
        assert_eq!(event["contest_id"], "2285");
        assert_eq!(event["problem"], "B");
        assert_eq!(event["language"], "54");
        assert_eq!(event["accepted"], true);
        assert_eq!(event["submission_id"], 245_000_123);
        assert_eq!(event["source_bytes"], 24);
        assert_eq!(
            event["source_sha256"],
            sha256_hex(b"int main() { return 0; }")
        );
        let line = event.to_string();
        for secret in [PASSWORD, TOKEN, "int main"] {
            assert!(!line.contains(secret), "{secret} leaked into {line}");
        }
    }

    fn my_row(id: u64, problem_link: &str) -> String {
        format!(
            "<table><tr data-submission-id=\"{id}\"><td><a href=\"{problem_link}\">B</a></td></tr>\
             <tr data-submission-id=\"1\"><td><a href=\"/contest/2285/problem/A\">A</a></td></tr></table>"
        )
    }

    fn my_page(body: &str) -> (Head, Response) {
        let request =
            parse_request_head(b"GET https://codeforces.com/contest/2285/my HTTP/1.1\r\n\r\n")
                .unwrap();
        let response = Response {
            status: 200,
            head: parse_response_head(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n")
                .unwrap(),
            body: body.as_bytes().to_vec(),
            body_truncated: false,
        };
        (request, response)
    }

    #[test]
    fn the_id_of_another_problem_is_not_taken() {
        let mut rule = Codeforces::new(false);
        let (post, head) = submit("/contest/2285/problem/B", "x");
        let _ = rule.on_request(&post);
        let told = events(rule.on_response(&head, &answer(302)));
        assert_eq!(told[0]["accepted"], true);
        let (request, list) = my_page(&my_row(9, "/contest/2285/problem/C"));
        assert_eq!(events(rule.on_response(&request, &list)), NO_EVENTS);
    }

    #[test]
    fn a_new_submit_before_the_list_gives_the_earlier_one_no_id() {
        let mut rule = Codeforces::new(false);
        let (post, head) = submit("/contest/2285/problem/B", "first");
        let _ = rule.on_request(&post);
        assert_eq!(events(rule.on_response(&head, &answer(302))).len(), 1);
        let (again, _) = submit("/contest/2285/problem/B", "second");
        // Nothing more is told for the first: the newest row of the list is the second's.
        assert_eq!(rule.on_request(&again), Vec::<Output>::new());
        let (request, list) = my_page(&my_row(77, "/contest/2285/problem/B"));
        assert_eq!(events(rule.on_response(&request, &list)), NO_EVENTS);
    }

    #[test]
    fn a_page_without_submissions_tells_no_id_and_the_submit_stays_told() {
        let mut rule = Codeforces::new(false);
        let (post, head) = submit("/contest/2285/problem/B", "x");
        let _ = rule.on_request(&post);
        assert_eq!(events(rule.on_response(&head, &answer(302))).len(), 1);
        let (request, other) = page("<html>no table</html>");
        assert_eq!(events(rule.on_response(&request, &other)), NO_EVENTS);
    }

    #[test]
    fn the_comparable_hash_ignores_line_ends_and_trailing_blanks() {
        let unix = normalize_source(b"int main() {\n  return 0;\n}\n");
        let windows = normalize_source(b"int main() {  \r\n  return 0;\t\r\n}\r\n\r\n");
        assert_eq!(unix, windows);
        assert_eq!(unix, b"int main() {\n  return 0;\n}");

        let mut rule = Codeforces::new(false);
        let (post, head) = submit("/contest/2285/problem/B", "a\r\nb\r\n");
        let _ = rule.on_request(&post);
        let told = events(rule.on_response(&head, &answer(302)));
        assert_eq!(told[0]["source_sha256_norm"], sha256_hex(b"a\nb"));
    }

    #[test]
    fn a_refused_submission_is_told_as_not_accepted_and_only_once() {
        let mut rule = Codeforces::new(false);
        let (post, head) = submit("/contest/2285/submit", "x");
        let _ = rule.on_request(&post);
        let told = events(rule.on_response(&head, &answer(200)));
        assert_eq!(told[0]["accepted"], false);
        assert_eq!(told[0]["username"], serde_json::Value::Null);
        assert_eq!(events(rule.on_response(&head, &answer(200))), NO_EVENTS);
    }

    #[test]
    fn a_post_without_the_submit_action_is_not_a_submission() {
        let mut rule = Codeforces::new(false);
        let head = parse_request_head(
            b"POST https://codeforces.com/enter HTTP/1.1\r\n\
              Content-Type: application/x-www-form-urlencoded\r\n\r\n",
        )
        .unwrap();
        let request = Request {
            head: head.clone(),
            body: format!("action=enter&handleOrEmail=ana%40example.org&password={PASSWORD}")
                .into_bytes(),
            body_truncated: false,
        };
        let _ = rule.on_request(&request);
        assert_eq!(events(rule.on_response(&head, &answer(302))), NO_EVENTS);
    }

    #[test]
    fn the_contest_comes_from_the_path_when_the_form_has_none() {
        assert_eq!(
            contest_of_path("/contest/2285/problem/A").as_deref(),
            Some("2285")
        );
        assert_eq!(
            contest_of_path("/problemset/problem/1/A").as_deref(),
            Some("1")
        );
        assert_eq!(contest_of_path("/contest/abc/submit"), None);
        assert_eq!(contest_of_path("/enter"), None);
    }
}
