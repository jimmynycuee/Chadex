use serde_json::{json, Value};

use super::common::{object_schema, with_optional_session_id};

pub fn git_review_summary_input_schema() -> Value {
    let mut schema = object_schema(with_optional_session_id(vec![
        ("project", "string", "Runner-registered project id.", true),
        (
            "base_commit",
            "string",
            "Exact 40-hex Git commit object id used to compute the merge-base.",
            true,
        ),
        (
            "head_commit",
            "string",
            "Exact 40-hex Git commit object id reviewed from merge-base to head.",
            true,
        ),
    ]));
    for field in ["base_commit", "head_commit"] {
        schema["properties"][field]["minLength"] = Value::from(40);
        schema["properties"][field]["maxLength"] = Value::from(40);
        schema["properties"][field]["pattern"] = Value::from("^[0-9A-Fa-f]{40}$");
    }
    schema
}

pub fn show_changes_input_schema() -> Value {
    object_schema(with_optional_session_id(vec![
        ("project", "string", "Runner-registered project id.", true),
        (
            "session_id",
            "string",
            "Optional wc_sess_* id to summarize with the git changes.",
            false,
        ),
        (
            "include_diff",
            "boolean",
            "Include bounded diff hunks (default false).",
            false,
        ),
        (
            "max_hunks",
            "integer",
            "Maximum hunks to return when include_diff=true (clamped).",
            false,
        ),
        (
            "max_hunk_lines",
            "integer",
            "Maximum lines per hunk when include_diff=true (clamped).",
            false,
        ),
        (
            "session_event_limit",
            "integer",
            "Optional recent session-event diagnostic projection (clamped). Omit or use 0 for the compact default, which keeps review signals and changed paths without event history.",
            false,
        ),
    ]))
}

pub fn git_status_input_schema() -> Value {
    object_schema(with_optional_session_id(vec![(
        "project",
        "string",
        "Configured project id.",
        true,
    )]))
}

pub fn git_commit_paths_input_schema() -> Value {
    let mut schema = object_schema(with_optional_session_id(vec![
        ("project", "string", "Runner-registered project id.", true),
        (
            "expected_head",
            "string",
            "Exact current 40-hex HEAD fence; normally copy show_changes.head.commit immediately before committing.",
            true,
        ),
        (
            "paths",
            "array",
            "Exact project-relative file paths to commit. Directories, project root, sensitive paths, and unchanged paths are rejected.",
            true,
        ),
        (
            "message",
            "string",
            "Commit message. Bounded and never persisted in model-facing audit previews.",
            true,
        ),
    ]));
    schema["properties"]["expected_head"]["minLength"] = Value::from(40);
    schema["properties"]["expected_head"]["maxLength"] = Value::from(40);
    schema["properties"]["expected_head"]["pattern"] = Value::from("^[0-9A-Fa-f]{40}$");
    schema["properties"]["paths"]["minItems"] = Value::from(1);
    schema["properties"]["paths"]["maxItems"] = Value::from(32);
    schema["properties"]["paths"]["items"] = json!({
        "type": "string",
        "minLength": 1,
        "maxLength": 512
    });
    schema["properties"]["message"]["minLength"] = Value::from(1);
    schema["properties"]["message"]["maxLength"] = Value::from(1000);
    schema
}

