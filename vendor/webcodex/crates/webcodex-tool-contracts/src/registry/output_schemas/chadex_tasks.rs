use serde_json::{json, Value};

use super::common::{array_schema, open_object_schema, schema_type, wrapped_output_schema};

fn task_state_schema() -> Value {
    json!({
        "type":"string",
        "enum":["queued","running","cancelling","interrupted","completed","failed","failed_validation","blocked","cancelled","unknown"]
    })
}

fn task_output_schema() -> Value {
    wrapped_output_schema(vec![
        (
            "task_id",
            schema_type(
                "string",
                "Opaque durable Chadex execution id, also exposed as execution_id.",
            ),
        ),
        (
            "project",
            schema_type("string", "Exact resolved runtime Project id."),
        ),
        (
            "goal",
            schema_type(
                "string",
                "Bounded caller-supplied task goal; never model reasoning.",
            ),
        ),
        ("status", task_state_schema()),
        ("execution_id", schema_type("string", "Durable execution identity; equals task_id. Reuse rejects replay, never re-executes.")),
        ("previous_execution_id", schema_type("string", "Previous execution for an explicit retry; a retry always has a new identity.")),
        ("execution_state", json!({"type":"string", "enum":["queued","running","succeeded","failed","cancelled","interrupted","unknown"], "description":"Canonical execution state. Terminal states are immutable; unknown must not be retried."})),
        (
            "current_step",
            schema_type("integer", "Zero-based current or next step index."),
        ),
        (
            "total_steps",
            schema_type("integer", "Total deterministic plan step count."),
        ),
        (
            "completed_steps",
            schema_type("integer", "Successfully completed step count."),
        ),
        (
            "plan",
            array_schema(
                json!({
                    "type":"string",
                    "enum":["search","read","edit","run_process","validate","review"]
                }),
                "Bounded deterministic step-kind plan only; never arguments, paths, contents, logs, or model reasoning.",
            ),
        ),
        (
            "cancel_requested",
            schema_type(
                "boolean",
                "Whether cooperative cancellation has been requested.",
            ),
        ),
        (
            "started_at_ms",
            schema_type("integer", "Unix task start timestamp in milliseconds."),
        ),
        (
            "finished_at_ms",
            schema_type(
                "integer",
                "Unix terminal timestamp in milliseconds when terminal.",
            ),
        ),
        (
            "duration_ms",
            schema_type(
                "integer",
                "Elapsed task wall-clock milliseconds when known.",
            ),
        ),
        (
            "limits",
            open_object_schema("Normalized task-level safety and resource limits."),
        ),
        (
            "counters",
            open_object_schema("Bounded step/mutation/changed-file counters."),
        ),
        (
            "validation",
            open_object_schema("Compact validation pipeline result; never full stdout/stderr."),
        ),
        (
            "review",
            open_object_schema("Compact final workspace review facts; never full file contents."),
        ),
        (
            "complexity",
            open_object_schema("Deterministic task complexity score and bounded evidence. Prefer estimation_source=preplan, which is independent of execution-step shape and includes a legacy plan-shape shadow score for calibration. Its recommendation constrains logical package_count while execution_task_count remains 1."),
        ),
        (
            "packaging",
            open_object_schema("Adaptive logical package plan inside the single outer execute_task call. Includes requested/recommended/effective counts, safe validation boundaries, Graphify freshness/dependency evidence, dependency-driven merges, current package, and compact package ranges/status."),
        ),
        (
            "workspace",
            open_object_schema("Explicit execution workspace lifecycle. Mutation tasks use detached Runner-managed worktrees; read-only tasks use mode=none. Includes isolation mode, execution Project identity, lifecycle state, timing trace, and bounded source-change/reconciliation facts."),
        ),
        (
            "steps",
            array_schema(
                open_object_schema("Compact task step outcome."),
                "Bounded compact step summaries.",
            ),
        ),
        (
            "failure",
            open_object_schema("Bounded terminal failure classification and message when present."),
        ),
    ])
}

fn execute_tasks_output_schema() -> Value {
    wrapped_output_schema(vec![
        ("project", schema_type("string", "Exact resolved runtime Project id shared by the batch.")),
        ("requested_count", schema_type("integer", "Number of deterministic task items submitted.")),
        ("succeeded_count", schema_type("integer", "Number of task items that reached successful terminal results.")),
        ("failed_count", schema_type("integer", "Number of task items that returned failed or blocked terminal results.")),
        ("duration_ms", schema_type("integer", "Batch wall-clock duration in milliseconds.")),
        (
            "items",
            array_schema(
                open_object_schema("Indexed per-task result containing success, error, and the ordinary compact execute_task output."),
                "Per-task results in caller order. Each task remains independently observable/recoverable by its task_id.",
            ),
        ),
    ])
}

fn task_recovery_output_schema() -> Value {
    wrapped_output_schema(vec![
        ("action", schema_type("string", "Recovery action that was executed.")),
        ("project", schema_type("string", "Exact resolved source Project id.")),
        ("task_id", schema_type("string", "Recovered task id when one task was targeted.")),
        ("status", schema_type("string", "Recovery/discard status when applicable.")),
        (
            "recovered_tasks",
            array_schema(open_object_schema("Recovered interrupted/preserved task projection."), "Recovered task projections restored from private Chadex task state."),
        ),
        ("workspace_manifest_count", schema_type("integer", "Persisted workspace manifest count for this project.")),
        ("registered_workspace_count", schema_type("integer", "Persisted workspaces still registered with Git worktree state.")),
        ("orphan_managed_worktree_count", schema_type("integer", "Managed worktrees detected without matching recovery manifest; reported conservatively, never blindly deleted.")),
        ("workspace_count", schema_type("integer", "Workspace count cleaned by discard.")),
        ("state_changed", schema_type("boolean", "Whether recovery state was changed.")),
        ("recovery", open_object_schema("Retry provenance for a newly executed task.")),
    ])
}

pub(super) fn output_schema_for_tool(name: &str) -> Option<Value> {
    match name {
        "execute_task" | "observe_task" | "cancel_task" => Some(task_output_schema()),
        "execute_tasks" => Some(execute_tasks_output_schema()),
        "task_recovery" => Some(task_recovery_output_schema()),
        _ => None,
    }
}
