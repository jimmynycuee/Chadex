use super::RunnerCapabilityRequirement::OwnerOnly;
use super::ToolVisibility::ModelVisible;
use super::{
    def, model_spec, permission_risk, require_all_scopes, ToolDefinition, PERMISSION_RISK_JOB,
};
use crate::metadata::{
    ToolPathHint::None as NoPath,
    ToolRisk::{JobRun, Read},
    JOB_RUN, PROJECT_READ, PROJECT_WRITE, TOOL_PROVIDER_CONTROL,
};
use crate::registry::input_schemas::{
    cancel_task_input_schema, execute_task_input_schema, execute_tasks_input_schema,
    observe_task_input_schema, task_recovery_input_schema,
};

const TASK_AUDIT_FIELDS: &[super::ToolAuditResultField] = &[
    super::ToolAuditResultField::value("task_id"),
    super::ToolAuditResultField::value("execution_id"),
    super::ToolAuditResultField::value("execution_state"),
    super::ToolAuditResultField::value("previous_execution_id"),
    super::ToolAuditResultField::value("project"),
    super::ToolAuditResultField::value("status"),
    super::ToolAuditResultField::value("completed_steps"),
    super::ToolAuditResultField::value("total_steps"),
    super::ToolAuditResultField::value("cancel_requested"),
    super::ToolAuditResultField::pointer(
        "requested_package_count",
        "/packaging/requested_package_count",
    ),
    super::ToolAuditResultField::pointer(
        "effective_package_count",
        "/packaging/effective_package_count",
    ),
    super::ToolAuditResultField::pointer("graphify_status", "/packaging/graphify_status"),
    super::ToolAuditResultField::pointer("graphify_adjusted", "/packaging/graphify_adjusted"),
    super::ToolAuditResultField::pointer("dependency_proof", "/packaging/dependency_proof"),
    super::ToolAuditResultField::pointer(
        "package_parallelism_proven",
        "/packaging/package_parallelism_proven",
    ),
    super::ToolAuditResultField::pointer("workspace_mode", "/workspace/mode"),
    super::ToolAuditResultField::pointer("workspace_state", "/workspace/state"),
    super::ToolAuditResultField::pointer("failure_kind", "/failure/kind"),
];

const BATCH_TASK_AUDIT_FIELDS: &[super::ToolAuditResultField] = &[
    super::ToolAuditResultField::value("project"),
    super::ToolAuditResultField::value("requested_count"),
    super::ToolAuditResultField::value("succeeded_count"),
    super::ToolAuditResultField::value("failed_count"),
    super::ToolAuditResultField::value("duration_ms"),
];

