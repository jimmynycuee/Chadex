use serde_json::{json, Value};

use super::common::{array_schema, schema_type, wrapped_output_schema};

fn stats_schema() -> Value {
    json!({
        "type": "object",
        "description": "Bounded orchestration evidence for this one-shot Code Mode execution.",
        "additionalProperties": false,
        "properties": {
            "tool_calls": {"type": "integer", "minimum": 0, "maximum": 32},
            "max_in_flight": {"type": "integer", "minimum": 0, "maximum": 8},
            "duration_ms": {"type": "integer", "minimum": 0},
            "returned_bytes": {"type": "integer", "minimum": 0, "maximum": 65536}
        },
        "required": ["tool_calls", "max_in_flight", "duration_ms", "returned_bytes"]
    })
}

fn content_schema() -> Value {
    let mut schema = array_schema(
        schema_type("string", "One bounded text(value) emission."),
        "Only text(value) emissions selected by the JavaScript orchestration. Nested raw ToolResults are not copied here automatically.",
    );
    schema["maxItems"] = json!(256);
    schema
}

fn effect_receipt_schema() -> Value {
    json!({
        "type": "object",
        "description": "Sparse correctness receipt for consequential canonical child calls that actually crossed the orchestration dispatch boundary.",
        "additionalProperties": false,
        "properties": {
            "consequential_calls": {"type": "integer", "minimum": 0, "maximum": 32},
            "known_results": {"type": "integer", "minimum": 0, "maximum": 32},
            "job_handoffs": {"type": "integer", "minimum": 0, "maximum": 32},
            "outcome_unknown": {"type": "integer", "minimum": 0, "maximum": 32},
            "children": {
                "type": "array",
                "maxItems": 32,
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "properties": {
                        "ordinal": {"type": "integer", "minimum": 1, "maximum": 32},
                        "tool": {"type": "string", "maxLength": 128},
                        "outcome": {"type": "string", "enum": ["known_result", "job_handoff", "outcome_unknown"]},
                        "state_changed": {"type": "boolean", "description": "Authoritative canonical state-change truth, present only for a known mutation result."},
                        "job_id": {"type": "string", "description": "Canonical Job identity, present only for a normal same-execution Job handoff."},
                        "continuation": {"type": "object", "description": "Parser-ready canonical Job continuation, present only when returned by the child ToolResult."}
                    },
                    "required": ["ordinal", "tool", "outcome"]
                }
            }
        },
        "required": ["consequential_calls", "known_results", "job_handoffs", "outcome_unknown", "children"]
    })
}

fn bounded_failure_message_schema() -> Value {
    let mut schema = schema_type(
        "string",
        "Model-facing bounded frontend failure detail. When consequential children were dispatched, the message warns against blindly rerunning the whole JavaScript program.",
    );
    schema["maxLength"] = json!(16_384);
    schema
}

fn failure_kind_schema() -> Value {
    json!({
        "type": "string",
        "enum": [
            "invalid_request",
            "runtime_error",
            "timeout",
            "tool_call_budget_exceeded",
            "output_limit_exceeded"
        ],
        "description": "Present on a bounded Code Mode runtime/host failure. Ordinary nested ToolResult business failures remain JavaScript values and do not become this field."
    })
}

pub(super) fn output_schema_for_tool(name: &str) -> Option<Value> {
    match name {
        "code_mode_exec" => Some(wrapped_output_schema(vec![
            ("content", content_schema()),
            ("stats", stats_schema()),
            ("message", bounded_failure_message_schema()),
            ("failure_kind", failure_kind_schema()),
        ])),
        "code_mode_exec_effectful" | "code_mode_exec_mutating" => {
            Some(wrapped_output_schema(vec![
                ("content", content_schema()),
                ("stats", stats_schema()),
                ("effect_receipt", effect_receipt_schema()),
                ("message", bounded_failure_message_schema()),
                ("failure_kind", failure_kind_schema()),
            ]))
        }
        _ => None,
    }
}
