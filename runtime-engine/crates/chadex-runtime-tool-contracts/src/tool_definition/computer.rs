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
            "Guaranteed read-only Computer observation surface. Use readiness to inspect current non-prompting OS permission state before desktop/control work, or use the closed action vocabulary for targets, windows, displays, applications, Accessibility status/tree/search/state, window/display snapshots, or clipboard text. MCP snapshot actions return native image content/resources for model vision instead of structured base64. Exact action scopes and Runner capabilities are enforced before dispatch; opaque ephemeral identities, stale-handle failure, traversal/image/clipboard bounds, and snapshot-generation semantics remain unchanged. No action can activate, launch, focus, type, move/click the pointer, write the clipboard, save a project artifact, use shell fallback, or retry an uncertain effect.",
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
                    "Effectful Computer control for the Observe -> Act -> Verify loop. Prefer fresh semantic Accessibility identities from computer_observe(action=find_elements/element_state) for focus, input_text, press, and scroll_to_element; use pointer coordinates only as a fallback from a fresh display snapshot_generation. Each action keeps exact scopes, native validation, and stale-handle rejection. activate_window raises the user's window, so call it only right before key or pointer_move/pointer_click; on macOS press, focus, input_text, and scroll_to_element normally need no activation (on Windows focus and input_text do). On a frontmost/foreground error, activate once and retry. After every effect, re-observe. If execution_state is outcome_unknown, reconcile with computer_observe before any retry. No arbitrary argv/path/script input, implicit activation, shell fallback, or blind retry.",
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
