use serde_json::{json, Value};

fn property(schema: &Value, name: &str) -> Value {
    schema["properties"][name].clone()
}

fn process_fields() -> serde_json::Map<String, Value> {
    let source = super::jobs::run_process_input_schema();
    let mut fields = serde_json::Map::new();
    for name in ["executable", "args", "stdin", "cwd", "timeout_secs"] {
        fields.insert(name.to_string(), property(&source, name));
    }
    fields
}

fn process_check_schema() -> Value {
    let fields = process_fields();
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": fields,
        "required": ["executable"]
    })
}

fn task_step_schema() -> Value {
    let search = super::files::search_project_texts_input_schema();
    let read = super::files::read_files_input_schema();
    let edits = super::line_edits::apply_text_edits_input_schema();
    let process = super::jobs::run_process_input_schema();
    let mut run_fields = process_fields();
    run_fields.insert(
        "kind".to_string(),
        json!({"type":"string","const":"run_process"}),
    );
    run_fields.insert("purpose".to_string(), property(&process, "purpose"));

    json!({
        "oneOf": [
            {
                "type":"object",
                "additionalProperties":false,
                "properties":{
                    "kind":{"type":"string","const":"search"},
                    "queries": property(&search, "queries"),
                    "max_result_bytes": property(&search, "max_result_bytes")
                },
                "required":["kind","queries"]
            },
            {
                "type":"object",
                "additionalProperties":false,
                "properties":{
                    "kind":{"type":"string","const":"read"},
                    "items": property(&read, "items"),
                    "with_line_numbers": property(&read, "with_line_numbers"),
                    "max_result_bytes": property(&read, "max_result_bytes")
                },
                "required":["kind","items"]
            },
            {
                "type":"object",
                "additionalProperties":false,
                "properties":{
                    "kind":{"type":"string","const":"edit"},
                    "changes": property(&edits, "changes"),
                    "dry_run": property(&edits, "dry_run")
                },
                "required":["kind","changes"]
            },
            {
                "type":"object",
                "additionalProperties":false,
                "properties": run_fields,
                "required":["kind","executable"]
            },
            {
                "type":"object",
                "additionalProperties":false,
                "properties":{
                    "kind":{"type":"string","const":"validate"},
                    "checks":{
                        "type":"array",
                        "minItems":1,
                        "maxItems":4,
                        "items":process_check_schema(),
                        "description":"One to four deterministic structured process validations. They run sequentially with purpose=validation and stop on the first failure."
                    }
                },
                "required":["kind","checks"]
            },
            {
                "type":"object",
                "additionalProperties":false,
                "properties":{
                    "kind":{"type":"string","const":"review"},
                    "include_diff":{"type":"boolean","default":true},
                    "max_hunks":{"type":"integer","minimum":1,"maximum":100},
                    "max_hunk_lines":{"type":"integer","minimum":1,"maximum":400}
                },
                "required":["kind"]
            }
        ]
    })
}

fn policy_schema() -> Value {
    json!({
        "type":"object",
        "additionalProperties":false,
        "properties":{
            "max_steps":{"type":"integer","minimum":1,"maximum":20,"default":12},
            "max_mutations":{"type":"integer","minimum":0,"maximum":8,"default":4},
            "max_changed_files":{"type":"integer","minimum":1,"maximum":32,"default":8},
            "timeout_secs":{"type":"integer","minimum":1,"maximum":600,"default":120},
            "max_result_bytes":{"type":"integer","minimum":8192,"maximum":262144,"default":32768},
            "allowed_path_prefixes":{
                "type":"array",
                "maxItems":16,
                "items":{"type":"string","minLength":1,"maxLength":512},
                "description":"Optional project-relative path prefixes. When non-empty, search/read/edit file paths must stay inside one prefix."
            },
            "allowed_operations":{
                "type":"array",
                "maxItems":6,
                "uniqueItems":true,
                "items":{"type":"string","enum":["search","read","edit","run_process","validate","review"]},
                "description":"Optional closed allowlist of task step kinds. Omit or [] to allow the Phase 9A-D bounded vocabulary."
            }
        }
    })
}

fn acceptance_schema() -> Value {
    json!({
        "type":"object",
        "additionalProperties":false,
        "properties":{
            "require_validation_success":{"type":"boolean","default":true,"description":"Require a successful validate step before completion."},
            "require_review":{"type":"boolean","default":true,"description":"Require a successful final workspace review. Must remain true when the plan contains run_process or validate steps."}
        }
    })
}

