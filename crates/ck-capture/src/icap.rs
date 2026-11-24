//! ICAP (RFC 3507): reading one message from Squid and writing the replies.
//!
//! Only what this service needs: `OPTIONS`, `REQMOD` and `RESPMOD`, a preview of the body, and the
//! answer "unchanged" (`204`), or the original message sent back when `204` is not allowed.

use std::io::{BufRead, Read, Write};

use crate::Error;

/// The most of a body that is kept. A login form or a code submission is far smaller.
pub const MAX_KEPT_BODY: usize = 64 * 1024;
/// The preview asked of Squid. Squid cannot hold a whole 64 KiB in the buffer it previews from: it
/// stopped at 65535 bytes of a 65536-byte preview and then waited, so the page hung. This stays
/// well under that.
const PREVIEW_BYTES: usize = 48 * 1024;
/// A message bigger than this is refused: it is not a form.
const MAX_BODY_READ: usize = 16 * 1024 * 1024;
const MAX_LINE: u64 = 16 * 1024;
const MAX_HEAD_SECTION: usize = 64 * 1024;
const MAX_HEADERS: usize = 64;
const ISTAG: &str = "\"ck-capture-1\"";

pub struct IcapRequest {
    pub method: String,
    pub uri: String,
    pub allow_204: bool,
    pub preview: bool,
    pub req_head: Vec<u8>,
    pub res_head: Vec<u8>,
    pub body: Vec<u8>,
    /// The body was longer than what is kept, or the preview ended before the body did.
    pub body_truncated: bool,
}

fn icap(message: impl Into<String>) -> Error {
    Error::Icap(message.into())
}

fn io(context: &'static str) -> impl FnOnce(std::io::Error) -> Error {
    move |source| Error::Io { context, source }
}

/// Reads one line without its line ending. `None` at the end of the connection.
fn read_line(reader: &mut impl BufRead) -> Result<Option<String>, Error> {
    let mut line = Vec::new();
    let read = reader
        .by_ref()
        .take(MAX_LINE)
        .read_until(b'\n', &mut line)
        .map_err(io("reading from the proxy"))?;
    if read == 0 {
        return Ok(None);
    }
    if !line.ends_with(b"\n") {
        return Err(icap("a line is too long or cut off"));
    }
    while line.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
        line.pop();
    }
    Ok(Some(String::from_utf8_lossy(&line).into_owned()))
}

/// Reads the next message. `None` when Squid closed the connection between messages.
///
/// `trace` is told what is being read, so a machine started for observation can show where a
/// message stops arriving.
pub fn read_request(
    reader: &mut impl BufRead,
    trace: &mut dyn FnMut(&str),
) -> Result<Option<IcapRequest>, Error> {
    let Some(request_line) = read_line(reader)? else {
        return Ok(None);
    };
    let mut parts = request_line.split_whitespace();
    let (Some(method), Some(uri), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(icap("bad request line"));
    };
    if !version.starts_with("ICAP/") {
        return Err(icap("not an ICAP request"));
    }

    let mut request = IcapRequest {
        method: method.to_ascii_uppercase(),
        uri: uri.to_string(),
        allow_204: false,
        preview: false,
        req_head: Vec::new(),
        res_head: Vec::new(),
        body: Vec::new(),
        body_truncated: false,
    };
    let mut encapsulated = String::new();
    for _ in 0..=MAX_HEADERS {
        let line = read_line(reader)?.ok_or_else(|| icap("cut off in the headers"))?;
        if line.is_empty() {
            trace(&format!(
                "{} head read, encapsulated {encapsulated}",
                request.method
            ));
            read_parts(reader, &encapsulated, &mut request, trace)?;
            return Ok(Some(request));
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(icap("bad header line"));
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "encapsulated" => encapsulated = value.to_string(),
            "allow" => request.allow_204 = value.split(',').any(|v| v.trim() == "204"),
            "preview" => request.preview = true,
            _ => {}
        }
    }
    Err(icap("too many headers"))
}

