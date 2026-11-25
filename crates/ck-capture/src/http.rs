//! The first lines of an HTTP request or response, as Squid hands them over.

/// A request or a response head. A request has a method and a target; a response has a status.
#[derive(Debug, Default, Clone)]
pub struct Head {
    pub method: String,
    pub target: String,
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

impl Head {
    /// The value of the first header with this name, in any letter case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The server name without a port, in lower case. A proxy request names it in the target; a
    /// request inside a tunnel only in the `Host` header.
    pub fn host(&self) -> String {
        let authority = match self.target.split_once("://") {
            Some((_, rest)) => rest.split(['/', '?']).next().unwrap_or(""),
            None => self.header("host").unwrap_or(""),
        };
        let authority = authority.rsplit('@').next().unwrap_or("");
        let host = match authority.rsplit_once(':') {
            Some((host, port)) if port.bytes().all(|b| b.is_ascii_digit()) => host,
            _ => authority,
        };
        // `codeforces.com.` is the same name; the rule compares without the dot.
        host.trim_end_matches('.').to_ascii_lowercase()
    }

    /// The path and the query, without the server.
    fn path_and_query(&self) -> &str {
        match self.target.split_once("://") {
            Some((_, rest)) => rest.find(['/', '?']).map_or("/", |at| &rest[at..]),
            None => &self.target,
        }
    }

    pub fn path(&self) -> &str {
        self.path_and_query()
            .split(['?', '#'])
            .next()
            .unwrap_or("/")
    }

    pub fn query(&self) -> &str {
        self.path_and_query()
            .split_once('?')
            .map_or("", |(_, query)| query.split('#').next().unwrap_or(""))
    }
}

fn parse_headers(lines: std::str::Lines<'_>) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    headers
}

pub fn parse_request_head(bytes: &[u8]) -> Option<Head> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines();
    let mut first = lines.next()?.split_whitespace();
    let method = first.next()?.to_ascii_uppercase();
    let target = first.next()?.to_string();
    Some(Head {
        method,
        target,
        status: 0,
        headers: parse_headers(lines),
    })
}

pub fn parse_response_head(bytes: &[u8]) -> Option<Head> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines = text.lines();
    let mut first = lines.next()?.split_whitespace();
    first
        .next()
        .filter(|version| version.starts_with("HTTP/"))?;
    let status = first.next()?.parse().ok()?;
    Some(Head {
        status,
        headers: parse_headers(lines),
        ..Head::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_request_sent_to_a_proxy() {
        let head = parse_request_head(
            b"POST https://Example.org:443/enter?a=1&b=2#x HTTP/1.1\r\nContent-Type: text/plain\r\n\r\n",
        )
        .unwrap();
        assert_eq!(head.method, "POST");
        assert_eq!(head.host(), "example.org");
        assert_eq!(head.path(), "/enter");
        assert_eq!(head.query(), "a=1&b=2");
        assert_eq!(head.header("content-type"), Some("text/plain"));
    }

    #[test]
    fn a_trailing_dot_does_not_change_the_host() {
        let head =
            parse_request_head(b"GET https://Codeforces.com.:443/a HTTP/1.1\r\n\r\n").unwrap();
        assert_eq!(head.host(), "codeforces.com");
    }

    #[test]
    fn reads_a_request_inside_a_tunnel_from_the_host_header() {
        let head = parse_request_head(b"GET /a/b HTTP/1.1\r\nHost: Example.org\r\n\r\n").unwrap();
        assert_eq!(head.host(), "example.org");
        assert_eq!(head.path(), "/a/b");
        assert_eq!(head.query(), "");
    }

    #[test]
    fn reads_a_response() {
        let head = parse_response_head(b"HTTP/1.1 302 Found\r\nLocation: /\r\n\r\n").unwrap();
        assert_eq!(head.status, 302);
        assert_eq!(head.header("LOCATION"), Some("/"));
        assert!(parse_response_head(b"garbage").is_none());
        assert!(parse_request_head(b"").is_none());
    }
}
