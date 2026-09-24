use super::*;

#[test]
fn durable_identifier_schemas_accept_compact_and_reject_retired_hex() {
    let prefixes = [
        "wc_dagent_",
        "wc_endpoint_",
        "wc_agent_task_",
        "wc_agent_task_attempt_",
        "wc_wake_",
        "wc_wake_attempt_",
        "wc_agent_wait_",
        "wc_goal_",
        "wc_conv_",
        "wc_participant_",
        "wc_cmsg_",
        "wc_delivery_",
        "wc_attention_event_",
        "wc_agent_task_fence_",
        "wc_wake_consume_",
        "wc_host_binding_",
    ];
    fn visit(
        value: &serde_json::Value,
        prefixes: &[&str],
        seen: &mut std::collections::HashSet<String>,
    ) {
        match value {
            serde_json::Value::Object(object) => {
                if let Some(pattern) = object.get("pattern").and_then(|v| v.as_str()) {
                    for &prefix in prefixes {
                        // Match the domain's exact literal prefix, not a prefix
                        // of another domain (task vs task_attempt, for example).
                        if !pattern.contains(&format!("{prefix}[")) {
                            continue;
                        }
                        let regex = regex::Regex::new(pattern).unwrap();
                        let proof = prefix.ends_with("fence_")
                            || prefix.ends_with("consume_")
                            || prefix.ends_with("binding_");
                        let suffix = if proof {
                            webcodex_core::compact::encode([0xfb; 16])
                        } else {
                            webcodex_core::compact::encode([0xfb; 12])
                        };
                        assert!(regex.is_match(&format!("{prefix}{suffix}")), "{pattern}");
                        assert!(
                            !regex.is_match(&format!("{prefix}{}", "a".repeat(32))),
                            "{pattern}"
                        );
                        seen.insert(prefix.to_string());
                    }
                }
                for child in object.values() {
                    visit(child, prefixes, seen);
                }
            }
            serde_json::Value::Array(values) => {
                for child in values {
                    visit(child, prefixes, seen);
                }
            }
            _ => {}
        }
    }
    let mut seen = std::collections::HashSet::new();
    for spec in registered_tool_specs()
        .into_iter()
        .chain(crate::registry::agent_continuation_app_tool_specs())
    {
        visit(&spec.input_schema, &prefixes, &mut seen);
        visit(&spec.output_schema, &prefixes, &mut seen);
    }
    for prefix in prefixes {
        // Attention events are projected only through their enclosing records.
        if prefix != "wc_attention_event_" {
            assert!(seen.contains(prefix), "missing schema coverage: {prefix}");
        }
    }
}

#[test]
fn workflow_session_identifier_schemas_accept_compact_and_persisted_legacy_forms() {
    fn visit(value: &serde_json::Value, seen: &mut std::collections::HashSet<&'static str>) {
        match value {
            serde_json::Value::Object(object) => {
                if let Some(pattern) = object.get("pattern").and_then(|value| value.as_str()) {
                    let cases = [
                        (
                            "session",
                            "^wc_sess_([A-Za-z0-9_-]{16}|[0-9a-f]{32})$",
                            "wc_sess_",
                        ),
                        (
                            "message",
                            "^wc_msg_([A-Za-z0-9_-]{16}|[0-9a-f]{32})$",
                            "wc_msg_",
                        ),
                    ];
                    for (name, expected_pattern, prefix) in cases {
                        if pattern != expected_pattern {
                            continue;
                        }
                        let regex = regex::Regex::new(pattern).unwrap();
                        let compact = webcodex_core::compact::encode([0xfb; 12]);
                        assert!(compact.contains('-') || compact.contains('_'));
                        assert!(regex.is_match(&format!("{prefix}{compact}")), "{pattern}");
                        assert!(regex.is_match(&format!("{prefix}{}", "a".repeat(32))));
                        assert!(!regex.is_match(&format!("{prefix}{}", "A".repeat(32))));
                        assert!(!regex.is_match(&format!("{prefix}{}", "a".repeat(15))));
                        assert!(!regex.is_match(&format!("{prefix}{}", "a".repeat(17))));
                        seen.insert(name);
                    }
                }
                for child in object.values() {
                    visit(child, seen);
                }
            }
            serde_json::Value::Array(values) => {
                for child in values {
                    visit(child, seen);
                }
            }
            _ => {}
        }
    }

    let mut seen = std::collections::HashSet::new();
    for spec in registered_tool_specs()
        .into_iter()
        .chain(crate::registry::agent_continuation_app_tool_specs())
    {
        visit(&spec.input_schema, &mut seen);
        visit(&spec.output_schema, &mut seen);
    }
    assert!(seen.contains("session"));
    assert!(seen.contains("message"));
}

#[test]
fn registered_tool_string_length_bounds_are_not_inverted() {
    fn visit(value: &serde_json::Value, path: &str) {
        match value {
            serde_json::Value::Object(object) => {
                if let (Some(minimum), Some(maximum)) = (
                    object.get("minLength").and_then(|value| value.as_u64()),
                    object.get("maxLength").and_then(|value| value.as_u64()),
                ) {
                    assert!(
                        minimum <= maximum,
                        "{path}: minLength {minimum} > maxLength {maximum}"
                    );
                }
                for (key, child) in object {
                    visit(child, &format!("{path}/{key}"));
                }
            }
            serde_json::Value::Array(values) => {
                for (index, child) in values.iter().enumerate() {
                    visit(child, &format!("{path}/{index}"));
                }
            }
            _ => {}
        }
    }

    for spec in registered_tool_specs()
        .into_iter()
        .chain(crate::registry::agent_continuation_app_tool_specs())
    {
        visit(&spec.input_schema, &format!("{}/input", spec.name));
        visit(&spec.output_schema, &format!("{}/output", spec.name));
    }
}