/// Reads the encapsulated HTTP heads and the body. The `Encapsulated` header lists where each part
/// starts, in the order they follow: `req-hdr=0, res-hdr=137, res-body=296`.
fn read_parts(
    reader: &mut impl BufRead,
    encapsulated: &str,
    request: &mut IcapRequest,
    trace: &mut dyn FnMut(&str),
) -> Result<(), Error> {
    let mut sections: Vec<(&str, usize)> = Vec::new();
    for item in encapsulated.split(',') {
        let Some((name, offset)) = item.trim().split_once('=') else {
            continue;
        };
        let offset = offset
            .trim()
            .parse()
            .map_err(|_| icap("bad Encapsulated offset"))?;
        sections.push((name.trim(), offset));
    }
    let mut position = 0;
    for (index, (name, start)) in sections.iter().enumerate() {
        if *start != position {
            return Err(icap("Encapsulated parts are not in order"));
        }
        let Some((_, end)) = sections.get(index + 1) else {
            if matches!(*name, "req-body" | "res-body" | "opt-body") {
                read_chunked(reader, request, trace)?;
            }
            break;
        };
        let length = end
            .checked_sub(*start)
            .filter(|length| *length <= MAX_HEAD_SECTION)
            .ok_or_else(|| icap("an HTTP head is too big"))?;
        let mut part = vec![0; length];
        reader
            .read_exact(&mut part)
            .map_err(io("reading an HTTP head"))?;
        match *name {
            "req-hdr" => request.req_head = part,
            "res-hdr" => request.res_head = part,
            _ => {}
        }
        position = *end;
    }
    Ok(())
}

/// Reads a chunked body to its end, keeping the first part. An `ieof` mark means the preview holds
/// the whole body; a preview that ends without it means the rest was not sent.
fn read_chunked(
    reader: &mut impl BufRead,
    request: &mut IcapRequest,
    trace: &mut dyn FnMut(&str),
) -> Result<(), Error> {
    let mut total = 0usize;
    let mut chunks = 0usize;
    loop {
        let line = read_line(reader)?.ok_or_else(|| icap("cut off in a body"))?;
        let (size, extension) = line.split_once(';').unwrap_or((line.as_str(), ""));
        let size = usize::from_str_radix(size.trim(), 16).map_err(|_| icap("bad chunk size"))?;
        chunks += 1;
        // The first few chunks, then now and then, so a message that stops arriving shows how far
        // it got without a line for each of thousands of chunks.
        if chunks <= 3 || chunks.is_power_of_two() || chunks.is_multiple_of(16) {
            trace(&format!(
                "chunk {chunks}: {size} bytes, {total} before it, extension {extension:?}"
            ));
        }
        if size == 0 {
            // A last chunk may be followed by trailer lines up to an empty line.
            while read_line(reader)?.is_some_and(|line| !line.is_empty()) {}
            if request.preview && !extension.contains("ieof") {
                request.body_truncated = true;
            }
            return Ok(());
        }
        total = total.saturating_add(size);
        if total > MAX_BODY_READ {
            return Err(icap("a body is too big"));
        }
        let keep = size.min(MAX_KEPT_BODY.saturating_sub(request.body.len()));
        let mut chunk = vec![0; keep];
        reader
            .read_exact(&mut chunk)
            .map_err(io("reading a body"))?;
        request.body.extend_from_slice(&chunk);
        if keep < size {
            request.body_truncated = true;
            let rest = (size - keep) as u64;
            std::io::copy(&mut reader.by_ref().take(rest), &mut std::io::sink())
                .map_err(io("skipping a body"))?;
        }
        read_line(reader)?; // the line ending after the chunk
    }
}

pub fn write_options(writer: &mut impl Write, uri: &str) -> Result<(), Error> {
    let method = if uri.contains("respmod") {
        "RESPMOD"
    } else {
        "REQMOD"
    };
    let reply = format!(
        "ICAP/1.0 200 OK\r\nMethods: {method}\r\nService: ck-capture\r\nISTag: {ISTAG}\r\n\
         Max-Connections: 64\r\nOptions-TTL: 3600\r\nAllow: 204\r\nPreview: {PREVIEW_BYTES}\r\nTransfer-Preview: *\r\n\
         Encapsulated: null-body=0\r\n\r\n"
    );
    send(writer, reply.as_bytes())
}

/// Says "unchanged". When the proxy did not allow a `204`, the original message is sent back
/// instead, which is only possible when it was all received.
pub fn write_unchanged(writer: &mut impl Write, request: &IcapRequest) -> Result<(), Error> {
    if request.allow_204 || request.preview {
        let reply = format!(
            "ICAP/1.0 204 No Content\r\nISTag: {ISTAG}\r\nEncapsulated: null-body=0\r\n\r\n"
        );
        send(writer, reply.as_bytes())
    } else if request.body_truncated {
        write_error(writer)
    } else {
        send(writer, &echo(request))
    }
}

