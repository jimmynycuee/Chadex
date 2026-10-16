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
            "project": {"type": "string", "minLength": 1, "description": "Configured project id to plan for. Omit to use the project open in the Chadex app."},
            "title": {"type": "string", "minLength": 1, "maxLength": 200, "description": "Short title of the work being planned."}
        }),
        &["title"],
    )
}

fn acceptance_schema() -> Value {
    json!({
        "type": "array",
        "maxItems": 8,
        "description": "Literal argv, no shell (root/cwd); exit 0 passes. All checks gate Goal success.",
        "items": {
            "type": "object",
            "properties": {
                "name": {"type": "string", "minLength": 1, "maxLength": 80},
                "argv": {"type": "array", "minItems": 1, "maxItems": 32, "items": {"type": "string", "maxLength": 1024}},
                "cwd": {"type": "string", "description": "Project-relative directory."},
                "timeout_secs": {"type": "integer", "minimum": 1, "maximum": 3600, "default": 600},
                "lane": {"type":"string", "enum":["offline_sandbox","loopback_integration","external_attestation"], "default":"offline_sandbox"},
                "kind": {"type":"string", "enum":["test","external_gate"], "default":"test"},
                "loopback_ports": {"type":"array", "maxItems":8, "uniqueItems":true, "items":{"type":"integer", "minimum":49152, "maximum":65535}},
                "container_image": {"type":"string", "pattern":"^sha256:[a-f0-9]{64}$", "description":"Exact local Linux image digest; network=none, never pulled."},
                "depends_on": {"type":"array", "maxItems":8, "uniqueItems":true, "items":{"type":"string", "minLength":1, "maxLength":80}},
                "external_gate": {"type":"object", "properties":{"owner":{"type":"string"},"action":{"type":"string"},"evidence":{"type":"string"},"next_step":{"type":"string"}}, "required":["owner","action","evidence","next_step"], "additionalProperties":false}
            },
            "required": ["name", "argv"],
            "additionalProperties": false
        }
    })
}

fn workspace_requirements_schema() -> Value {
    json!({"type":"array","maxItems":128,"items":{"type":"object","properties":{
        "source_path":{"type":"string","minLength":1,"maxLength":1024},
        "source_sha256":{"type":"string","pattern":"^[a-f0-9]{64}$"},
        "destination_path":{"type":"string","minLength":1,"maxLength":1024},
        "permission":{"type":"string","enum":["read_only"]}
    },"required":["source_path","source_sha256","destination_path","permission"],"additionalProperties":false}})
}
fn dependencies_schema() -> Value {
    json!({"type":"array","maxItems":32,"items":{"type":"object","properties":{
        "path":{"type":"string","minLength":1,"maxLength":1024},
        "provenance":{"type":"string","minLength":1,"maxLength":2048}
    },"required":["path","provenance"],"additionalProperties":false}})
}

fn budget_schema() -> Value {
    json!({
        "type": "object",
        "description": "Goal limits; defaults 5 iterations, 120 min, 1800 s/agent turn.",
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
            "budget": budget_schema(),
            "workspace_requirements": workspace_requirements_schema(),
            "dependencies": dependencies_schema()
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
            "plan_id": {"type": "string", "pattern": PLAN_ID_PATTERN, "description": "Existing plan id; omit for a new direct Goal."},
            "project": {"type": "string", "minLength": 1, "description": "Project id; omit for the app's open project."},
            "title": {"type": "string", "minLength": 1, "maxLength": 200, "description": "Short direct Goal title."},
            "objective": {"type": "string", "minLength": 1, "maxLength": 65536, "description": "Success condition, constraints, and prohibited changes."},
            "acceptance": acceptance_schema(),
            "budget": budget_schema(),
            "workspace_requirements": workspace_requirements_schema(),
            "dependencies": dependencies_schema()
        }),
        &[],
    )
}

pub fn plan_wait_approval_input_schema() -> Value {
    object(
        json!({
            "plan_id": plan_id(),
            "wait_secs": {"type": "integer", "minimum": 0, "maximum": 45, "default": 45, "description": "Wait up to this many seconds for the user's decision."}
        }),
        &["plan_id"],
    )
}

pub fn goal_check_input_schema() -> Value {
    object(
        json!({
            "goal_run_id": goal_run_id(),
            "wait_secs": {"type": "integer", "minimum": 0, "maximum": 45, "default": 45, "description": "Wait up to this many seconds for the checks to finish."}
        }),
        &["goal_run_id"],
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
            "ttl_minutes": {"type": "integer", "minimum": 1, "maximum": 10080},
            "mode": {"type": "string", "enum": ["codex", "chatgpt"]}
        }),
        &["action"],
    )
}

pub fn goal_operator_input_schema() -> Value {
    object(
        json!({
            "action": {"type": "string", "enum": ["overview", "status", "pause", "resume", "cancel", "dismiss", "clear_board", "emergency_stop", "release_emergency_stop", "retry_acceptance", "propose_acceptance", "approve_acceptance", "adopt_checkpoint"]},
            "goal_run_id": goal_run_id(),
            "project": {"type": "string", "minLength": 1, "description": "Optional full configured project id; clear_board is scoped to this project when supplied and clears all projects when omitted."},
            "reason": {"type": "string", "maxLength": 2048},
            "acceptance": acceptance_schema(),
            "workspace_requirements": workspace_requirements_schema(),
            "dependencies": dependencies_schema(),
            "expected_acceptance_revision": {"type":"integer","minimum":1},
            "acceptance_sha256": {"type":"string","pattern":"^[a-f0-9]{64}$"},
            "proposal_id": {"type":"string","minLength":1,"maxLength":96}
        }),
        &["action"],
    )
}
