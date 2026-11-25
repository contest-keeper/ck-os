//! Picks a few named, non-secret values out of a form body.
//!
//! `fields.rs` never reads a value. This module reads only the fields a rule asks for by name, so a
//! password in the same form is never copied anywhere.

/// The values of the fields called one of `wanted`, as bytes, in the order they appear. Forms of
/// type `application/x-www-form-urlencoded` and `multipart/form-data` are understood.
pub fn pick(content_type: &str, body: &[u8], wanted: &[&str]) -> Vec<(String, Vec<u8>)> {
    let lower = content_type.to_ascii_lowercase();
    if lower.starts_with("application/x-www-form-urlencoded") {
        urlencoded(body, wanted)
    } else if lower.starts_with("multipart/form-data") {
        multipart(content_type, body, wanted)
    } else {
        Vec::new()
    }
}

/// The first value of `name` among `picked`, as text.
pub fn text(picked: &[(String, Vec<u8>)], name: &str) -> Option<String> {
    let (_, value) = picked.iter().find(|(key, _)| key == name)?;
    Some(String::from_utf8_lossy(value).trim().to_owned())
}

fn urlencoded(body: &[u8], wanted: &[&str]) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    for pair in body.split(|&b| b == b'&') {
        let mut halves = pair.splitn(2, |&b| b == b'=');
        // The site decodes names too, so `act%69on` is `action`.
        let name = percent_decode(halves.next().unwrap_or(b""));
        let name = String::from_utf8_lossy(&name).into_owned();
        let Some(value) = halves.next() else { continue };
        if wanted.contains(&name.as_str()) {
            found.push((name, percent_decode(value)));
        }
    }
    found
}

fn percent_decode(raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(raw.len());
    let mut at = 0;
    while at < raw.len() {
        let escaped = if raw[at] == b'%' && at + 2 < raw.len() {
            hex(raw[at + 1]).zip(hex(raw[at + 2]))
        } else {
            None
        };
        if let Some((high, low)) = escaped {
            out.push(high * 16 + low);
            at += 3;
        } else {
            out.push(if raw[at] == b'+' { b' ' } else { raw[at] });
            at += 1;
        }
    }
    out
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|at| at + from)
}

fn multipart(content_type: &str, body: &[u8], wanted: &[&str]) -> Vec<(String, Vec<u8>)> {
    let lower = content_type.to_ascii_lowercase();
    let Some(at) = lower.find("boundary=") else {
        return Vec::new();
    };
    let boundary = content_type[at + 9..]
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .trim_matches('"');
    if boundary.is_empty() {
        return Vec::new();
    }
    let delimiter = format!("--{boundary}").into_bytes();
    let mut found = Vec::new();
    let mut start = find(body, &delimiter, 0);
    while let Some(here) = start {
        let part_start = here + delimiter.len();
        let next = find(body, &delimiter, part_start);
        let part = &body[part_start..next.unwrap_or(body.len())];
        if let Some((name, value)) = split_part(part)
            && wanted.contains(&name.as_str())
        {
            found.push((name, value.to_vec()));
        }
        start = next;
    }
    found
}

/// The field name and the value of one part: its headers, an empty line, then the value.
fn split_part(part: &[u8]) -> Option<(String, &[u8])> {
    let head_end = find(part, b"\r\n\r\n", 0)?;
    let head = String::from_utf8_lossy(&part[..head_end]);
    let mut name = None;
    for line in head.lines() {
        if line
            .to_ascii_lowercase()
            .starts_with("content-disposition:")
        {
            name = disposition_name(line);
        }
    }
    let value = &part[head_end + 4..];
    // The line break before the next boundary belongs to the boundary, not to the value.
    let value = value.strip_suffix(b"\r\n").unwrap_or(value);
    Some((name?, value))
}

/// The `name` parameter of a `Content-Disposition` line, in any letter case and with or without
/// quotes or spaces around the `=`. `filename` is a different parameter.
fn disposition_name(line: &str) -> Option<String> {
    for parameter in split_unquoted(line).into_iter().skip(1) {
        let Some((key, value)) = parameter.split_once('=') else {
            continue;
        };
        if key.trim().eq_ignore_ascii_case("name") {
            return Some(value.trim().trim_matches('"').to_owned());
        }
    }
    None
}

/// Splits at semicolons that are not inside a quoted string, so a file name such as
/// `"x; name=action"` stays one parameter. A backslash escapes the next character in quotes.
fn split_unquoted(line: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (at, c) in line.char_indices() {
        if escaped {
            escaped = false;
        } else if quoted && c == '\\' {
            escaped = true;
        } else if c == '"' {
            quoted = !quoted;
        } else if c == ';' && !quoted {
            parts.push(&line[start..at]);
            start = at + 1;
        }
    }
    parts.push(&line[start..]);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_urlencoded_form_gives_only_the_wanted_fields_decoded() {
        let body = b"password=hunter2&contestId=2285&submittedProblemIndex=B&x=a%20b+c";
        let got = pick(
            "application/x-www-form-urlencoded",
            body,
            &["contestId", "x"],
        );
        assert_eq!(text(&got, "contestId").as_deref(), Some("2285"));
        assert_eq!(text(&got, "x").as_deref(), Some("a b c"));
        assert_eq!(text(&got, "password"), None);
        assert_eq!(got.len(), 2);
    }

    #[test]
    fn a_multipart_form_gives_values_and_files_by_name() {
        let body = b"--b\r\nContent-Disposition: form-data; name=\"csrf_token\"\r\n\r\nSECRET\r\n\
--b\r\nContent-Disposition: form-data; name=\"programTypeId\"\r\n\r\n54\r\n\
--b\r\nContent-Disposition: form-data; name=\"sourceFile\"; filename=\"a.cpp\"\r\n\r\nint main() {}\r\nreturn 0;\r\n--b--\r\n";
        let got = pick(
            "multipart/form-data; boundary=b",
            body,
            &["programTypeId", "sourceFile"],
        );
        assert_eq!(text(&got, "programTypeId").as_deref(), Some("54"));
        assert_eq!(got[1].1, b"int main() {}\r\nreturn 0;");
        assert_eq!(text(&got, "csrf_token"), None);
    }

    #[test]
    fn other_content_types_and_a_missing_boundary_give_nothing() {
        assert_eq!(pick("application/json", b"{}", &["a"]), Vec::new());
        assert_eq!(pick("multipart/form-data", b"--b\r\n", &["a"]), Vec::new());
    }

    #[test]
    fn a_semicolon_inside_a_file_name_does_not_make_a_parameter() {
        let line =
            r#"Content-Disposition: form-data; filename="x; name=action"; name="sourceFile""#;
        assert_eq!(disposition_name(line).as_deref(), Some("sourceFile"));
    }

    #[test]
    fn names_are_found_however_they_are_written() {
        let got = pick(
            "application/x-www-form-urlencoded",
            b"act%69on=a&x=1",
            &["action"],
        );
        assert_eq!(text(&got, "action").as_deref(), Some("a"));

        let body = "--b
Content-Disposition: form-data; filename=\"x\"; NAME = \"action\"

submitSolutionFormSubmitted
--b--
"
        .replace('\n', "\r\n");
        let got = pick(
            "multipart/form-data; boundary=b",
            body.as_bytes(),
            &["action"],
        );
        assert_eq!(
            text(&got, "action").as_deref(),
            Some("submitSolutionFormSubmitted")
        );
    }
}