pub fn write_error(writer: &mut impl Write) -> Result<(), Error> {
    send(
        writer,
        b"ICAP/1.0 500 Server Error\r\nEncapsulated: null-body=0\r\n\r\n",
    )
}

fn send(writer: &mut impl Write, bytes: &[u8]) -> Result<(), Error> {
    writer
        .write_all(bytes)
        .and_then(|()| writer.flush())
        .map_err(io("writing to the proxy"))
}

fn echo(request: &IcapRequest) -> Vec<u8> {
    // A reply to a response modification carries the response only, not the request it answers.
    let respmod = request.method == "RESPMOD";
    let req_head: &[u8] = if respmod { &[] } else { &request.req_head };
    let mut parts: Vec<String> = Vec::new();
    let mut offset = 0;
    if !req_head.is_empty() {
        parts.push("req-hdr=0".to_string());
        offset = req_head.len();
    }
    if !request.res_head.is_empty() {
        parts.push(format!("res-hdr={offset}"));
        offset += request.res_head.len();
    }
    let body = match (request.body.is_empty(), request.res_head.is_empty()) {
        (true, _) => "null-body",
        (false, true) => "req-body",
        (false, false) => "res-body",
    };
    parts.push(format!("{body}={offset}"));
    let encapsulated = parts.join(", ");
    let mut reply =
        format!("ICAP/1.0 200 OK\r\nISTag: {ISTAG}\r\nEncapsulated: {encapsulated}\r\n\r\n")
            .into_bytes();
    reply.extend_from_slice(req_head);
    reply.extend_from_slice(&request.res_head);
    if !request.body.is_empty() {
        reply.extend_from_slice(format!("{:x}\r\n", request.body.len()).as_bytes());
        reply.extend_from_slice(&request.body);
        reply.extend_from_slice(b"\r\n0\r\n\r\n");
    }
    reply
}

