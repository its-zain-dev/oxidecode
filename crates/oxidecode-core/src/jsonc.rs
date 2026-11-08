//! JSON-with-comments (JSONC) handling used by OpenCode configuration files.

/// Removes `//` and `/* */` comments plus trailing commas, leaving strings intact.
pub fn strip(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;

    while i < bytes.len() {
        let byte = bytes[i];
        if in_string {
            out.push(byte as char);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }

        match byte {
            b'"' => {
                in_string = true;
                out.push('"');
                i += 1;
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => {
                i += 2;
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'*' => {
                i += 2;
                while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                    i += 1;
                }
                i = (i + 2).min(bytes.len());
            }
            _ => {
                out.push(byte as char);
                i += 1;
            }
        }
    }

    remove_trailing_commas(&out)
}

fn remove_trailing_commas(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    let mut in_string = false;
    let mut escaped = false;

    while i < bytes.len() {
        let byte = bytes[i];
        if in_string {
            out.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if byte == b'"' {
            in_string = true;
            out.push(byte);
            i += 1;
            continue;
        }
        if byte == b',' {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < bytes.len() && (bytes[j] == b'}' || bytes[j] == b']') {
                i += 1;
                continue;
            }
        }
        out.push(byte);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::strip;

    #[test]
    fn strips_comments_and_trailing_commas() {
        let input = r#"{
            // comment
            "a": 1, /* block */
            "b": [1, 2,],
            "c": "http://not-a-comment",
        }"#;
        let parsed: serde_json::Value = serde_json::from_str(&strip(input)).unwrap();
        assert_eq!(parsed["a"], 1);
        assert_eq!(parsed["c"], "http://not-a-comment");
        assert_eq!(parsed["b"].as_array().unwrap().len(), 2);
    }
}
