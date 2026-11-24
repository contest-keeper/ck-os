//! Squid is trusted, but the bytes it sends carry what a contestant's browser sent. Whatever arrives,
//! reading it must end in a value or an error, never a panic, and never keep more than `MAX_KEPT_BODY`.
//!
//! The number of cases per property is proptest's default (256) and follows `PROPTEST_CASES`.

use proptest::prelude::*;

use super::{IcapRequest, MAX_KEPT_BODY, read_request};

fn read(bytes: &[u8]) -> Result<Option<IcapRequest>, crate::Error> {
    read_request(&mut &bytes[..], &mut |_| {})
}

/// A message head with the given `Encapsulated` value and a body made of the given chunk lines.
fn message(encapsulated: &str, rest: &[u8]) -> Vec<u8> {
    let mut bytes = format!(
        "REQMOD icap://x/reqmod ICAP/1.0\r\nPreview: 4\r\nEncapsulated: {encapsulated}\r\n\r\n"
    )
    .into_bytes();
    bytes.extend_from_slice(rest);
    bytes
}

proptest! {
    #[test]
    fn any_bytes_end_in_a_value_or_an_error(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        if let Ok(Some(request)) = read(&bytes) {
            prop_assert!(request.body.len() <= MAX_KEPT_BODY);
        }
    }

    #[test]
    fn any_text_after_a_real_request_line_ends_in_a_value_or_an_error(
        lines in proptest::collection::vec("[^\r\n]{0,200}", 0..80),
    ) {
        let mut bytes = b"REQMOD icap://x/reqmod ICAP/1.0\r\n".to_vec();
        for line in &lines {
            bytes.extend_from_slice(line.as_bytes());
            bytes.extend_from_slice(b"\r\n");
        }
        if let Ok(Some(request)) = read(&bytes) {
            prop_assert!(request.body.len() <= MAX_KEPT_BODY);
        }
    }

    #[test]
    fn any_encapsulated_value_ends_in_a_value_or_an_error(
        value in "[^\r\n]{0,200}",
        rest in proptest::collection::vec(any::<u8>(), 0..2048),
    ) {
        if let Ok(Some(request)) = read(&message(&value, &rest)) {
            prop_assert!(request.body.len() <= MAX_KEPT_BODY);
        }
    }

    #[test]
    fn huge_or_unordered_offsets_are_refused_without_allocating(
        first in any::<u64>(),
        second in any::<u64>(),
    ) {
        let value = format!("req-hdr={first}, res-hdr={second}, res-body={}", second.wrapping_add(1));
        let _ = read(&message(&value, b"short"));
    }

    #[test]
    fn any_chunk_line_ends_in_a_value_or_an_error(
        chunk_line in "[^\r\n]{0,64}",
        data in proptest::collection::vec(any::<u8>(), 0..256),
    ) {
        let mut rest = chunk_line.into_bytes();
        rest.extend_from_slice(b"\r\n");
        rest.extend_from_slice(&data);
        if let Ok(Some(request)) = read(&message("req-body=0", &rest)) {
            prop_assert!(request.body.len() <= MAX_KEPT_BODY);
        }
    }

    #[test]
    fn a_huge_chunk_size_is_refused_without_reading_that_much(size in "[1-9a-fA-F][0-9a-fA-F]{8,39}") {
        let result = read(&message("req-body=0", format!("{size}\r\nabc").as_bytes()));
        prop_assert!(result.is_err());
    }

    #[test]
    fn a_body_in_any_chunking_keeps_its_first_part_and_says_when_it_was_cut(
        body in proptest::collection::vec(any::<u8>(), 0..(MAX_KEPT_BODY + 4096)),
        chunk in 1usize..40_000,
    ) {
        let mut rest = Vec::new();
        for part in body.chunks(chunk) {
            rest.extend_from_slice(format!("{:x}\r\n", part.len()).as_bytes());
            rest.extend_from_slice(part);
            rest.extend_from_slice(b"\r\n");
        }
        rest.extend_from_slice(b"0; ieof\r\n\r\n");
        let request = read(&message("req-body=0", &rest)).expect("a well-formed message").expect("a request");
        let kept = body.len().min(MAX_KEPT_BODY);
        prop_assert_eq!(&request.body[..], &body[..kept]);
        prop_assert_eq!(request.body_truncated, body.len() > MAX_KEPT_BODY);
    }
}
