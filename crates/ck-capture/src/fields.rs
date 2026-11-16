//! The names of the fields in a form or JSON body. The values are never read out of this module.

const MAX_FIELDS: usize = 64;
const MAX_NAME: usize = 64;
/// Written instead of a name that is long or has odd characters: such a "name" may be a value.
const ODD: &str = "<odd name>";

fn clean(name: &str) -> String {
    let plain = !name.is_empty()
        && name.len() <= MAX_NAME
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'[' | b']'));
    if plain {
        name.to_string()
    } else {
        ODD.to_string()
    }
}

/// The field names of a body, in the order they appear, or nothing when the type is not a form.
pub fn field_names(content_type: &str, body: &[u8]) -> Vec<String> {
    let lower = content_type.to_ascii_lowercase();
    let mut names = if lower.starts_with("application/x-www-form-urlencoded") {
        urlencoded(&String::from_utf8_lossy(body))
    } else if lower.starts_with("application/json") {
        json(body)
    } else if lower.starts_with("multipart/form-data") {
        multipart(content_type, &String::from_utf8_lossy(body))
    } else {
        Vec::new()
    };
    names.truncate(MAX_FIELDS);
    names
}

/// The names in a query string or in a form body.
pub fn urlencoded(text: &str) -> Vec<String> {
    text.split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| clean(pair.split('=').next().unwrap_or("")))
        .collect()
}

fn json(body: &[u8]) -> Vec<String> {
    match serde_json::from_slice::<serde_json::Value>(body) {
        Ok(serde_json::Value::Object(map)) => map.keys().map(|key| clean(key)).collect(),
        _ => Vec::new(),
    }
}

/// The `name="..."` of each part's `Content-Disposition` line. Only lines right after a boundary
/// line, up to the first empty line, are read, so a part's body cannot add a name.
fn multipart(content_type: &str, text: &str) -> Vec<String> {
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
    let delimiter = format!("--{boundary}");
    let mut names = Vec::new();
    let mut in_headers = false;
    for line in text.lines() {
        if line == delimiter {
            in_headers = true;
        } else if line.starts_with(&delimiter) || (in_headers && line.is_empty()) {
            in_headers = false;
        } else if in_headers
            && line
                .to_ascii_lowercase()
                .starts_with("content-disposition:")
        {
            // `filename="..."` also contains `name="`; only a standalone `name="` counts.
            if let Some(start) = line.find(" name=\"") {
                let rest = &line[start + 7..];
                names.push(clean(rest.split('"').next().unwrap_or("")));
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_form_names_and_not_values() {
        let names = field_names(
            "application/x-www-form-urlencoded; charset=UTF-8",
            b"handleOrEmail=ana%40example.org&password=hunter2&csrf_token=abc&remember=on",
        );
        assert_eq!(
            names,
            ["handleOrEmail", "password", "csrf_token", "remember"]
        );
    }

    #[test]
    fn reads_top_level_json_names_only() {
        let names = field_names("application/json", br#"{"user":"ana","pw":{"inner":"x"}}"#);
        // serde_json lists object keys in sorted order.
        assert_eq!(names, ["pw", "user"]);
        assert_eq!(
            field_names("application/json", b"{ cut off"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn reads_multipart_names_and_ignores_the_parts_bodies() {
        let body = "--b\r\nContent-Disposition: form-data; name=\"csrf\"\r\n\r\nsecret1\r\n\
                    --b\r\nContent-Disposition: form-data; name=\"source\"; filename=\"a.cpp\"\r\n\
                    Content-Type: text/plain\r\n\r\nint main(){}\r\nContent-Disposition: form-data; name=\"fake\"\r\n\
                    --b--\r\n";
        assert_eq!(
            field_names("multipart/form-data; boundary=b", body.as_bytes()),
            ["csrf", "source"]
        );
    }

    #[test]
    fn a_long_or_odd_name_is_hidden() {
        let long = "a".repeat(80);
        let names = urlencoded(&format!("{long}=1&bad name=2&ok=3"));
        assert_eq!(names, [ODD, ODD, "ok"]);
    }

    #[test]
    fn other_content_types_give_no_names() {
        assert_eq!(field_names("text/html", b"a=b"), Vec::<String>::new());
    }
}