#[cfg(test)]
mod hostile_input;

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> IcapRequest {
        read_request(&mut text.as_bytes(), &mut |_| {})
            .unwrap()
            .unwrap()
    }

    #[test]
    fn reads_a_request_with_a_preview_body() {
        let head = "POST /enter HTTP/1.1\r\nHost: example.org\r\n\r\n";
        let text = format!(
            "REQMOD icap://127.0.0.1:1344/reqmod ICAP/1.0\r\nAllow: 204\r\nPreview: 65536\r\n\
             Encapsulated: req-hdr=0, req-body={}\r\n\r\n{head}5\r\nhello\r\n0; ieof\r\n\r\n",
            head.len()
        );
        let request = parse(&text);
        assert_eq!(request.method, "REQMOD");
        assert_eq!(request.req_head, head.as_bytes());
        assert_eq!(request.body, b"hello");
        assert!(!request.body_truncated);
        assert!(request.allow_204 && request.preview);
    }

    #[test]
    fn a_preview_without_ieof_is_marked_cut() {
        let head = "POST / HTTP/1.1\r\n\r\n";
        let text = format!(
            "REQMOD icap://x/reqmod ICAP/1.0\r\nPreview: 4\r\n\
             Encapsulated: req-hdr=0, req-body={}\r\n\r\n{head}4\r\nabcd\r\n0\r\n\r\n",
            head.len()
        );
        assert!(parse(&text).body_truncated);
    }

    #[test]
    fn keeps_only_the_first_part_of_a_long_body_and_reads_on_to_its_end() {
        let head = "POST / HTTP/1.1\r\n\r\n";
        let size = MAX_KEPT_BODY + 10;
        let text = format!(
            "REQMOD icap://x/reqmod ICAP/1.0\r\nAllow: 204\r\n\
             Encapsulated: req-hdr=0, req-body={}\r\n\r\n{head}{size:x}\r\n{}\r\n0\r\n\r\nNEXT",
            head.len(),
            "a".repeat(size)
        );
        let mut reader = text.as_bytes();
        let request = read_request(&mut reader, &mut |_| {}).unwrap().unwrap();
        assert_eq!(request.body.len(), MAX_KEPT_BODY);
        assert!(request.body_truncated);
        assert_eq!(reader, b"NEXT");
    }

    #[test]
    fn a_message_without_a_body_needs_no_chunks() {
        let head = "GET / HTTP/1.1\r\n\r\n";
        let text = format!(
            "REQMOD icap://x/reqmod ICAP/1.0\r\nEncapsulated: req-hdr=0, null-body={}\r\n\r\n{head}",
            head.len()
        );
        let request = parse(&text);
        assert_eq!(request.req_head, head.as_bytes());
        assert_eq!(request.body, Vec::<u8>::new());
    }

    #[test]
    fn reads_a_response_message_with_both_heads() {
        let req = "GET /a HTTP/1.1\r\nHost: h\r\n\r\n";
        let res = "HTTP/1.1 200 OK\r\n\r\n";
        let text = format!(
            "RESPMOD icap://x/respmod ICAP/1.0\r\nAllow: 204\r\n\
             Encapsulated: req-hdr=0, res-hdr={}, res-body={}\r\n\r\n{req}{res}2\r\nhi\r\n0\r\n\r\n",
            req.len(),
            req.len() + res.len()
        );
        let request = parse(&text);
        assert_eq!(request.req_head, req.as_bytes());
        assert_eq!(request.res_head, res.as_bytes());
        assert_eq!(request.body, b"hi");
    }

    #[test]
    fn refuses_garbage_and_oversized_bodies() {
        assert!(read_request(&mut "hello\r\n\r\n".as_bytes(), &mut |_| {}).is_err());
        let text = "REQMOD x ICAP/1.0\r\nEncapsulated: req-body=0\r\n\r\nffffffff\r\n";
        assert!(read_request(&mut text.as_bytes(), &mut |_| {}).is_err());
        assert!(
            read_request(&mut "".as_bytes(), &mut |_| {})
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn unchanged_is_a_204_when_allowed_and_an_echo_when_not() {
        let mut out = Vec::new();
        let mut request = parse(
            "REQMOD x ICAP/1.0\r\nAllow: 204\r\nEncapsulated: req-hdr=0, null-body=4\r\n\r\nGET\n",
        );
        write_unchanged(&mut out, &request).unwrap();
        assert!(out.starts_with(b"ICAP/1.0 204"));

        request.allow_204 = false;
        out.clear();
        write_unchanged(&mut out, &request).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("ICAP/1.0 200 OK"));
        assert!(text.contains("Encapsulated: req-hdr=0, null-body=4"));
        assert!(text.ends_with("GET\n"));
    }

    #[test]
    fn an_echo_is_framed_with_carriage_returns_like_every_other_icap_line() {
        let head = "POST /a HTTP/1.1\r\n\r\n";
        let mut request = parse(&format!(
            "REQMOD x ICAP/1.0\r\nEncapsulated: req-hdr=0, req-body={}\r\n\r\n{head}2\r\nhi\r\n0\r\n\r\n",
            head.len()
        ));
        request.allow_204 = false;
        request.preview = false;
        let mut out = Vec::new();
        write_unchanged(&mut out, &request).unwrap();
        let expected = format!(
            "ICAP/1.0 200 OK\r\nISTag: {ISTAG}\r\nEncapsulated: req-hdr=0, req-body={}\r\n\r\n{head}2\r\nhi\r\n0\r\n\r\n",
            head.len()
        );
        assert_eq!(String::from_utf8(out).unwrap(), expected);
    }

    #[test]
    fn options_offer_a_preview_and_a_response_echo_leaves_out_the_request() {
        let mut out = Vec::new();
        write_options(&mut out, "/respmod").unwrap();
        let options = String::from_utf8(out).unwrap();
        assert!(options.contains("Methods: RESPMOD"));
        assert!(options.contains("Transfer-Preview: *"));
        assert!(options.contains(&format!("Preview: {PREVIEW_BYTES}\r\n")));

        let req = "GET /a HTTP/1.1\r\n\r\n";
        let res = "HTTP/1.1 200 OK\r\n\r\n";
        let mut request = parse(&format!(
            "RESPMOD x ICAP/1.0\r\nEncapsulated: req-hdr=0, res-hdr={}, null-body={}\r\n\r\n{req}{res}",
            req.len(),
            req.len() + res.len()
        ));
        request.allow_204 = false;
        let mut out = Vec::new();
        write_unchanged(&mut out, &request).unwrap();
        let reply = String::from_utf8(out).unwrap();
        assert!(reply.contains(&format!("Encapsulated: res-hdr=0, null-body={}", res.len())));
        assert!(!reply.contains("GET /a"));
        assert!(reply.ends_with(res));
    }
}
