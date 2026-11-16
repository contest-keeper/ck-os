//! The observation rule: writes what the traffic looks like, never what it says.
//!
//! This is how a platform's login and submission are learned before a rule for it is written
//! (plan B.1 and D.1). Each line names the method, host, path, status, header names, cookie names and
//! form or JSON field names. The only header values that are written are the media type, the length
//! and the path of a redirect, none of which is a secret. A query string and every field value are
//! dropped, so a password, a token or a session id cannot reach the output.

use serde_json::{Value, json};

use crate::fields::{field_names, urlencoded};
use crate::{Head, Output, Request, Response, Rule};

/// Files a page loads for its looks. Their traffic says nothing about a login or a submission.
const STATIC_EXTENSIONS: [&str; 12] = [
    ".png", ".jpg", ".jpeg", ".gif", ".svg", ".ico", ".css", ".js", ".woff", ".woff2", ".ttf",
    ".map",
];
const STATIC_TYPES: [&str; 5] = [
    "image/",
    "font/",
    "text/css",
    "text/javascript",
    "application/javascript",
];

#[derive(Default)]
pub struct Observer;

fn header_names(head: &Head) -> Vec<&str> {
    let mut names: Vec<&str> = Vec::new();
    for (name, _) in &head.headers {
        if !names.iter().any(|seen| seen.eq_ignore_ascii_case(name)) {
            names.push(name);
        }
    }
    names
}

/// The names in `Cookie: a=1; b=2` or the name in each `Set-Cookie: a=1; Path=/`.
fn cookie_names(head: &Head, header: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (name, value) in &head.headers {
        if !name.eq_ignore_ascii_case(header) {
            continue;
        }
        let parts: Vec<&str> = if header.eq_ignore_ascii_case("cookie") {
            value.split(';').collect()
        } else {
            vec![value.split(';').next().unwrap_or("")]
        };
        for part in parts {
            if let Some((cookie, _)) = part.split_once('=') {
                names.push(cookie.trim().chars().take(64).collect::<String>());
            }
        }
    }
    names
}

/// The server and path a redirect leads to, without the query.
fn redirect_target(location: &str) -> String {
    location.split(['?', '#']).next().unwrap_or("").to_string()
}

