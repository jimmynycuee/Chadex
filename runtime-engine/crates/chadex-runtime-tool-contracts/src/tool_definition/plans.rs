use super::ToolVisibility::{ModelHidden, ModelVisible};
use super::{
    adaptive_runtime_direct, def, model_spec, require_all_scopes, ToolDefinition, ToolEffect,
    ToolIdempotency, ToolVisibility, TOOL_CATEGORY_GOAL,
};
use crate::metadata::{
    ToolPathHint::None as NoPath,
    ToolRisk::{self, JobRun, Read, WorkflowManage},
    JOB_RUN, PROJECT_READ, PROJECT_WRITE, RUNTIME_READ, TOOL_PROVIDER_CONTROL,
};
use crate::registry::input_schemas::{
    goal_operator_input_schema, goal_run_control_input_schema, goal_run_list_input_schema,
    goal_check_input_schema, goal_run_start_input_schema, plan_wait_approval_input_schema, goal_run_status_input_schema,
    plan_get_input_schema,
    plan_list_input_schema, plan_review_input_schema, plan_save_input_schema,
    plan_start_input_schema,
};

const PLAN_AUDIT_FIELDS: &[super::ToolAuditResultField] = &[
    super::ToolAuditResultField::value("error_kind"),
    super::ToolAuditResultField::value("created"),
];

const GOAL_RUN_START_SCOPES: &[&str] = &[PROJECT_READ, PROJECT_WRITE, JOB_RUN];

const fn plan_def(
    name: &'static str,
    visibility: ToolVisibility,
    effect: ToolEffect,
    risk: ToolRisk,
    idempotency: ToolIdempotency,
    scope: &'static str,
) -> ToolDefinition {
    plan_def_for(name, visibility, effect, risk, idempotency, scope, false)
}

const fn plan_def_for(
    name: &'static str,
    visibility: ToolVisibility,
    effect: ToolEffect,
    risk: ToolRisk,
    idempotency: ToolIdempotency,
    scope: &'static str,
    requires_project: bool,
) -> ToolDefinition {
    def(
        name,
        super::ToolAuditPolicy::typed_fields(PLAN_AUDIT_FIELDS),
        visibility,
        TOOL_CATEGORY_GOAL,
        None,
        TOOL_PROVIDER_CONTROL,
        super::ToolSemanticContract {
            effect,
            risk,
            approval: super::ToolApprovalPolicy::None,
            idempotency,
        },
        Some(scope),
        requires_project,
        NoPath,
        false,
        false,
        super::ToolSessionEvidencePolicy::NONE,
    )
}

