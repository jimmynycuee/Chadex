use serde_json::{json, Value};

const PLAN_ID_PATTERN: &str = "^wc_plan_[A-Za-z0-9_-]{12,32}$";
const GOAL_RUN_ID_PATTERN: &str = "^wc_grun_[A-Za-z0-9_-]{12,32}$";

fn plan_id() -> Value {
    json!({"type": "string", "pattern": PLAN_ID_PATTERN, "description": "Plan id returned by plan_start."})
}

fn goal_run_id() -> Value {
    json!({"type": "string", "pattern": GOAL_RUN_ID_PATTERN, "description": "Goal run id returned by goal_run_start."})
}

fn object(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

pub fn plan_start_input_schema() -> Value {
    object(
        json!({
            "project": {"type": "string", "minLength": 1, "description": "Configured project id to plan for."},
            "title": {"type": "string", "minLength": 1, "maxLength": 200, "description": "Short title of the work being planned."}
        }),
        &["project", "title"],
    )
}

fn acceptance_schema() -> Value {
    json!({
        "type": "array",
        "maxItems": 8,
        "description": "Objective acceptance checks. Each runs as literal argv (no shell) in the project root or cwd; exit code 0 passes. Goal Mode succeeds only when all pass. Prefer the repository's real test/build/lint commands. Omit to keep the previous revision's checks.",
        "items": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "minLength": 1, "maxLength": 80},
                "argv": {"type": "array", "minItems": 1, "maxItems": 32, "items": {"type": "string", "maxLength": 1024}},
                "cwd": {"type": "string", "description": "Optional project-relative directory."},
                "timeout_secs": {"type": "integer", "minimum": 1, "maximum": 3600, "default": 600}
            },
            "required": ["name", "argv"],
            "additionalProperties": false
        }
    })
}

fn budget_schema() -> Value {
    json!({
        "type": "object",
        "description": "Goal Mode limits. Omit to keep the previous revision's budget (defaults: 5 iterations, 120 minutes, 1800 s per agent turn).",
        "properties": {
            "max_iterations": {"type": "integer", "minimum": 1, "maximum": 20},
            "max_minutes": {"type": "integer", "minimum": 1, "maximum": 1440},
            "agent_timeout_secs": {"type": "integer", "minimum": 60, "maximum": 3600}
        },
        "additionalProperties": false
    })
}

pub fn plan_save_input_schema() -> Value {
    object(
        json!({
            "plan_id": plan_id(),
            "parent_revision": {"type": "integer", "minimum": 0, "description": "The revision this edit is based on; must be the current latest revision (0 before the first save)."},
            "content": {"type": "string", "minLength": 1, "maxLength": 65536, "description": "Full plan in Markdown: context and evidence, approach, steps, risks, and how it will be verified."},
            "acceptance": acceptance_schema(),
            "budget": budget_schema()
        }),
        &["plan_id", "parent_revision", "content"],
    )
}

pub fn plan_get_input_schema() -> Value {
    object(
        json!({
            "plan_id": plan_id(),
            "revision": {"type": "integer", "minimum": 1, "description": "Revision to read; defaults to the latest."},
            "diff_from": {"type": "integer", "minimum": 1, "description": "Optional older revision to diff against."}
        }),
        &["plan_id"],
    )
}

pub fn plan_list_input_schema() -> Value {
    object(
        json!({"include_closed": {"type": "boolean", "default": false}}),
        &[],
    )
}

pub fn goal_run_start_input_schema() -> Value {
    object(
        json!({
            "plan_id": {"type": "string", "pattern": PLAN_ID_PATTERN, "description": "Approved Plan Mode plan to execute. Omit for a direct Goal."},
            "project": {"type": "string", "minLength": 1, "description": "Direct Goal: configured project id to work in."},
            "title": {"type": "string", "minLength": 1, "maxLength": 200, "description": "Direct Goal: short title."},
            "objective": {"type": "string", "minLength": 1, "maxLength": 65536, "description": "Direct Goal: what must be true when done, constraints, and anything the agent must not change."},
            "acceptance": acceptance_schema(),
            "budget": budget_schema()
        }),
        &[],
    )
}

pub fn goal_run_status_input_schema() -> Value {
    object(
        json!({
            "goal_run_id": goal_run_id(),
            "wait_secs": {"type": "integer", "minimum": 0, "maximum": 45, "default": 0, "description": "Wait up to this many seconds for the run to finish or stop before answering."}
        }),
        &["goal_run_id"],
    )
}

pub fn goal_run_list_input_schema() -> Value {
    object(json!({}), &[])
}

pub fn goal_run_control_input_schema() -> Value {
    object(
        json!({
            "goal_run_id": goal_run_id(),
            "action": {"type": "string", "enum": ["pause", "resume", "cancel"]}
        }),
        &["goal_run_id", "action"],
    )
}

pub fn plan_review_input_schema() -> Value {
    object(
        json!({
            "action": {"type": "string", "enum": ["list", "inspect", "approve", "approve_and_start", "reject", "revoke", "close"]},
            "plan_id": plan_id(),
            "revision": {"type": "integer", "minimum": 1},
            "content_sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
            "reason": {"type": "string", "maxLength": 2048},
            "approval_id": {"type": "string"},
            "ttl_minutes": {"type": "integer", "minimum": 1, "maximum": 10080}
        }),
        &["action"],
    )
}

pub fn goal_operator_input_schema() -> Value {
    object(
        json!({
            "action": {"type": "string", "enum": ["overview", "status", "pause", "resume", "cancel", "dismiss", "clear_board", "emergency_stop", "release_emergency_stop"]},
            "goal_run_id": goal_run_id(),
            "reason": {"type": "string", "maxLength": 2048}
        }),
        &["action"],
    )
}