/// The first few user names a page links to as `href="/profile/NAME"`. A page for a logged-in user
/// shows their own name in its header, which is how a login rule can learn who it is. A name is
/// public; it is not a credential.
fn profile_links(content_type: &str, body: &[u8]) -> Vec<String> {
    const MARK: &str = "href=\"/profile/";
    if !content_type.to_ascii_lowercase().starts_with("text/html") {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(body);
    let mut names: Vec<String> = Vec::new();
    for part in text.split(MARK).skip(1) {
        let name: String = part
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
            .take(32)
            .collect();
        if !name.is_empty() && !names.contains(&name) {
            names.push(name);
        }
        if names.len() == 3 {
            break;
        }
    }
    names
}

fn is_static_type(content_type: &str) -> bool {
    let lower = content_type.to_ascii_lowercase();
    STATIC_TYPES.iter().any(|prefix| lower.starts_with(prefix))
}

impl Rule for Observer {
    fn on_request(&mut self, request: &Request) -> Vec<Output> {
        let head = &request.head;
        let path = head.path();
        let get = matches!(head.method.as_str(), "GET" | "HEAD");
        let lower = path.to_ascii_lowercase();
        if get && STATIC_EXTENSIONS.iter().any(|ext| lower.ends_with(ext)) {
            return Vec::new();
        }
        let content_type = head.header("content-type").unwrap_or("");
        let line: Value = json!({
            "event": "request",
            "method": head.method,
            "host": head.host(),
            "path": path,
            "query_names": urlencoded(head.query()),
            "header_names": header_names(head),
            "cookie_names": cookie_names(head, "cookie"),
            "content_type": content_type,
            "body_bytes": request.body.len(),
            "body_cut": request.body_truncated,
            "fields": field_names(content_type, &request.body),
        });
        vec![Output::Note(line.to_string())]
    }

    fn on_response(&mut self, request: &Head, response: &Response) -> Vec<Output> {
        let head = &response.head;
        let content_type = head.header("content-type").unwrap_or("");
        if is_static_type(content_type) {
            return Vec::new();
        }
        let line: Value = json!({
            "event": "response",
            "method": request.method,
            "host": request.host(),
            "path": request.path(),
            "status": response.status,
            "header_names": header_names(head),
            "set_cookie_names": cookie_names(head, "set-cookie"),
            "content_type": content_type,
            "redirect_to": head.header("location").map(redirect_target),
            "body_bytes": response.body.len(),
            "body_cut": response.body_truncated,
            "fields": field_names(content_type, &response.body),
            "profile_links": profile_links(content_type, &response.body),
        });
        vec![Output::Note(line.to_string())]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parse_request_head, parse_response_head};

    fn notes(outputs: Vec<Output>) -> Vec<String> {
        outputs
            .into_iter()
            .filter_map(|output| match output {
                Output::Note(line) => Some(line),
                Output::Event { .. } => None,
            })
            .collect()
    }

    const PASSWORD: &str = "hunter2-pass";
    const TOKEN: &str = "tok3n-value-xyz";
    const SESSION: &str = "sess1on-id-123";
    const SECRETS: [&str; 5] = [PASSWORD, TOKEN, SESSION, "ana@example.org", "int main"];

    fn assert_no_secret(lines: &[String]) {
        assert_ne!(lines.len(), 0);
        for line in lines {
            for secret in SECRETS {
                assert!(!line.contains(secret), "{secret} leaked into: {line}");
            }
        }
    }

    fn login_request(content_type: &str, body: &str) -> Request {
        let text = format!(
            "POST https://codeforces.com/enter?csrf_token={TOKEN}&x=1 HTTP/1.1\r\n\
             Content-Type: {content_type}\r\nCookie: JSESSIONID={SESSION}; lang=en\r\n\
             Authorization: Bearer {TOKEN}\r\n\r\n"
        );
        Request {
            head: parse_request_head(text.as_bytes()).unwrap(),
            body: body.as_bytes().to_vec(),
            body_truncated: false,
        }
    }

    #[test]
    fn a_form_login_shows_names_and_no_values() {
        let body = format!(
            "csrf_token={TOKEN}&action=enter&handleOrEmail=ana%40example.org&password={PASSWORD}"
        );
        let request = login_request("application/x-www-form-urlencoded", &body);
        let lines = notes(Observer.on_request(&request));
        assert_no_secret(&lines);
        let seen: Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(seen["host"], "codeforces.com");
        assert_eq!(seen["path"], "/enter");
        assert_eq!(seen["query_names"], json!(["csrf_token", "x"]));
        assert_eq!(seen["cookie_names"], json!(["JSESSIONID", "lang"]));
        assert_eq!(
            seen["fields"],
            json!(["csrf_token", "action", "handleOrEmail", "password"])
        );
        assert!(seen["header_names"].to_string().contains("Authorization"));
    }

    #[test]
    fn json_and_multipart_bodies_show_no_values() {
        let json_body = format!(r#"{{"user":"ana@example.org","password":"{PASSWORD}"}}"#);
        assert_no_secret(&notes(
            Observer.on_request(&login_request("application/json", &json_body)),
        ));

        let multipart = format!(
            "--b\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\n{TOKEN}\r\n\
             --b\r\nContent-Disposition: form-data; name=\"source\"; filename=\"a.cpp\"\r\n\r\n\
             int main() {{ return 0; }}\r\n--b--\r\n"
        );
        let lines = notes(Observer.on_request(&login_request(
            "multipart/form-data; boundary=b",
            &multipart,
        )));
        assert_no_secret(&lines);
        assert!(lines[0].contains(r#""fields":["csrf","source"]"#));
    }

    #[test]
    fn a_response_shows_status_cookie_names_and_the_redirect_path_only() {
        let request = login_request("application/x-www-form-urlencoded", "a=b").head;
        let text = format!(
            "HTTP/1.1 302 Found\r\nLocation: /profile/ana?token={TOKEN}\r\n\
             Set-Cookie: JSESSIONID={SESSION}; Path=/; HttpOnly\r\n\
             Set-Cookie: 39ce7={TOKEN}; Path=/\r\nContent-Type: text/html\r\n\r\n"
        );
        let response = Response {
            status: 302,
            head: parse_response_head(text.as_bytes()).unwrap(),
            body: format!("hello ana@example.org {PASSWORD}").into_bytes(),
            body_truncated: false,
        };
        let lines = notes(Observer.on_response(&request, &response));
        assert_no_secret(&lines);
        let seen: Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(seen["status"], 302);
        assert_eq!(seen["redirect_to"], "/profile/ana");
        assert_eq!(seen["set_cookie_names"], json!(["JSESSIONID", "39ce7"]));
        assert_eq!(seen["fields"], json!([]));
    }

    #[test]
    fn a_page_shows_the_user_names_it_links_to_and_nothing_else_of_its_body() {
        let request = login_request("application/x-www-form-urlencoded", "a=b").head;
        let body = format!(
            "<a href=\"/profile/ana\">ana</a> <a href=\"/profile/ana\">again</a>              <a href=\"/profile/bob_2\">bob</a> token={TOKEN}"
        );
        let response = Response {
            status: 200,
            head: parse_response_head(
                b"HTTP/1.1 200 OK
Content-Type: text/html; charset=UTF-8

",
            )
            .unwrap(),
            body: body.into_bytes(),
            body_truncated: false,
        };
        let lines = notes(Observer.on_response(&request, &response));
        assert_no_secret(&lines);
        let seen: Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(seen["profile_links"], json!(["ana", "bob_2"]));
    }

    #[test]
    fn pictures_and_styles_are_skipped() {
        let request = Request {
            head: parse_request_head(b"GET https://codeforces.com/s/logo.PNG HTTP/1.1\r\n\r\n")
                .unwrap(),
            body: Vec::new(),
            body_truncated: false,
        };
        assert_eq!(Observer.on_request(&request), Vec::<Output>::new());
        let response = Response {
            status: 200,
            head: parse_response_head(b"HTTP/1.1 200 OK\r\nContent-Type: text/css\r\n\r\n")
                .unwrap(),
            body: Vec::new(),
            body_truncated: false,
        };
        assert_eq!(
            Observer.on_response(&request.head, &response),
            Vec::<Output>::new()
        );
    }
}
