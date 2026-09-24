pub(super) fn is_lower_hex(value: &str, expected_len: usize) -> bool {
    value.len() == expected_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn is_git_object_hex(value: &str) -> bool {
    is_lower_hex(value, 40) || is_lower_hex(value, 64)
}

pub(super) fn parse_status_result_field<'a>(result: &'a str, key: &str) -> Option<&'a str> {
    result.lines().find_map(|line| {
        let (field, value) = line.split_once('=')?;
        (field == key).then_some(value.trim())
    })
}

pub(super) fn parse_optional_usize(result: &str, key: &str) -> Option<usize> {
    parse_status_result_field(result, key).and_then(|v| v.parse::<usize>().ok())
}

pub(super) fn parse_optional_bool(result: &str, key: &str) -> Option<bool> {
    parse_status_result_field(result, key).and_then(|v| match v {
        "0" | "false" => Some(false),
        "1" | "true" => Some(true),
        _ => None,
    })
}

pub(super) fn parse_fixed_decimal(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() || !bytes.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

pub(super) fn strip_wire_lf(value: &str) -> Option<String> {
    if value.is_empty() {
        Some(String::new())
    } else {
        value.strip_suffix('\n').map(ToOwned::to_owned)
    }
}
