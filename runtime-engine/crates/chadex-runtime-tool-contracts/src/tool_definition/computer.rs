use super::RunnerCapabilityRequirement::FileWrite;
use super::ToolVisibility::ModelVisible;
use super::{
    adaptive_runtime_direct, def, model_spec, permission_risk, require_all_scopes,
    require_any_scopes, ToolDefinition, PERMISSION_RISK_WRITE, TOOL_CATEGORY_COMPUTER,
};
use crate::metadata::{
    ToolPathHint::{Artifact, None as NoPath},
    ToolRisk::{ComputerControl as ComputerControlRisk, ProjectWrite, Read},
    COMPUTER_CONTROL, COMPUTER_LAUNCH, COMPUTER_READ, PROJECT_WRITE, TOOL_PROVIDER_CONTROL,
};
use crate::registry::input_schemas::{
    computer_control_input_schema, computer_observe_input_schema,
    computer_save_snapshot_input_schema,
};

const COMPUTER_CONTROL_GATEWAY_SCOPES: &[&str] = &[COMPUTER_CONTROL, COMPUTER_LAUNCH];

pub(super) const DEFINITIONS: &[ToolDefinition] = &[
    adaptive_runtime_direct(
        model_spec(
            def(
                "computer_observe",
            super::ToolAuditPolicy::typed_semantic(
                super::ToolAuditSemanticResultPolicy::ComputerObservation,
            ),
            ModelVisible, TOOL_CATEGORY_COMPUTER, None, TOOL_PROVIDER_CONTROL,
            super::ToolSemanticContract { effect: super::ToolEffect::Observe, risk: Read, approval: super::ToolApprovalPolicy::None, idempotency: super::ToolIdempotency::PureRead },
            Some(COMPUTER_READ), false, NoPath, false, false, super::ToolSessionEvidencePolicy::NONE,
        ),
            "Read-only Computer observation. Use it whenever a task needs to see the user's desktop: a GUI app, System Settings, a browser page, or a running build. readiness reports non-prompting OS permission state; the closed action vocabulary covers targets, windows, displays, applications, Accessibility status/tree/search/state, window/display snapshots, and clipboard text. root_element_id scopes tree/find to a subtree; find value matches AXValue (never returned). MCP snapshots return native image content for model vision, not structured base64. Exact scopes and Runner capabilities are enforced before dispatch; ephemeral identities, stale-handle failure, bounds, and snapshot-generation semantics are unchanged. No action can activate, launch, focus, type, click, write the clipboard, save an artifact, use shell fallback, or retry an uncertain effect.",
            computer_observe_input_schema,
        )
        .with_gpt_action_unsupported(),
        57,
    ),
    adaptive_runtime_direct(
        require_any_scopes(
            permission_risk(
                model_spec(
                    def(
                        "computer_control",
                        super::ToolAuditPolicy::typed_semantic(
                            super::ToolAuditSemanticResultPolicy::ComputerControl,
                        ),
                        ModelVisible, TOOL_CATEGORY_COMPUTER, None, TOOL_PROVIDER_CONTROL,
                        super::ToolSemanticContract { effect: super::ToolEffect::Execute, risk: ComputerControlRisk, approval: super::ToolApprovalPolicy::Standard, idempotency: super::ToolIdempotency::NonIdempotent },
                        None, false, NoPath, true, false, super::ToolSessionEvidencePolicy::NONE,
                    ),
                    "Effectful Computer control (Observe -> Act -> Verify) for operating a desktop app no project tool or command can drive. Prefer fresh Accessibility identities from computer_observe(find_elements/element_state) for focus, input_text, press, and scroll_to_element; use pointer coordinates only from a fresh display snapshot_generation. Exact scopes and stale-handle rejection apply. activate_window raises the user's window: call it only right before key or pointer_move/pointer_click (macOS press/focus/input_text/scroll_to_element need none; Windows focus/input_text do). On a foreground error, activate once and retry; if the foreground keeps changing, the user is using the Mac: stop and ask instead. Re-observe after every effect; on outcome_unknown reconcile with computer_observe before any retry. No argv/path/script input, implicit activation, shell fallback, or blind retry.",
                    computer_control_input_schema,
                )
                .with_gpt_action_unsupported(),
                PERMISSION_RISK_WRITE,
            ),
            COMPUTER_CONTROL_GATEWAY_SCOPES,
        ),
        58,
    ),
    require_all_scopes(
        model_spec(
            def(
                "computer_save_snapshot",
                super::ToolAuditPolicy::typed_fields(&[
                    super::ToolAuditResultField::value("project"),
                    super::ToolAuditResultField::value("path"),
                    super::ToolAuditResultField::value("client_id"),
                    super::ToolAuditResultField::value("surface_id"),
                    super::ToolAuditResultField::value("source_width"),
                    super::ToolAuditResultField::value("source_height"),
                    super::ToolAuditResultField::presence("region_present", "region"),
                    super::ToolAuditResultField::value("width"),
                    super::ToolAuditResultField::value("height"),
                    super::ToolAuditResultField::value("mime_type"),
                    super::ToolAuditResultField::value("file_bytes"),
                    super::ToolAuditResultField::value("saved"),
                ]),
                ModelVisible,
                TOOL_CATEGORY_COMPUTER,
                Some(FileWrite),
                TOOL_PROVIDER_CONTROL,
                super::ToolSemanticContract {
                    effect: super::ToolEffect::Mutate,
                    risk: ProjectWrite,
                    approval: super::ToolApprovalPolicy::Standard,
                    idempotency: super::ToolIdempotency::NonIdempotent,
                },
                Some(PROJECT_WRITE),
                true,
                Artifact,
                false,
                false,
                super::ToolSessionEvidencePolicy::NONE,
            ),
            "Save one exact window snapshot as a create-only project artifact without returning image bytes. Reuses computer_observe(action=snapshot_window) region/downscale semantics and requires computer:read plus project:write. No overwrite or encoding control. Unknown writes require artifact-metadata reconciliation before retry.",
            computer_save_snapshot_input_schema,
        ),
        &[PROJECT_WRITE, COMPUTER_READ],
    ),
];