pub(super) const DEFINITIONS: &[ToolDefinition] = &[
    permission_risk(
        model_spec(
            require_all_scopes(
                def(
                    "execute_task",
                    super::ToolAuditPolicy::typed_fields(TASK_AUDIT_FIELDS).session_input(
                        super::ToolAuditSessionInputPolicy::OmitTopLevel(&["goal", "steps"]),
                    ),
                    ModelVisible,
                    "task_execution",
                    Some(OwnerOnly),
                    TOOL_PROVIDER_CONTROL,
                    super::ToolSemanticContract {
                        effect: super::ToolEffect::Execute,
                        risk: JobRun,
                        approval: super::ToolApprovalPolicy::Standard,
                        idempotency: super::ToolIdempotency::NonIdempotent,
                    },
                    Some(PROJECT_WRITE),
                    true,
                    NoPath,
                    true,
                    false,
                    super::ToolSessionEvidencePolicy::NONE,
                ),
                &[PROJECT_WRITE, JOB_RUN],
            ),
            "Combine already-decided edit/validate/review steps in one call; no LLM replanning. Inspect first for result-dependent decisions. Use direct tools if isolation cost dominates or source revision fencing is needed; revisions are Project-scoped. Stable task_id prevents replay; omission creates a new execution. execution_id equals task_id; execution_state is canonical, terminal states immutable, succeeded durable. Never retry unknown outcomes. Cancellation does not prove termination. Mutations/processes use detached managed worktrees. Parallelism requires dependency proof and estimated gain; small tasks stay sequential. Apply-back retains write-set overlap, isolated reconciliation, strict source CAS, permission/path guards, validation and review. Recovery retry links previous_execution_id. Reuse returned validation/review. No permanent branch, push or deploy.",
            execute_task_input_schema,
        ),
        PERMISSION_RISK_JOB,
    ),
    permission_risk(
        model_spec(
            require_all_scopes(
                def(
                    "execute_tasks",
                    super::ToolAuditPolicy::typed_fields(BATCH_TASK_AUDIT_FIELDS).session_input(
                        super::ToolAuditSessionInputPolicy::OmitTopLevel(&["tasks"]),
                    ),
                    ModelVisible,
                    "task_execution",
                    Some(OwnerOnly),
                    TOOL_PROVIDER_CONTROL,
                    super::ToolSemanticContract {
                        effect: super::ToolEffect::Execute,
                        risk: JobRun,
                        approval: super::ToolApprovalPolicy::Standard,
                        idempotency: super::ToolIdempotency::NonIdempotent,
                    },
                    Some(PROJECT_WRITE),
                    true,
                    NoPath,
                    true,
                    false,
                    super::ToolSessionEvidencePolicy::NONE,
                ),
                &[PROJECT_WRITE, JOB_RUN],
            ),
            "Submit two to four independent deterministic Chadex task plans in one request. Runtime dispatches them concurrently through the same bounded task/executor scheduler; each task retains its own worktree isolation, write-set-aware apply-back, validation, recovery, and failure state. Use this when the tasks are independently useful and do not need ordered cross-task state. Never use batch submission to bypass package dependency planning inside one task.",
            execute_tasks_input_schema,
        ),
        PERMISSION_RISK_JOB,
    ),
    model_spec(
        def(
            "observe_task",
            super::ToolAuditPolicy::typed_fields(TASK_AUDIT_FIELDS),
            ModelVisible,
            "task_execution",
            Some(OwnerOnly),
            TOOL_PROVIDER_CONTROL,
            super::ToolSemanticContract {
                effect: super::ToolEffect::Observe,
                risk: Read,
                approval: super::ToolApprovalPolicy::None,
                idempotency: super::ToolIdempotency::PureRead,
            },
            Some(PROJECT_READ),
            true,
            NoPath,
            false,
            false,
            super::ToolSessionEvidencePolicy::NONE,
        ),
        "Read the compact process-local state of one exact Chadex deterministic task. Observation never starts, resumes, retries, or changes task work.",
        observe_task_input_schema,
    ),
    permission_risk(
        model_spec(
            require_all_scopes(
                def(
                    "cancel_task",
                    super::ToolAuditPolicy::typed_fields(TASK_AUDIT_FIELDS),
                    ModelVisible,
                    "task_execution",
                    Some(OwnerOnly),
                    TOOL_PROVIDER_CONTROL,
                    super::ToolSemanticContract {
                        effect: super::ToolEffect::Mutate,
                        risk: JobRun,
                        approval: super::ToolApprovalPolicy::Standard,
                        idempotency: super::ToolIdempotency::DesiredState,
                    },
                    Some(PROJECT_WRITE),
                    true,
                    NoPath,
                    true,
                    false,
                    super::ToolSessionEvidencePolicy::NONE,
                ),
                &[PROJECT_WRITE, JOB_RUN],
            ),
            "Request cooperative cancellation of one exact Chadex deterministic task. Cancellation is checked between bounded steps; an already-dispatched mutation is never reported as cancelled until its authoritative result is known.",
            cancel_task_input_schema,
        ),
        PERMISSION_RISK_JOB,
    ),
    permission_risk(
        model_spec(
            require_all_scopes(
                def(
                    "task_recovery",
                    super::ToolAuditPolicy::typed_fields(TASK_AUDIT_FIELDS),
                    ModelVisible,
                    "task_execution",
                    Some(OwnerOnly),
                    TOOL_PROVIDER_CONTROL,
                    super::ToolSemanticContract {
                        effect: super::ToolEffect::Execute,
                        risk: JobRun,
                        approval: super::ToolApprovalPolicy::Standard,
                        idempotency: super::ToolIdempotency::DesiredState,
                    },
                    Some(PROJECT_WRITE),
                    true,
                    NoPath,
                    true,
                    false,
                    super::ToolSessionEvidencePolicy::NONE,
                ),
                &[PROJECT_WRITE, JOB_RUN],
            ),
            "Recover deterministic Chadex tasks persisted across runtime restart. `list` reconciles persisted task/worktree metadata, `retry` replays the exact persisted deterministic plan as a new task, and `discard` safely unregisters/removes preserved worktrees before deleting recovery state. Unknown executions cannot be retried. Retry creates a new execution_id with previous_execution_id. Never deletes a worktree without matching ownership metadata.",
            task_recovery_input_schema,
        ),
        PERMISSION_RISK_JOB,
    ),
];
