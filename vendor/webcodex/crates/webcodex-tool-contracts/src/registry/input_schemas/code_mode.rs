use serde_json::{json, Value};

use super::common::object_schema;

pub fn code_mode_exec_input_schema() -> Value {
    let mut schema = object_schema(vec![
        (
            "project",
            "string",
            "Required Project target. Nested JavaScript tool calls cannot select or override Project authority.",
            true,
        ),
        (
            "session_id",
            "string",
            "Required exact Workflow Session. Nested JavaScript tool calls remain bound to this Session and record canonical evidence there.",
            true,
        ),
        (
            "source",
            "string",
            "Bounded JavaScript orchestration source. tools.<name>(args) returns a Promise for admitted read-only tools; use Promise.all only for independent observations, keep result-dependent/adaptive calls sequential, and call text(value) for final bounded output. Project/Session are outer-bound. No shell, filesystem, network, Node, Deno, WebAssembly, mutation, validation, Jobs, plugins, or MCP are exposed.",
            true,
        ),
        (
            "timeout_ms",
            "integer",
            "Optional wall-clock budget in milliseconds. Defaults to 5000 and is server-clamped to 1..30000.",
            false,
        ),
    ]);
    schema["properties"]["source"]["maxLength"] = Value::from(65_536);
    schema["properties"]["timeout_ms"]["minimum"] = Value::from(0);
    schema["properties"]["session_id"]["pattern"] =
        json!("^wc_sess_([A-Za-z0-9_-]{16}|[0-9a-f]{32})$");
    schema
}

pub fn code_mode_exec_effectful_input_schema() -> Value {
    let mut schema = object_schema(vec![
        (
            "project",
            "string",
            "Required Project target. Nested JavaScript tool calls cannot select or override Project authority.",
            true,
        ),
        (
            "session_id",
            "string",
            "Required exact Workflow Session. Every nested child remains a canonical ToolRuntime invocation in this same Session.",
            true,
        ),
        (
            "source",
            "string",
            "Experimental E2a JavaScript orchestration source. Admitted tools are the E1 read-only set plus cargo_check and cargo_test. Structured validators may hand off the same execution as ordinary Jobs; no mutation, shell, generic process, Job observation, plugins/MCP, or recursive Code Mode is exposed.",
            true,
        ),
        (
            "timeout_ms",
            "integer",
            "Optional orchestration/frontend decision deadline in milliseconds. Defaults to 5000 and is server-clamped to 1..30000. The response may follow after a short bounded drain of already-started canonical child calls needed to report truthful consequential outcomes.",
            false,
        ),
    ]);
    schema["properties"]["source"]["maxLength"] = Value::from(65_536);
    schema["properties"]["timeout_ms"]["minimum"] = Value::from(0);
    schema["properties"]["session_id"]["pattern"] =
        json!("^wc_sess_([A-Za-z0-9_-]{16}|[0-9a-f]{32})$");
    schema
}

pub fn code_mode_exec_mutating_input_schema() -> Value {
    let mut schema = object_schema(vec![
        (
            "project",
            "string",
            "Required Project target. Nested JavaScript tool calls cannot select or override Project authority.",
            true,
        ),
        (
            "session_id",
            "string",
            "Required exact Workflow Session. Every nested child remains a canonical ToolRuntime invocation in this same Session.",
            true,
        ),
        (
            "source",
            "string",
            "Experimental E2b JavaScript orchestration source. Admitted tools are the E1 read set plus one canonical apply_text_edits mutation attempt. Validation, shell/process, Jobs, other mutations, gateways, and recursive Code Mode are not exposed. Use read_files read_revision for guarded adaptive edits and inspect after mutation.",
            true,
        ),
        (
            "timeout_ms",
            "integer",
            "Optional orchestration/frontend decision deadline in milliseconds. Defaults to 5000 and is server-clamped to 1..30000. Already-started canonical mutation may be reconciled for at most a short bounded drain so state-change truth is not fabricated.",
            false,
        ),
    ]);
    schema["properties"]["source"]["maxLength"] = Value::from(65_536);
    schema["properties"]["timeout_ms"]["minimum"] = Value::from(0);
    schema["properties"]["session_id"]["pattern"] =
        json!("^wc_sess_([A-Za-z0-9_-]{16}|[0-9a-f]{32})$");
    schema
}