fn preplan_schema() -> Value {
    json!({
        "type":"object",
        "additionalProperties":false,
        "description":"Task difficulty evidence estimated before the deterministic execution steps are shaped. Use task scope, likely impacted subsystems/languages, validation domains, and risk characteristics; do not derive these counts from how many execute_task steps you chose.",
        "properties":{
            "estimated_file_count":{"type":"integer","minimum":1,"maximum":64},
            "estimated_subsystem_count":{"type":"integer","minimum":1,"maximum":32},
            "estimated_language_count":{"type":"integer","minimum":1,"maximum":16},
            "validation_domain_count":{"type":"integer","minimum":0,"maximum":16},
            "cross_runtime_boundary":{"type":"boolean","default":false},
            "stateful_or_schema_change":{"type":"boolean","default":false},
            "concurrency_or_security_sensitive":{"type":"boolean","default":false},
            "graphify_informed":{"type":"boolean","default":false,"description":"True only when a fresh Graphify graph materially informed the pre-plan scope estimate."}
        },
        "required":["estimated_file_count","estimated_subsystem_count","estimated_language_count","validation_domain_count"]
    })
}

pub fn execute_task_input_schema() -> Value {
    json!({
        "type":"object",
        "additionalProperties":false,
        "properties":{
            "project":{"type":"string","minLength":1,"description":"Exact or resolvable Runner-registered project id."},
            "task_id":{"type":"string","pattern":"^chadex_task_[0-9a-f]{32}$","description":"Stable caller-chosen execution id for replay protection, observation and cancellation. Reusing it never re-executes work. Omission creates a fresh execution."},
            "goal":{"type":"string","minLength":1,"maxLength":4000,"description":"Bounded task goal. Runtime does not ask an LLM to reinterpret it."},
            "preplan":preplan_schema(),
            "package_count":{"type":"integer","minimum":1,"maximum":4,"description":"Optional caller override for the logical package target. When preplan is present and package_count is omitted, Runtime starts from the pre-plan complexity recommendation. When both are omitted, legacy behavior remains one package. Runtime may only reduce the target using complexity, safe validation boundaries, or fresh Graphify EXTRACTED dependency evidence."},
            "steps":{"type":"array","minItems":1,"maxItems":20,"items":task_step_schema(),"description":"Already-decided deterministic execution plan."},
            "policy":policy_schema(),
            "acceptance":acceptance_schema(),
            "session_id":{"type":"string","pattern":"^wc_sess_([A-Za-z0-9_-]{16}|[0-9a-f]{32})$","description":"Optional existing Workflow Session for ordinary nested tool evidence."}
        },
        "required":["project","goal","steps"]
    })
}

fn task_batch_item_schema() -> Value {
    json!({
        "type":"object",
        "additionalProperties":false,
        "properties":{
            "task_id":{"type":"string","pattern":"^chadex_task_[0-9a-f]{32}$","description":"Stable caller-chosen execution id for replay protection. Reusing it never re-executes work. Omission creates a fresh execution."},
            "goal":{"type":"string","minLength":1,"maxLength":4000},
            "preplan":preplan_schema(),
            "package_count":{"type":"integer","minimum":1,"maximum":4},
            "steps":{"type":"array","minItems":1,"maxItems":20,"items":task_step_schema()},
            "policy":policy_schema(),
            "acceptance":acceptance_schema(),
            "session_id":{"type":"string","pattern":"^wc_sess_([A-Za-z0-9_-]{16}|[0-9a-f]{32})$"}
        },
        "required":["goal","steps"]
    })
}

pub fn execute_tasks_input_schema() -> Value {
    json!({
        "type":"object",
        "additionalProperties":false,
        "properties":{
            "project":{"type":"string","minLength":1,"description":"Exact or resolvable Runner-registered project id shared by every task."},
            "tasks":{
                "type":"array",
                "minItems":2,
                "maxItems":4,
                "items":task_batch_item_schema(),
                "description":"Two to four independent deterministic task plans admitted together so Chadex can schedule them concurrently. Each item keeps its own isolation, validation, apply-back, recovery, and task id."
            }
        },
        "required":["project","tasks"]
    })
}

fn task_identity_input_schema(description: &str) -> Value {
    json!({
        "type":"object",
        "additionalProperties":false,
        "description":description,
        "properties":{
            "project":{"type":"string","minLength":1},
            "task_id":{"type":"string","pattern":"^chadex_task_[0-9a-f]{32}$"}
        },
        "required":["project","task_id"]
    })
}

pub fn observe_task_input_schema() -> Value {
    task_identity_input_schema("Observe one exact process-local Chadex deterministic task.")
}

pub fn cancel_task_input_schema() -> Value {
    task_identity_input_schema("Cooperatively request cancellation of one exact Chadex deterministic task between bounded steps.")
}

pub fn task_recovery_input_schema() -> Value {
    json!({
        "type":"object",
        "additionalProperties":false,
        "properties":{
            "project":{"type":"string","minLength":1,"description":"Exact or resolvable Runner-registered source project id."},
            "action":{"type":"string","enum":["list","retry","discard"],"description":"List recovered tasks, retry one from its persisted deterministic plan, or discard one and clean its preserved worktrees."},
            "task_id":{"type":"string","pattern":"^chadex_task_[0-9a-f]{32}$","description":"Required for retry/discard; omit for list."}
        },
        "required":["project","action"]
    })
}
