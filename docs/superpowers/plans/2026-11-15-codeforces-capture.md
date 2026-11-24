# Codeforces capture on the stick Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The stick sees a contestant log in to Codeforces and submit a solution, and queues a `Login` and a `Submission` event for the Hub. No password, cookie, session id or source code is kept; a SHA-256 fingerprint of the source goes instead, as `api.md` asks.

**Architecture (route A, the one the conventions point to):** Squid already carries all of the contestant's web traffic. It decrypts `codeforces.com` only (`ssl_bump` with a CA made on the stick) and hands each request and response of that site to a small ICAP service inside `ck-agent` (`ck-agent capture`). The service answers "no change" every time, so it can never alter or delay a page beyond the check. A new crate `ck-agent-capture` holds the logic (page parsers, a state machine, the ICAP reader) and queues events through the existing `ck-agent-uplink` `Queue`.

**Route B (fallback, appendix at the end):** if the image's Squid cannot decrypt TLS, a separate intercepting proxy replaces Squid's part. It is more code and needs a network-lock change, so route A is preferred.

**Tech Stack:** Rust, `rcgen` (the stick's CA), `multer` (multipart), `sha2`, `tokio` only if the ICAP loop needs it (blocking threads are enough for one contestant), existing `ck-agent-uplink`.

**Hub side:** [ck-hub#34](https://github.com/contest-keeper/ck-hub/pull/34) reads `EventKind::Submission` payloads of the form
`{"judge":"codeforces","handle":..,"contest_id":..,"problem_index":..,"submission_id":..?}` with the time in `Event.sent_at`. This plan adds an optional `source_sha256`, which the Hub ignores for now, and a `Login` event `{"judge":"codeforces","handle":..}`.

---

## What changed from the first version of this plan

I checked every PR and issue in the ck repos: none covers capture. The conventions in `.github/conventions` did change the plan:

- `testing.md` lists **ICAP** as a boundary, and `security.md` says to use a **local CA** and never switch TLS checks off. So route A (Squid decrypts, ICAP reads) replaces my own proxy as the main design.
- `api.md`: **send SHA-256 fingerprints instead of source code**, and event types live **only in `ck-protocol`** (see "After this plan").
- `security.md`: usernames in logs only at `debug`; minimum collection. Tests: key things only, tables of cases, `proptest` for parsers, injected time.

## What Task 0 has found so far (details in `docs/capture.md`)

Done with a real login on a throwaway account, through `mitmdump` (a decrypting proxy):

- Cloudflare **accepted the re-made connection** for page views, login and submit. The first `GET /enter` was challenged ("Just a moment...", 403, twice) and Chrome solved it alone in about 4 s; after that nothing was challenged. An interactive challenge could still appear, and the contestant's Firefox would show it through Squid. Squid's handshake is not mitmproxy's: still to try on the VM.
- **Login**: `POST /enter`, 302 to `https://codeforces.com/` and the cookies `X-User` and `X-User-Sha1`. The header of a logged-in page is `<a href="/profile/HANDLE">HANDLE</a> | <a href="/TOKEN/logout">Logout</a>` inside `<div class="lang-chooser">` (the logout link has a token in its path); logged out it is `<a href="/enter?back=%2F">Enter</a> | <a href="/register">Register</a>`.
- **Submit** (contest and problemset): `POST /contest/{contest}/problem/{index}` or `POST /problemset/problem/{contest}/{index}`, with a random query parameter and `csrf_token`; multipart fields `action=submitSolutionFormSubmitted`, `submittedProblemIndex`, `programTypeId`, `source` and `sourceFile` (the source was in `sourceFile`, `source` empty). Accepted: 302 to an absolute URL: `https://codeforces.com/contest/{contest}/my`, or `https://codeforces.com/problemset/status` for the problemset. Then `GET` of that page, where the first `data-submission-id="..."` is the new submission.
- Recorded and scrubbed fixtures were in `crates/ck-agent-capture/tests/fixtures/` (`logged_in_header.html`, `logged_out_header.html`, `my_submissions_excerpt.html`). The orphan crate was removed in `chore: remove the orphan capture crate and an unused dependency`; restore the files from the history before that commit (`git log --diff-filter=D -- crates/ck-agent-capture`) when the parsers are written.

Not verified yet: gym and group submit paths, Squid on the image, ICAP as Squid sends it.

## What I could not verify (Task 0 steps 1, 5 and the gym/group paths)

1. **Squid on the image can decrypt TLS.** Debian's plain `squid` package is built with GnuTLS and has no `ssl_bump`; `squid-openssl` has. Check `squid -v` for `--with-openssl` and `--enable-ssl-crtd`, and that `security_file_certgen` exists. If not, route A needs a package change in the image patch, which may not be possible offline, and route B applies.
2. **Squid sends full bodies and `Allow: 204`** to the ICAP service (so the service can answer "unchanged" without echoing the message). The service tells Squid not to preview (no `Preview` header in its `OPTIONS` answer); check Squid honours that.
3. **Gym and group submit paths** (`/gym/{id}/problem/{index}`, `/group/{g}/contest/{id}/problem/{index}` are assumed from the contest path).
4. **Whether Cloudflare accepts Squid's own handshake.** It accepted mitmproxy's; Squid's is different. Try on the VM before relying on it.
5. Squid's own config on the image: whether `squid.conf` includes `/etc/squid/conf.d/*.conf`, the `http_port` line, and the user it runs as (`netlock.nft` assumes `proxy`).
6. The ICAP wire details (RFC 3507) as Squid sends them. Task 4 is written to the RFC and checked against a real Squid in Task 0/7.

## Decisions this plan makes

- **Login is detected from the page**, not the login form: the logged-in handle in the header of Codeforces pages. The login request, with the password, is never read. This also handles logging in with an email and a session that was already open.
- **Submit is detected from the request** (path and form) and **counts only when Codeforces redirects to "my submissions"**. Of the form only the problem field and the source's SHA-256 are used; the source itself is dropped from memory right after.
- **ICAP gives request and response together** in the response phase, so the service knows which request an answer belongs to, with no per-connection bookkeeping.
- **The submission id is best-effort**: read from the next "my submissions" page and waited for up to 10 s, then queued without it (the Hub falls back to handle, problem and time).
- **Squid asks Codeforces for uncompressed pages** (`Accept-Encoding: identity` replaced for that domain), so the service never has to decompress.
- **The CA is made on the stick**, once, key readable only by root and Squid, limited by X.509 name constraints to `codeforces.com`.
- **Needs your decision (Task 6):** what happens to Codeforces if the capture service is down. `bypass=off` (recommended): Codeforces does not load until it is back, so no submit goes unseen and falsely flagged. `bypass=on`: Codeforces keeps working, but submits made meanwhile are flagged by the Hub as unmatched. Either way only `codeforces.com` is affected, never the rest of the web.

## File structure

- Create `crates/ck-agent-capture/Cargo.toml`, `src/lib.rs`
- Create `src/codeforces.rs`: pure parsers.
- Create `src/tracker.rs`: observations from what was seen.
- Create `src/event.rs`: observation to payload, and queueing.
- Create `src/icap.rs`: reads ICAP requests, builds answers (RFC 3507 subset).
- Create `src/service.rs`: the ICAP server loop that ties `icap`, `codeforces` and `tracker` together.
- Create `src/authority.rs`: the stick's CA.
- Create `crates/ck-agent-capture/tests/fixtures/*`: recorded, scrubbed pages (Task 0).
- Create `crates/ck-agent-capture/tests/service.rs`: the service end to end over a socket.
- Modify `crates/ck-agent/src/main.rs`, `Cargo.toml`: `capture` and `capture-ca` subcommands.
- Modify `crates/ck-agent-config/src/branding.rs`: Firefox trusts the CA.
- Create `image/patch/rootfs/etc/systemd/system/ck-capture.service`, `ck-capture-ca.service`, `image/patch/rootfs/etc/squid/conf.d/ck-capture.conf`; modify `image/patch/apply-to-image.sh`, `tests/test_image_patch.py`.
- Create `docs/capture.md`: what Task 0 found, how to test.

Branch from `develop`: `feature/codeforces-capture`. All commands run in `/Users/robertpopa/projects/ck-wt/ck-os`.

---

### Task 0: Spike: facts before code

**Files:**
- Create: `docs/capture.md`, `crates/ck-agent-capture/tests/fixtures/`

Needs a person with a throwaway Codeforces account, typed by them and never by Claude or written down. Nothing committed contains a cookie, token, email or password.

- [ ] **Step 1: Squid (decides route A or B).** On a VM from the base image (`docs/testing.md`), run `squid -v`, `ls /usr/lib/squid/security_file_certgen`, `grep -n "http_port\|include" /etc/squid/squid.conf`, `ps -o user= -C squid`. Write the answers in `docs/capture.md`. If Squid cannot decrypt TLS, tell the user and switch to route B before doing anything else.

- [ ] **Step 2: Cloudflare check.** From a laptop, run `mitmproxy` (it re-makes TLS connections the way a decrypting proxy does), point a throwaway Firefox profile at it with its CA installed, log in to Codeforces, open a contest and submit a solution to a practice problem. Expected: no "Just a moment" challenge, and the submit works. If Codeforces blocks it, stop and tell the user: capture by decrypting at the stick will not work for Codeforces.

- [ ] **Step 3: Record the flows.** From the mitmproxy capture write into `docs/capture.md` (real values replaced by `HANDLE`, `1900`, `B`): paths and form field names for a contest submit, a gym submit, a group contest submit and a problemset submit; status and `Location` of an accepted submit and of a refused one; the part of a logged-in header that holds the handle, and the same part logged out; the attribute holding the submission id in "my submissions"; the content type and whether responses are compressed.

- [x] **Step 4: Fixtures** (done, see `tests/fixtures/`; `submit_multipart.txt` and the ICAP samples are still to make). Save to `crates/ck-agent-capture/tests/fixtures/`: `logged_in.html`, `logged_out.html`, `my_submissions.html` (header and first row only, handle replaced by `tourist`), and `submit_multipart.txt` (one submit body with the source replaced by `print(1)`). Search them for `csrf`, `JSESSIONID`, `@` and the real handle before saving.

- [ ] **Step 5: Squid talks ICAP.** Run any ICAP echo you can find (or `nc -l 1344` and read what Squid sends) with a one-line `icap_service` in a test `squid.conf`, to see a real `REQMOD` and `RESPMOD` head from this Squid. Save one of each (bodies cut) as `tests/fixtures/icap_reqmod.txt` and `icap_respmod.txt`. Task 4's tests use them.

- [ ] **Step 6: Commit the notes**

```bash
git switch -c feature/codeforces-capture develop
git add docs/capture.md crates/ck-agent-capture/tests/fixtures
git commit -m "docs: record how codeforces and squid look on the wire"
```

**If any "could not verify" item turned out different, fix the later tasks first.**

---

### Task 1: Page parsers

**Files:**
- Create: `crates/ck-agent-capture/Cargo.toml`, `src/lib.rs`, `src/codeforces.rs`
- Fixtures already recorded (Task 0): `tests/fixtures/logged_in_header.html`, `logged_out_header.html`, `my_submissions_excerpt.html`

- [ ] **Step 1: Crate.** `Cargo.toml` follows `ck-agent-uplink/Cargo.toml` (same `[package]` and `[lints]`). Dependencies: `chrono`, `ck-protocol`, `serde_json` (same versions and rev), `sha2`. `src/lib.rs`: `pub mod codeforces;`. Dev-dependency: `proptest`.

- [ ] **Step 2: Tests first** in `src/codeforces.rs`, as tables (one test per behaviour, `testing.md`):
  - `who_reads_the_header`: `logged_in_header.html` is `Handle("tourist")`, `logged_out_header.html` is `Nobody`, `""`, a Cloudflare "Just a moment..." page and a JSON answer are `Unknown`, a header whose profile link is `/profile/a"><script>` is `Unknown`, a header with a profile link but no logout link is `Unknown`.
  - `a_submit_is_read_from_the_path`: `POST` paths: `/contest/2263/problem/C1` is `(2263, "C1")`, `/problemset/problem/2290/H` is `(2290, "H")` and is marked `problemset`, `/gym/100/problem/A` is `(100, "A")`, `/group/abc123/contest/7/problem/B` is `(7, "B")`; `/contest/2263/my`, `/contest/x/problem/A`, `/contest/2263/problem/` and `/` are `None`.
  - `an_accepted_submit_is_the_redirect_to_the_submissions_page`: `(302, Some("https://codeforces.com/contest/2263/my"))` is `Some(MyPage::Contest(2263))`; `(302, Some("https://codeforces.com/problemset/status"))` is `Some(MyPage::Problemset)`; `(200, None)`, `(302, Some("https://codeforces.com/enter"))` and `(302, None)` are `None`.
  - `a_submissions_page_is_recognised_and_its_first_id_read`: `my_page("/contest/2263/my", "")` is `Contest(2263)`; `my_page("/problemset/status", "my=on")` is `Problemset`, but without `my=on` it is `None`; `first_submission_id(my_submissions_excerpt.html)` is `398649329`; a page without the attribute is `None`.
  - `no_input_makes_the_parsers_panic` (`proptest`): arbitrary strings into `who`, `first_submission_id`, `submit_target`, `redirect_target` never panic.

- [ ] **Step 3: Run, expect a compile failure**

```bash
cargo test -p ck-agent-capture
```

- [ ] **Step 4: Implement** (plain code, no regex crate):

```rust
//! What a Codeforces page, request or response means. Pure functions.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Who {
    Handle(String),
    /// A page for a visitor who is not logged in.
    Nobody,
    /// Not a page that says either way (a challenge page, a data answer, a cut page).
    Unknown,
}

/// A page that lists the contestant's own submissions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MyPage {
    Contest(u64),
    Problemset,
}

/// Where a submit goes: the contest and the problem index, both from the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubmitTarget {
    pub contest_id: u64,
    pub problem_index: String,
    pub problemset: bool,
}

const HEADER_START: &str = "<div class=\"lang-chooser\">";
const HEADER_END: &str = "<br style=\"clear: both;\"/>";

/// The header says who is logged in: a profile link next to a logout link. Handles are letters,
/// digits, `_`, `-` and `.`; anything else is not trusted.
pub fn who(html: &str) -> Who {
    let Some(start) = html.find(HEADER_START) else {
        return Who::Unknown;
    };
    let rest = &html[start..];
    let end = rest.find(HEADER_END).unwrap_or(rest.len()).min(4000);
    let header = &rest[..floor_boundary(rest, end)];
    if header.contains("href=\"/enter") {
        return Who::Nobody;
    }
    let Some(link) = header.find("href=\"/profile/") else {
        return Who::Unknown;
    };
    let after = &header[link + "href=\"/profile/".len()..];
    let handle: String = after.chars().take_while(|c| *c != '"').collect();
    if is_handle(&handle) && after.contains("/logout\"") {
        Who::Handle(handle)
    } else {
        Who::Unknown
    }
}

/// The largest index at or below `at` that is a character boundary, so cutting never panics.
fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

fn is_handle(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 40
        && text.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

fn segments(path: &str) -> Vec<&str> {
    path.trim_matches('/').split('/').collect()
}

/// A submit is a `POST` to the problem's own page. The caller still checks that the form says
/// `action=submitSolutionFormSubmitted`, because other `POST`s go to the same page.
pub fn submit_target(path: &str) -> Option<SubmitTarget> {
    let (contest, index, problemset) = match segments(path).as_slice() {
        ["contest" | "gym", id, "problem", index] | ["group", _, "contest", id, "problem", index] => {
            (id.parse().ok()?, *index, false)
        }
        ["problemset", "problem", id, index] => (id.parse().ok()?, *index, true),
        _ => return None,
    };
    (!index.is_empty() && index.len() <= 4).then(|| SubmitTarget {
        contest_id: contest,
        problem_index: index.to_owned(),
        problemset,
    })
}

/// Codeforces answers an accepted submit with a redirect, as an absolute URL, to the page that
/// lists the contestant's submissions. Anything else is not an accepted submit.
pub fn redirect_target(status: u16, location: Option<&str>) -> Option<MyPage> {
    if !matches!(status, 302 | 303) {
        return None;
    }
    let path = location?.split('?').next()?;
    let path = path.split_once("://").map_or(path, |(_, rest)| rest.find('/').map_or("", |at| &rest[at..]));
    match segments(path).as_slice() {
        ["contest" | "gym", id, "my"] | ["group", _, "contest", id, "my"] => {
            id.parse().ok().map(MyPage::Contest)
        }
        ["problemset", "status"] => Some(MyPage::Problemset),
        _ => None,
    }
}

/// Is this request (path and query) a page of the contestant's own submissions?
pub fn my_page(path: &str, query: &str) -> Option<MyPage> {
    match segments(path).as_slice() {
        ["contest" | "gym", id, "my"] | ["group", _, "contest", id, "my"] => {
            id.parse().ok().map(MyPage::Contest)
        }
        ["problemset", "status"] if query.split('&').any(|pair| pair == "my=on") => {
            Some(MyPage::Problemset)
        }
        _ => None,
    }
}

/// The id of the newest submission on a submissions page, which lists the newest first.
pub fn first_submission_id(html: &str) -> Option<u64> {
    let marker = "data-submission-id=\"";
    let start = html.find(marker)? + marker.len();
    let digits: String = html[start..].chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}
```

  If a recorded real page disagrees with a test, the page wins: fix the parser, not the fixture.

- [ ] **Step 5: Run and commit**

```bash
cargo test -p ck-agent-capture
git add crates/ck-agent-capture
git commit -m "feat: read codeforces pages and submits"
```

---

### Task 2: The tracker

**Files:**
- Create: `crates/ck-agent-capture/src/tracker.rs`; modify `src/lib.rs`

Turns what the service saw into observations. Time is passed in. One contestant per stick, so one current handle.

- [ ] **Step 1: One table-driven test per behaviour:**
  - `a_login_is_one_event_per_change_of_handle`: `saw_page(Handle a)` → one `Login`; again → none; `Handle b` → one; `Nobody` → none and forgets the handle.
  - `a_submit_waits_for_its_id_then_is_sent`: handle known; `request_seen(contest 1900, "B", sha, t0)` → none; `answer_seen(true)` → none (now waiting for the id); `saw_my_page(Contest(1900), Some(77))` → one `Submit` with `submission_id: Some(77)`, `at: t0`, the sha.
  - `a_submit_without_an_id_is_sent_after_the_wait`: `tick(t0 + 9s)` → none; `tick(t0 + 11s)` → one with `submission_id: None`.
  - `a_refused_submit_is_not_an_event`: `request_seen` then `answer_seen(false)` then a long `tick` → none.
  - `only_the_matching_my_page_completes`: a submissions page of another contest changes nothing, and a problemset submit is completed by `Problemset` only; a second accepted submit sends the first without an id.
  - `a_submit_with_no_known_handle_is_dropped` (returns none, does not panic).

- [ ] **Step 2: Implement**

```rust
//! Works out what a contestant did from what the capture service saw.

use chrono::{DateTime, TimeDelta, Utc};

use crate::codeforces::{MyPage, SubmitTarget, Who};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    Login { handle: String, at: DateTime<Utc> },
    Submit {
        handle: String,
        contest_id: u64,
        problem_index: String,
        source_sha256: String,
        submission_id: Option<u64>,
        at: DateTime<Utc>,
    },
}

struct Submit {
    handle: String,
    /// Where Codeforces will list it: `MyPage::Contest(id)` or `MyPage::Problemset`.
    lists_on: MyPage,
    contest_id: u64,
    problem_index: String,
    source_sha256: String,
    at: DateTime<Utc>,
}

impl Submit {
    fn finish(self, submission_id: Option<u64>) -> Observation {
        Observation::Submit {
            handle: self.handle,
            contest_id: self.contest_id,
            problem_index: self.problem_index,
            source_sha256: self.source_sha256,
            submission_id,
            at: self.at,
        }
    }
}

pub struct Tracker {
    handle: Option<String>,
    /// A submit request that Codeforces has not answered yet.
    asked: Option<Submit>,
    /// An accepted submit that waits for its id.
    waiting: Option<Submit>,
    wait_for_id: TimeDelta,
}

impl Tracker {
    #[must_use]
    pub fn new(wait_for_id: TimeDelta) -> Self {
        Self { handle: None, asked: None, waiting: None, wait_for_id }
    }

    /// A page said who is logged in. A different handle than before is a login.
    pub fn saw_page(&mut self, who: Who, now: DateTime<Utc>) -> Vec<Observation> {
        match who {
            Who::Handle(handle) if self.handle.as_ref() != Some(&handle) => {
                self.handle = Some(handle.clone());
                vec![Observation::Login { handle, at: now }]
            }
            Who::Nobody => {
                self.handle = None;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// A submit request went by. It only counts once Codeforces accepts it. Without a known handle
    /// the Hub could not use the event, so it is dropped and the caller logs that.
    pub fn request_seen(&mut self, target: &SubmitTarget, source_sha256: &str, now: DateTime<Utc>) {
        self.asked = self.handle.clone().map(|handle| Submit {
            handle,
            lists_on: if target.problemset { MyPage::Problemset } else { MyPage::Contest(target.contest_id) },
            contest_id: target.contest_id,
            problem_index: target.problem_index.clone(),
            source_sha256: source_sha256.to_owned(),
            at: now,
        });
    }

    /// Codeforces answered the submit request. Returns the earlier submit that never got its id,
    /// if this one pushes it out.
    pub fn answer_seen(&mut self, accepted: bool) -> Vec<Observation> {
        let Some(asked) = self.asked.take() else { return Vec::new() };
        if !accepted {
            return Vec::new();
        }
        let earlier = self.waiting.replace(asked).map(|s| s.finish(None));
        earlier.into_iter().collect()
    }

    /// A submissions page with the id of its newest submission.
    pub fn saw_my_page(&mut self, page: MyPage, id: Option<u64>) -> Vec<Observation> {
        let Some(id) = id else { return Vec::new() };
        match self.waiting.take() {
            Some(waiting) if waiting.lists_on == page => vec![waiting.finish(Some(id))],
            other => {
                self.waiting = other;
                Vec::new()
            }
        }
    }

    /// Sends a submit that waited long enough for its id.
    pub fn tick(&mut self, now: DateTime<Utc>) -> Vec<Observation> {
        match self.waiting.take() {
            Some(waiting) if now - waiting.at >= self.wait_for_id => vec![waiting.finish(None)],
            other => {
                self.waiting = other;
                Vec::new()
            }
        }
    }
}
```

- [ ] **Step 3: Run and commit**

```bash
cargo test -p ck-agent-capture tracker
git add crates/ck-agent-capture
git commit -m "feat: track codeforces logins and submits"
```

---

### Task 3: Events

**Files:**
- Create: `crates/ck-agent-capture/src/event.rs`; modify `src/lib.rs`, `Cargo.toml` (`ck-agent-uplink` by path)

- [ ] **Step 1: Tests first** (one table test, plus one queue test): the payload of a login has `judge` and `handle` only; of a submit has `judge`, `handle`, `contest_id`, `problem_index`, `source_sha256`, and `submission_id` only when known (key left out, never `null`, as `api.md` requires). The queue test pushes into a `tempfile` `Queue` and reads it back with `pending()`: kind `Submission`, `sent_at` equal to the observation's `at`, and **the serialized file contains no source code, password or token** (use a sentinel source `SECRET-SOURCE` in the observation's fixture and assert only its hash appears).

- [ ] **Step 2: Implement**

```rust
//! What goes to the Hub. These payload shapes are the contract with ck-hub.

use ck_agent_uplink::{Error, Queue};
use ck_protocol::{EventKind, StickId};
use serde_json::{Value, json};

use crate::tracker::Observation;

#[must_use]
pub fn payload(observation: &Observation) -> (EventKind, Value) {
    match observation {
        Observation::Login { handle, .. } => {
            (EventKind::Login, json!({ "judge": "codeforces", "handle": handle }))
        }
        Observation::Submit { handle, contest_id, problem_index, source_sha256, submission_id, .. } => {
            let mut payload = json!({
                "judge": "codeforces",
                "handle": handle,
                "contest_id": contest_id,
                "problem_index": problem_index,
                "source_sha256": source_sha256,
            });
            if let Some(id) = submission_id {
                payload["submission_id"] = json!(id);
            }
            (EventKind::Submission, payload)
        }
    }
}

/// Queues the observation as an event of this stick and boot, at the time it happened.
pub fn queue(queue: &Queue, stick: &StickId, boot: &str, observation: &Observation) -> Result<(), Error> {
    let (kind, payload) = payload(observation);
    let at = match observation {
        Observation::Login { at, .. } | Observation::Submit { at, .. } => *at,
    };
    queue.push(stick, boot, kind, payload, at).map(|_| ())
}
```

- [ ] **Step 3: Run and commit**

```bash
cargo test -p ck-agent-capture event
git add crates/ck-agent-capture
git commit -m "feat: turn codeforces observations into events"
```

---

### Task 4: The ICAP reader

**Files:**
- Create: `crates/ck-agent-capture/src/icap.rs`; modify `src/lib.rs`

A small, strict reader for the part of RFC 3507 Squid uses, not a general ICAP server. Everything from the network is untrusted and size-limited (`security.md`).

- [ ] **Step 1: Tests first**, using the saved Squid fixtures: `reads_a_reqmod_with_a_body` (method, request head, decoded body), `reads_a_respmod_with_both_heads_and_a_body`, `a_body_over_the_limit_is_cut_and_marked` (limit 1 MiB for requests, 2 MiB for responses: the rest is read and thrown away so the connection stays usable, and `truncated` is set), `a_malformed_request_is_an_error_not_a_panic`, `options_says_no_preview_and_allows_204`, and a `proptest` that feeds arbitrary bytes to `read_request` and only requires it to return without panicking or reading more than the limits.

- [ ] **Step 2: Implement** the pieces below; check each against the fixtures:

```rust
//! The ICAP messages Squid sends (RFC 3507): `OPTIONS`, `REQMOD` and `RESPMOD`.

use std::io::{BufRead, Write};

/// Limits on what one message may hold. A bigger body is read and dropped.
pub const MAX_HEAD_BYTES: usize = 64 * 1024;

#[derive(Debug, PartialEq, Eq)]
pub enum Method {
    Options,
    Reqmod,
    Respmod,
}

#[derive(Debug)]
pub struct Message {
    pub method: Method,
    /// The HTTP request line and headers Squid forwarded (REQMOD and RESPMOD), as text.
    pub request_head: Option<String>,
    /// The HTTP status line and headers of the answer (RESPMOD).
    pub response_head: Option<String>,
    pub body: Vec<u8>,
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the ICAP message is not valid: {0}")]
    Bad(&'static str),
    #[error("cannot read the ICAP message: {0}")]
    Io(#[from] std::io::Error),
}

pub fn read_message(input: &mut impl BufRead, body_limit: usize) -> Result<Message, Error> { /* see below */ }
```

  Behavior of `read_message`: read the ICAP request line and headers (stop at the blank line; at most `MAX_HEAD_BYTES` in total); take the method from the first word (anything but the three is `Bad`); parse `Encapsulated:` into offsets (`req-hdr`, `res-hdr`, `req-body`, `res-body`, `null-body`, `opt-body`); read the HTTP heads as the byte ranges between consecutive offsets; if a `*-body` section follows, read it as chunked transfer (hex size, optional `;ext`, CRLF, data, CRLF, ending with a zero chunk and CRLF, treating `; ieof` as a normal end), appending up to `body_limit` bytes and discarding the rest while setting `truncated`. Never allocate from a number the peer sent before checking it against a limit.

```rust
/// `OPTIONS`: no preview (so Squid sends whole bodies), 204 allowed, and the service's tag.
pub fn options_answer(service: &str) -> Vec<u8> {
    let text = format!(
        "ICAP/1.0 200 OK\r\nMethods: REQMOD, RESPMOD\r\nService: {service}\r\n\
         ISTag: \"ck-capture-1\"\r\nAllow: 204\r\nTransfer-Complete: *\r\n\
         Max-Connections: 8\r\nOptions-TTL: 3600\r\nEncapsulated: null-body=0\r\n\r\n"
    );
    text.into_bytes()
}

/// "Use the message as it was": nothing is changed or delayed.
pub fn no_change() -> &'static [u8] {
    b"ICAP/1.0 204 No Content\r\nISTag: \"ck-capture-1\"\r\nEncapsulated: null-body=0\r\n\r\n"
}

pub fn write_answer(out: &mut impl Write, answer: &[u8]) -> std::io::Result<()> {
    out.write_all(answer)?;
    out.flush()
}
```

  If Task 0 shows Squid does not send `Allow: 204` for a message, the service must echo the original message with a `200` instead; add that then, with a test.

- [ ] **Step 3: Run and commit**

```bash
cargo test -p ck-agent-capture icap
git add crates/ck-agent-capture
git commit -m "feat: read the icap messages squid sends"
```

---

### Task 5: The service and `ck-agent capture`

**Files:**
- Create: `crates/ck-agent-capture/src/service.rs`, `tests/service.rs`; modify `src/lib.rs`, `Cargo.toml` (`multer`, `tokio` with `rt` and `macros` only if `multer`'s async API needs it)
- Modify: `crates/ck-agent/Cargo.toml`, `crates/ck-agent/src/main.rs`; test `crates/ck-agent/tests/capture.rs`

- [ ] **Step 1: The decision logic as one pure function, tested first.**

```rust
/// What one ICAP message from Squid means for the tracker.
pub fn handle(tracker: &mut Tracker, sink: &mut impl FnMut(Observation), message: &Message, now: DateTime<Utc>)
```

  Table test with fixtures (request heads and bodies built from `submit_multipart.txt`, `logged_in.html`, `my_submissions.html`): a `REQMOD` of `POST /contest/1900/problem/B` with `action=submitSolutionFormSubmitted` calls `request_seen` with contest 1900, problem `B` and the SHA-256 of the fixture's source (`print(1)`, once in `source` and once in `sourceFile`); a `RESPMOD` whose request head is that submit and whose response head is `302` with `Location: https://codeforces.com/contest/1900/my` calls `answer_seen(true)`; a `RESPMOD` of a text/html page calls `saw_page`; one for `GET /contest/1900/my` also calls `saw_my_page` with the fixture's id; a `RESPMOD` for any other host or a body marked `truncated` changes nothing. **A test that the secrets never leave `handle`:** feed a submit with `csrf_token=SECRET-TOKEN` in the form and in the query, and assert that no `Observation` and no `tracing` output (capture with `tracing-subscriber`'s test writer) contains `SECRET-TOKEN` or the source text.

  Rules for the implementation: look only at the request line, the `Host`, `Content-Type`, `Location` and the body. A request is a submit when it is a `POST`, `submit_target(path)` matches and the multipart form has `action=submitSolutionFormSubmitted` (read with `multer`); the problem comes from the path. The source is the form field `source` if it is not empty, otherwise the file `sourceFile` (the recorded submits used `sourceFile`): hash it with `sha2`, never store it. The answer counts when `redirect_target(status, Location)` is `Some`. A `RESPMOD` of a `GET` for which `my_page(path, query)` is `Some` also gets `first_submission_id`. Ignore the Cloudflare challenge exchange (a `POST /enter` with 64-character hex field names, and 403 pages): they match nothing. Log nothing from the request. The handle is logged only at `debug` level.

- [ ] **Step 2: The server loop.** `run(listener, queue, stick, boot)`: accept connections on `127.0.0.1:1344` (one thread each, at most 8), loop reading messages with `read_message` (answers `OPTIONS` with `options_answer`, everything else with `no_change`, **always, even on an error inside `handle`**, so a bug in capture never blocks a page), feed `handle`, and sink observations through `event::queue` (an error is logged without the payload). A second thread calls `tracker.tick(Utc::now())` each second. The tracker is behind a `Mutex`, shared by the connection threads.

- [ ] **Step 3: Integration test** `tests/service.rs`: start the service on port 0 with a temp `Queue`, write the saved Squid fixtures plus a made-up login → submit → my-submissions sequence to the socket, and assert the answers are `204` and the queue holds one `Login` and one `Submission` with the right handle, contest, problem, id and `source_sha256`. A second case: `RESPMOD` after the service's tracker was fed nothing yields no events and still `204`.

- [ ] **Step 4: Subcommand.** In `ck-agent` add `Capture { root, listen }` (default listen `127.0.0.1:1344`) like `Uplink`: `Uplink::load(root)` for the identity (a stick with no settings exits with `EXIT_NO_SETTINGS`), `Queue::open(root.join(QUEUE_FOLDER))`, run until SIGTERM. Test in `tests/capture.rs` like `tests/uplink.rs`: start it with `--listen 127.0.0.1:0 --ready-file <path>` (hidden, for tests), wait for the address in the file, send an `OPTIONS` and read the `200`, then SIGTERM and a clean exit.

- [ ] **Step 5: Run and commit**

```bash
cargo test
git add crates
git commit -m "feat: run the codeforces capture service from ck-agent"
```

---

### Task 6: The CA, Squid and the image

**Files:**
- Create: `crates/ck-agent-capture/src/authority.rs`; modify `crates/ck-agent/src/main.rs` (`capture-ca` subcommand)
- Create: `image/patch/rootfs/etc/systemd/system/ck-capture.service`, `ck-capture-ca.service`, `image/patch/rootfs/etc/squid/conf.d/ck-capture.conf`
- Modify: `image/patch/apply-to-image.sh`, `tests/test_image_patch.py`, `crates/ck-agent-config/src/branding.rs`, its tests, `docs/testing.md`

- [ ] **Step 1: The CA.** Tests first (one table test): `load_or_create(dir)` twice leaves the certificate byte-identical; the key file is `0o640` owned `root:proxy` when it can be (mode checked, owner only on the stick), the certificate `0o644`; the certificate is a CA with a name constraint permitting only `codeforces.com` (parse with `x509-parser` as a dev-dependency); a damaged key is an `Err`, never a silently new CA (the browser would distrust it with no explanation). Implement with `rcgen`: `IsCa::Ca(BasicConstraints::Constrained(0))`, `key_usages = [KeyCertSign, CrlSign]`, `name_constraints` with `GeneralSubtree::DnsName("codeforces.com")`, five years. Create the key file with `create_new` and mode set at creation. If the `rcgen` in use lacks name constraints, stop and tell the user: the CA must not be able to sign for other sites. Subcommand `ck-agent capture-ca --dir /var/lib/ck/capture`.

- [ ] **Step 2: Units.** `ck-capture-ca.service` is a oneshot that runs `ck-agent capture-ca`, `Before=squid.service display-manager.service`, `After=ck-config.service`, `StateDirectory=ck/capture`, `StateDirectoryMode=0755`. `ck-capture.service` runs `ck-agent capture` (`After=ck-config.service`, `Before=squid.service`, `Restart=always`, `RestartSec=2`, the same hardening as `ck-uplink.service`, `ReadWritePaths=/var/lib/ck/queue`). The queue folder is created `0700` by `ck-uplink.service`; make sure `Queue::open` keeps it `0700` if `ck-capture` happens to start first.

- [ ] **Step 3: Squid.** `ck-capture.conf` (adjust to the image's Squid and the facts of Task 0, and check `squid.conf` includes `conf.d`; otherwise append the lines to a copy of `squid.conf` in the patch):

```
# Decrypt codeforces.com only. Every other site is tunnelled as before, with its own certificate.
http_port 3128 ssl-bump cert=/var/lib/ck/capture/ca.pem key=/var/lib/ck/capture/ca-key.pem generate-host-certificates=on dynamic_cert_mem_cache_size=4MB
sslcrtd_program /usr/lib/squid/security_file_certgen -s /var/lib/squid/ssl_db -M 4MB
acl ck_codeforces ssl::server_name .codeforces.com
acl ck_step1 at_step SslBump1
ssl_bump peek ck_step1
ssl_bump bump ck_codeforces
ssl_bump splice all

# Ask Codeforces for plain pages, so the service does not have to decompress.
request_header_access Accept-Encoding deny ck_codeforces
request_header_add Accept-Encoding identity ck_codeforces

icap_enable on
icap_service ck_req reqmod_precache icap://127.0.0.1:1344/capture bypass=off
icap_service ck_resp respmod_precache icap://127.0.0.1:1344/capture bypass=off
adaptation_access ck_req allow ck_codeforces
adaptation_access ck_resp allow ck_codeforces
adaptation_access ck_req deny all
adaptation_access ck_resp deny all
```

  `bypass=off` is the recommended answer to the decision above; change it to `on` if the user chooses so. The `http_port 3128` line already exists in the image's `squid.conf`: replace it there instead of duplicating it if Task 0 shows that. Squid's database folder for generated certificates must exist and belong to `proxy`: create it with `security_file_certgen -c -s /var/lib/squid/ssl_db -M 4MB` in a `ExecStartPre` of a drop-in for `squid.service`.

- [ ] **Step 3b: Netlock.** With route A nothing changes in `netlock.nft`: the contestant still reaches only Squid on 3128, and `root` (the capture service and Squid's own ICAP connection are on `lo` anyway) is accepted. Confirm with the `docs/testing.md` network checks.

- [ ] **Step 4: Firefox trusts the CA.** In `apply_firefox` (`branding.rs`) keep every key already in `policies.json` and set `policies.Certificates.Install` to `["/var/lib/ck/capture/ca.pem"]`. Test in `crates/ck-agent-config/tests/branding.rs` (one table test): the key is added, existing keys such as bookmarks survive, running twice gives the same file.

- [ ] **Step 5: Patch script and tests.** Read `apply-to-image.sh` and enable and copy the two units and the Squid file wherever `ck-uplink.service` is handled; mirror its assertions in `tests/test_image_patch.py`.

```bash
python3 -m pytest tests/test_image_patch.py -q
shellcheck image/patch/apply-to-image.sh
cargo test
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat: decrypt codeforces in squid and hand it to the capture service"
```

---

### Task 7: Test on a VM

**Files:**
- Modify: `docs/testing.md`, `docs/capture.md`

Needs the contest image; runs locally like the other VM tests. Add the checks to `ck-vmtest` only if the existing pattern makes that easy, otherwise as written steps.

- [ ] **Step 1:** Patch a copy of the image (`docs/base-images.md`), boot it, and with a throwaway Codeforces account typed by the tester: log in, open a contest, submit.
- [ ] **Step 2: On the stick.** `ck-capture` and `squid` are active; `ck-capture-ca.service` ran; Firefox shows no warning for `codeforces.com`; **another site (for example `example.org`) shows its real certificate, not ours**; presenting a certificate from this CA for any other name is refused (name constraints).
- [ ] **Step 3: At the Hub.** Run `ck-agent uplink --once` against a local Hub from ck-hub#34 (in memory) and check it holds one `Login` and one `Submission` with the right handle, contest, problem and `source_sha256`. Search the stick's queue folder, `journalctl` and the Hub's data for the password, the CSRF token and a line of the submitted code: none may appear.
- [ ] **Step 4:** Stop `ck-capture` with Codeforces open and write down what the contestant sees (it should match the `bypass` decision), and that other sites still work. Start it again and check the next submit is captured.
- [ ] **Step 5:** Write the results in `docs/testing.md`, commit, and open the PR into `develop`.

```bash
git add docs
git commit -m "docs: test the codeforces capture on a stick"
```

---

## Appendix: route B (only if Squid cannot decrypt TLS)

Replace Tasks 4 to 6 with: a `hudsucker` proxy in `ck-agent capture` listening on `127.0.0.1:3129` that decrypts `codeforces.com` only (`should_intercept`), tunnels everything else untouched, and forwards through Squid at `127.0.0.1:3128` (so the allowlist still applies: if that chaining cannot be done, stop rather than connect to the internet directly). It reuses Tasks 1 to 3 and the CA of Task 6 unchanged (as a `rcgen` `RcgenAuthority`), but the handler must tie each response to its request itself (HTTP/1.1 only towards the browser, state per connection), buffer and decompress HTML (`hudsucker::decode_response`), and be tested over plain HTTP with a watched host list. Changes outside the crate: Firefox's `Proxy` policy to `127.0.0.1:3129` locked, and `netlock.nft` letting non-root users reach 3129 instead of 3128 so nobody can submit around the proxy. That last change makes the capture service the only way to the web, so a crash of it takes the whole contestant's browsing down; ask the user before doing it.

## After this plan

1. **`ck-protocol`:** add the `Login` and `Submission` payload types (conventions: event types live only there), check them against capture rules B.3/D.2, then make `ck-hub` read them instead of its own parser (small follow-up to ck-hub#34).
2. **Clock:** the Hub matches within ±10 s of the judge's time, so the stick's clock must be right; have `ck-selftest` report drift.
3. **Kilonova and vJudge:** only the parsers in `codeforces.rs` and the Squid host lists differ; extract a trait when the second judge exists, not before.
4. **Fingerprints at the Hub:** `source_sha256` is stored but not used yet; a later feature can flag the same source sent from two teams.

## Self-review notes

- Covered: capture of login and submit, no credential or source kept (hash instead), best-effort submission id, only `codeforces.com` decrypted, allowlist untouched, events through the existing queue, the image units and Squid config, browser trust, the fail-open or fail-closed decision.
- Not verified and said so: Squid's TLS and ICAP behaviour on the image, the real Codeforces markup, Cloudflare acceptance.
- Soft spots: `rcgen` name constraints and the `multer` calls are from docs, not compiled; the ICAP reader is written to the RFC and must match what Squid really sends (Task 0 step 5 saves the samples).