pub(super) const DEFINITIONS: &[ToolDefinition] = &[
    adaptive_runtime_direct(
        model_spec(
            plan_def_for(
                "plan_start",
                ModelVisible,
                ToolEffect::Mutate,
                WorkflowManage,
                ToolIdempotency::Keyed,
                PROJECT_READ,
                true,
            ),
            "Enter Chadex Plan Mode for one project. Use when the user asks to plan first / not change code yet / 'Chadex Plan Mode' / @plan. From this call until the user approves or exits in the Chadex app, the Chadex backend makes this connection read-only: only reviewed read tools (files, search, Git history, Memory, Skills) work and every write, shell, job, Git write, Computer Use, Plugin, MCP gateway or agent call is refused. Idempotent for the same project. Writes only Chadex's own plan store.",
            plan_start_input_schema,
        )
        .with_gpt_action_description("Enter backend-enforced read-only Plan Mode for a project before planning work."),
        61,
    ),
    adaptive_runtime_direct(
        model_spec(
            plan_def(
                "plan_save",
                ModelVisible,
                ToolEffect::Mutate,
                WorkflowManage,
                ToolIdempotency::Keyed,
                PROJECT_READ,
            ),
            "Save a new immutable plan revision (Markdown) with objective acceptance checks (literal argv run in the project, exit 0 = pass) and an optional Goal budget. parent_revision must equal the current latest revision (0 for the first save). Every save creates revision N+1 with a content_sha256; earlier revisions and any approval of them are kept but no longer approvable. Ground the plan in what you actually read. Then ask the user to review and approve that exact revision in the Chadex app.",
            plan_save_input_schema,
        )
        .with_gpt_action_description("Save the next immutable plan revision with acceptance checks and budget."),
        63,
    ),
    model_spec(
        plan_def(
            "plan_get",
            ModelVisible,
            ToolEffect::Observe,
            Read,
            ToolIdempotency::PureRead,
            PROJECT_READ,
        ),
        "Read one of your plans: a revision's content, acceptance checks, budget and hash, the revision history, review decisions (approved / changes requested / revoked / superseded), and optionally a line diff from another revision (diff_from).",
        plan_get_input_schema,
    ),
    model_spec(
        plan_def(
            "plan_list",
            ModelVisible,
            ToolEffect::Observe,
            Read,
            ToolIdempotency::PureRead,
            PROJECT_READ,
        ),
        "List your recent plans and whether Plan Mode is active.",
        plan_list_input_schema,
    ),
    require_all_scopes(
        adaptive_runtime_direct(
            model_spec(
                plan_def(
                    "goal_run_start",
                    ModelVisible,
                    ToolEffect::Execute,
                    JobRun,
                    ToolIdempotency::Keyed,
                    PROJECT_WRITE,
                ),
                "Chadex Goal Mode: work until objective acceptance checks pass. Send a goal with project, title, objective and acceptance (argv commands; exit 0 = done). You cannot start it: the user starts it in the Chadex app (Plan & Goal) and picks the mode: Codex (Chadex runs Codex CLI in the background, uses their Codex tokens, ChatGPT can be closed) or ChatGPT (you edit the Goal's workspace project here and call goal_check until achieved). Calling with plan_id returns the run once started. Use when the user gives a goal / 'Chadex Goal Mode' / @goal. Not for implementing an approved Plan Mode plan (use plan_wait_approval). The result is a branch for the user to review; nothing is merged, pushed or deployed. ChatGPT is not notified when a Codex-mode Goal ends: Chadex notifies the user on the Mac.",
                goal_run_start_input_schema,
            )
            .with_gpt_action_description("Send a direct Goal request (the user starts it in the Chadex app) or find a started Goal."),
            62,
        ),
        GOAL_RUN_START_SCOPES,
    ),
    adaptive_runtime_direct(
        model_spec(
            plan_def(
                "goal_run_status",
                ModelVisible,
                ToolEffect::Observe,
                Read,
                ToolIdempotency::PureRead,
                PROJECT_READ,
            ),
            "Read a Goal run's state (queued/running/paused/needs_attention/succeeded/failed/cancelled/stopped), reason, iteration, acceptance evidence per iteration, journaled operations, recent audit events, result branch, and reported token usage (or unknown). Pass wait_secs (up to 45) to wait for the run to finish before answering. Use goal_run_list to find a run started from the Chadex app.",
            goal_run_status_input_schema,
        )
        .with_gpt_action_description("Read Goal run progress, acceptance evidence and state."),
        64,
    ),
    model_spec(
        plan_def(
            "plan_wait_approval",
            ModelVisible,
            ToolEffect::Observe,
            Read,
            ToolIdempotency::PureRead,
            PROJECT_READ,
        ),
        "After plan_save, keep calling this in the same reply while the user reviews the plan in the Chadex app. Returns approved (Plan Mode has ended: implement the approved revision right away in the project with your normal tools, then run its acceptance checks and report), changes_requested (revise with plan_save, then wait again), closed or approval_withdrawn (stop and tell the user), goal_started (the user started a Goal instead: do not edit; follow it with goal_run_status), or waiting (call again). Do not end your reply while it says waiting unless the user asked you to stop.",
        plan_wait_approval_input_schema,
    ),
    require_all_scopes(
        model_spec(
            plan_def(
                "goal_check",
                ModelVisible,
                ToolEffect::Execute,
                JobRun,
                ToolIdempotency::Keyed,
                PROJECT_WRITE,
            ),
            "ChatGPT-mode Goal: after you finish a round of edits in the Goal's workspace project (workspace_project in goal_run_status), call this. Chadex commits the round and runs the Goal's approved acceptance checks (sandboxed; you cannot change them). If achieved is false, read the failures, fix them in the same workspace project and call goal_check again in the same reply; keep going until achieved is true or the run stops. Never claim the goal is done without achieved: true. Calling again while checks are still running does not start another round.",
            goal_check_input_schema,
        )
        .with_gpt_action_description("Check a ChatGPT-mode Goal's acceptance after a round of edits."),
        GOAL_RUN_START_SCOPES,
    ),
    model_spec(
        plan_def(
            "goal_run_list",
            ModelVisible,
            ToolEffect::Observe,
            Read,
            ToolIdempotency::PureRead,
            PROJECT_READ,
        ),
        "List your recent Goal runs.",
        goal_run_list_input_schema,
    ),
    model_spec(
        plan_def(
            "goal_run_control",
            ModelVisible,
            ToolEffect::Mutate,
            WorkflowManage,
            ToolIdempotency::NonIdempotent,
            PROJECT_READ,
        ),
        "Pause, resume (only a run the user paused), or cancel one of your Goal runs. A running agent job is stopped at once and its result recorded. Runs that need attention (unknown outcome, permission denial, no progress) can only be resumed from the Chadex app after review.",
        goal_run_control_input_schema,
    ),
    model_spec(
        plan_def(
            "plan_review",
            ModelHidden,
            ToolEffect::Mutate,
            WorkflowManage,
            ToolIdempotency::NonIdempotent,
            RUNTIME_READ,
        ),
        "Operator-only (Chadex app over local operator REST): list, inspect, approve, approve_and_start, reject (request changes), revoke, or close plans. Approval binds the exact revision and content hash, the project's Runner and root, and its Git HEAD.",
        plan_review_input_schema,
    ),
    model_spec(
        plan_def(
            "goal_operator",
            ModelHidden,
            ToolEffect::Mutate,
            WorkflowManage,
            ToolIdempotency::NonIdempotent,
            RUNTIME_READ,
        ),
        "Operator-only (Chadex app over local operator REST): Goal overview, status, pause/resume/cancel any run, dismiss a finished run from the lists, clear the board (close open plans, hide finished runs; running Goals continue), and the persistent emergency stop.",
        goal_operator_input_schema,
    ),
];
