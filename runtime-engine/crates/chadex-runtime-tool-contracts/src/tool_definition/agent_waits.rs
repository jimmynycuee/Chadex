use super::ToolVisibility::{ModelHidden, ModelVisible};
use super::{
    adaptive_runtime_direct, def, model_spec, permission_risk, require_all_scopes, ToolDefinition,
    PERMISSION_RISK_WRITE, TOOL_CATEGORY_AGENT_WAIT,
};
use crate::metadata::{
    ToolPathHint::None as NoPath,
    ToolRisk::{Read, WorkflowManage},
    COMMUNICATION_MANAGE, COMMUNICATION_READ, TOOL_PROVIDER_CONTROL,
};
use crate::registry::input_schemas::{
    cancel_agent_wait_input_schema, read_agent_wait_input_schema,
    wait_for_agent_events_input_schema,
};
use webcodex_core::authority::{COMMUNICATION_MANAGE_SCOPES, COMMUNICATION_READ_SCOPES};

pub(super) const DEFINITIONS: &[ToolDefinition] = &[
    require_all_scopes(
        adaptive_runtime_direct(
            permission_risk(
                model_spec(
                    def(
                        "wait_for_agent_events",
                    super::ToolAuditPolicy::typed_fields(&[
                        super::ToolAuditResultField::pointer("wait_id", "/agent_wait/wait_id"),
                        super::ToolAuditResultField::pointer("state", "/agent_wait/state"),
                        super::ToolAuditResultField::pointer("revision", "/agent_wait/revision"),
                        super::ToolAuditResultField::pointer("match_count", "/agent_wait/match_count"),
                        super::ToolAuditResultField::value("replayed"),
                        super::ToolAuditResultField::value("state_changed"),
                        super::ToolAuditResultField::value("error_kind"),
                    ]),
                    ModelVisible,
                    TOOL_CATEGORY_AGENT_WAIT,
                    None,
                    TOOL_PROVIDER_CONTROL,
                    super::ToolSemanticContract {
                        effect: super::ToolEffect::Mutate,
                        risk: WorkflowManage,
                        approval: super::ToolApprovalPolicy::Standard,
                        idempotency: super::ToolIdempotency::Keyed,
                    },
                    Some(COMMUNICATION_MANAGE),
                    false,
                    NoPath,
                    false,
                    false,
                    super::ToolSessionEvidencePolicy::NONE,
                    ),
                    "Create one caller-owned durable one-shot AgentWait over 1..8 exact agent_task_terminal selectors with fixed ANY semantics. Re-authorizes the exact Agent, current Endpoint generation, and every source Task before atomically registering sources and already-terminal snapshots. The Endpoint is presentation only, not durable Wait ownership. First match creates one existing-carrier Wake; further matches coalesce only while that Wake is pending/claimed. Exact keyed replay returns the same Wait.",
                    wait_for_agent_events_input_schema,
                ).with_gpt_action_description("Create one durable one-shot AgentWait over exact AgentTask terminal selectors. ANY semantics; first match creates/coalesces the existing-carrier Wake. Exact idempotency replay returns the same Wait."),
                PERMISSION_RISK_WRITE,
            ),
            21,
        ),
        COMMUNICATION_MANAGE_SCOPES,
    ),
    require_all_scopes(
        model_spec(
            def(
                "read_agent_wait",
                super::ToolAuditPolicy::typed_fields(&[
                    super::ToolAuditResultField::pointer("wait_id", "/agent_wait/wait_id"),
                    super::ToolAuditResultField::pointer("state", "/agent_wait/state"),
                    super::ToolAuditResultField::pointer("revision", "/agent_wait/revision"),
                    super::ToolAuditResultField::pointer("match_count", "/agent_wait/match_count"),
                    super::ToolAuditResultField::value("error_kind"),
                ]),
                ModelVisible,
                TOOL_CATEGORY_AGENT_WAIT,
                None,
                TOOL_PROVIDER_CONTROL,
                super::ToolSemanticContract { effect: super::ToolEffect::Observe, risk: Read, approval: super::ToolApprovalPolicy::None, idempotency: super::ToolIdempotency::PureRead },
                Some(COMMUNICATION_READ), false, NoPath, false, false, super::ToolSessionEvidencePolicy::NONE,
            ),
            "Read one exact caller-owned AgentWait: wait_id, state, and matching Task/Attempt identities with terminal states when present. The result contains no source Task instruction/result/reason/log/fence/token and grants no Task, Project, Goal, Session, or execution authority; re-read source domains independently.",
            read_agent_wait_input_schema,
        ),
        COMMUNICATION_READ_SCOPES,
    ),
    require_all_scopes(
        permission_risk(
            model_spec(
                def(
                    "cancel_agent_wait",
                    super::ToolAuditPolicy::typed_fields(&[
                        super::ToolAuditResultField::pointer("wait_id", "/agent_wait/wait_id"),
                        super::ToolAuditResultField::pointer("state", "/agent_wait/state"),
                        super::ToolAuditResultField::value("replayed"),
                        super::ToolAuditResultField::value("state_changed"),
                        super::ToolAuditResultField::value("error_kind"),
                    ]),
                    ModelVisible,
                    TOOL_CATEGORY_AGENT_WAIT,
                    None,
                    TOOL_PROVIDER_CONTROL,
                    super::ToolSemanticContract { effect: super::ToolEffect::Mutate, risk: WorkflowManage, approval: super::ToolApprovalPolicy::Standard, idempotency: super::ToolIdempotency::Keyed },
                    Some(COMMUNICATION_MANAGE), false, NoPath, false, false, super::ToolSessionEvidencePolicy::NONE,
                ),
                "Cancel an exact one-shot AgentWait only while waiting or while its Wait-origin Wake remains pre-dispatch pending/claimed. A claimed Wake is safely revoked/retired. Once Host dispatch preparation has occurred, cancellation fails closed because delivery cannot truthfully be withdrawn. Resumed/cancelled Waits are immutable.",
                cancel_agent_wait_input_schema,
            ),
            PERMISSION_RISK_WRITE,
        ),
        COMMUNICATION_MANAGE_SCOPES,
    ),
    require_all_scopes(
        def(
            "agent_wait_state",
            super::ToolAuditPolicy::typed_fields(&[
                super::ToolAuditResultField::pointer("wait_id", "/agent_wait/wait_id"),
                super::ToolAuditResultField::pointer("state", "/agent_wait/state"),
                super::ToolAuditResultField::pointer("revision", "/agent_wait/revision"),
                super::ToolAuditResultField::pointer("match_count", "/agent_wait/match_count"),
                super::ToolAuditResultField::value("error_kind"),
            ]),
            ModelHidden,
            TOOL_CATEGORY_AGENT_WAIT,
            None,
            TOOL_PROVIDER_CONTROL,
            super::ToolSemanticContract { effect: super::ToolEffect::Observe, risk: Read, approval: super::ToolApprovalPolicy::None, idempotency: super::ToolIdempotency::PureRead },
            Some(COMMUNICATION_READ), false, NoPath, false, false, super::ToolSessionEvidencePolicy::NONE,
        )
        .with_activity(
            super::ToolActivityPresentation::Transport,
            super::ToolActivityInteraction::NonMeaningful,
        ),
        COMMUNICATION_READ_SCOPES,
    ),
];