pub fn git_diff_hunks_input_schema() -> Value {
    let mut schema = object_schema(with_optional_session_id(vec![
        ("project", "string", "Runner-registered project id.", true),
        (
            "paths",
            "array",
            "Optional project-relative paths to scope diff.",
            false,
        ),
        (
            "max_hunks",
            "integer",
            "Maximum hunks to return (clamped).",
            false,
        ),
        (
            "max_hunk_lines",
            "integer",
            "Maximum lines per hunk (clamped).",
            false,
        ),
        (
            "max_page_bytes",
            "integer",
            "Raw producer page budget in bytes, independent of the final serialized model result. The concrete default and producer bounds are derived from the shared runtime contract.",
            false,
        ),
        (
            "cached",
            "boolean",
            "Use staged diff via git diff --cached.",
            false,
        ),
        (
            "base_commit",
            "string",
            "Optional exact 40-hex Git commit object id; requires head_commit and committed-range mode.",
            false,
        ),
        (
            "head_commit",
            "string",
            "Optional exact 40-hex Git commit object id reviewed from the single merge-base; requires base_commit.",
            false,
        ),
        (
            "continuation",
            "string",
            "Compact opaque runtime continuation returned by git_diff_hunks. Copy it verbatim only through the returned parser-ready suggested_call; do not interpret it. It may identify either a later-record page cursor or the next complete-line fragment of one exact hunk; token type is opaque and scope/fence-bound. Repeat its exact original effective scope/paging inputs unchanged (base_commit/head_commit for committed mode, cached/worktree mode, paths, max_hunks, max_hunk_lines, and max_page_bytes). Later-record and hunk-fragment continuations remain distinct identities.",
            false,
        ),
    ]));
    // Negative byte budgets are not meaningful and cannot be represented by
    // the runtime's usize input. Zero and other sub-minimum nonnegative values
    // intentionally reach the authoritative runtime clamp.
    schema["properties"]["max_page_bytes"]["minimum"] = Value::from(0);
    schema["properties"]["max_page_bytes"]["default"] =
        Value::from(webcodex_core::runtime_contract::DEFAULT_GIT_DIFF_HUNKS_PAGE_BYTES);
    let min_page_kib = webcodex_core::runtime_contract::MIN_GIT_DIFF_HUNKS_PAGE_BYTES / 1024;
    let default_page_kib =
        webcodex_core::runtime_contract::DEFAULT_GIT_DIFF_HUNKS_PAGE_BYTES / 1024;
    let max_page_kib = webcodex_core::runtime_contract::MAX_GIT_DIFF_HUNKS_PAGE_BYTES / 1024;
    schema["properties"]["max_page_bytes"]["description"] = json!(format!(
        "Raw producer page budget in bytes, independent of the final serialized model result. Defaults to the shared safe producer maximum ({default_page_kib} KiB). Any recognized nonnegative integer is accepted and runtime-clamped to the fixed {min_page_kib}..{max_page_kib} KiB producer bounds so ordinary Runner result retention retains framing headroom."
    ));
    for field in ["base_commit", "head_commit"] {
        schema["properties"][field]["minLength"] = Value::from(40);
        schema["properties"][field]["maxLength"] = Value::from(40);
        schema["properties"][field]["pattern"] = Value::from("^[0-9A-Fa-f]{40}$");
    }
    schema["properties"]["continuation"]["maxLength"] =
        Value::from(webcodex_core::runtime_contract::GIT_DIFF_HUNKS_CONTINUATION_MAX_BYTES);
    schema["allOf"] = json!([
        {
            "if": { "required": ["base_commit"] },
            "then": {
                "required": ["head_commit"],
                "properties": { "cached": { "const": false } }
            }
        },
        {
            "if": { "required": ["head_commit"] },
            "then": {
                "required": ["base_commit"],
                "properties": { "cached": { "const": false } }
            }
        },
        {
            "if": {
                "required": ["cached"],
                "properties": { "cached": { "const": true } }
            },
            "then": {
                "not": {
                    "anyOf": [
                        { "required": ["base_commit"] },
                        { "required": ["head_commit"] }
                    ]
                }
            }
        }
    ]);
    schema
}

pub fn git_log_input_schema() -> Value {
    let mut schema = object_schema(with_optional_session_id(vec![
        ("project", "string", "Runner-registered project id.", true),
        (
            "head_commit",
            "string",
            "Optional exact 40-hex commit snapshot fence. Omit on the first page so Runtime resolves the current HEAD; parser-ready suggested_call carries the observed commit on later pages.",
            false,
        ),
        (
            "limit",
            "integer",
            "Maximum commits to return (default 20, clamped to 1..100).",
            false,
        ),
        (
            "skip",
            "integer",
            "Number of recent commits to skip (default 0, clamped to 0..10000).",
            false,
        ),
    ]));
    schema["properties"]["head_commit"]["minLength"] = Value::from(40);
    schema["properties"]["head_commit"]["maxLength"] = Value::from(40);
    schema["properties"]["head_commit"]["pattern"] = Value::from("^[0-9A-Fa-f]{40}$");
    schema
}
