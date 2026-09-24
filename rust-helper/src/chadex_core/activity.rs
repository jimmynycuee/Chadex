use serde::Serialize;

const ACTIVITY_MESSAGE_BYTES: usize = 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RuntimeActivityEntry {
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub source: String,
    pub level: &'static str,
    pub event_kind: &'static str,
    pub message: String,
}

pub fn sanitize_message(message: &str) -> String {
    let mut safe = redact_bearer_credentials(message);
    for prefix in [
        "wc_pair_",
        "wc_pat_",
        "wc_agent_",
        "webcodex_temporary_",
        "sk-",
    ] {
        safe = redact_prefixed_token(&safe, prefix);
    }
    truncate_utf8(&safe, ACTIVITY_MESSAGE_BYTES)
}

fn redact_bearer_credentials(value: &str) -> String {
    let lower = value.to_ascii_lowercase();
    let mut cursor = 0;
    let mut out = String::with_capacity(value.len());
    while let Some(relative) = lower[cursor..].find("bearer") {
        let start = cursor + relative;
        let after_scheme = start + "bearer".len();
        let boundary = start == 0 || !value.as_bytes()[start - 1].is_ascii_alphanumeric();
        if !boundary || !value[after_scheme..].starts_with(char::is_whitespace) {
            out.push_str(&value[cursor..after_scheme]);
            cursor = after_scheme;
            continue;
        }
        let tail = &value[after_scheme..];
        let token_start = after_scheme + tail.len() - tail.trim_start().len();
        let token_len = value[token_start..]
            .char_indices()
            .take_while(|(_, ch)| {
                !ch.is_whitespace() && !matches!(ch, '\"' | '\'' | ',' | ';' | '}' | ']')
            })
            .map(|(offset, ch)| offset + ch.len_utf8())
            .last()
            .unwrap_or(0);
        out.push_str(&value[cursor..start]);
        out.push_str("[redacted]");
        cursor = token_start + token_len;
    }
    out.push_str(&value[cursor..]);
    out
}

fn redact_prefixed_token(value: &str, prefix: &str) -> String {
    let mut rest = value;
    let mut out = String::with_capacity(value.len());
    while let Some(index) = rest.find(prefix) {
        out.push_str(&rest[..index]);
        out.push_str("[redacted]");
        let tail = &rest[index + prefix.len()..];
        let consumed = tail
            .char_indices()
            .take_while(|(_, ch)| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
            .map(|(offset, ch)| offset + ch.len_utf8())
            .last()
            .unwrap_or(0);
        rest = &tail[consumed..];
    }
    out.push_str(rest);
    out
}

fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizer_redacts_runtime_credentials() {
        let safe = sanitize_message(
            "Authorization: Bearer abc wc_pair_secret wc_pat_secret wc_agent_secret webcodex_temporary_secret",
        );
        assert!(!safe.contains("secret"));
        assert!(!safe.contains("abc"));
        assert!(safe.matches("[redacted]").count() >= 5);
    }

    #[test]
    fn sanitizer_removes_entire_bearer_token_in_mixed_case_and_json() {
        for input in [
            "Authorization: Bearer opaqueToken123",
            "aUtHoRiZaTiOn: bEaReR\topaqueToken123",
            r#"{"Authorization":"Bearer opaqueToken123"}"#,
            "失敗 Bearer opaqueToken123 next=ok",
            "key=sk-proj-testKey123",
        ] {
            let safe = sanitize_message(input);
            assert!(!safe.contains("opaqueToken123"));
            assert!(!safe.contains("testKey123"));
            assert!(safe.contains("[redacted]"));
        }
        assert_eq!(
            sanitize_message("bearer of good news"),
            "[redacted] good news"
        );
    }

    #[test]
    fn sanitizer_truncates_on_utf8_boundary() {
        let input = "界".repeat(400);
        let safe = truncate_utf8(&input, ACTIVITY_MESSAGE_BYTES);
        assert!(safe.ends_with('…'));
        assert!(safe.len() <= ACTIVITY_MESSAGE_BYTES + '…'.len_utf8());
    }
}
