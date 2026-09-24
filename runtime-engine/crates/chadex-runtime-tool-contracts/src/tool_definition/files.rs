use super::RunnerCapabilityRequirement::{FileRead, Shell};
use super::ToolVisibility::ModelVisible;
use super::{
    adaptive_runtime_direct, context_reobservable, def, model_spec, ToolDefinition,
    TOOL_CATEGORY_FILE, TOOL_CATEGORY_PROJECT,
};
use crate::metadata::{
    ToolPathHint::None as NoPath, ToolRisk::Read, PROJECT_READ, TOOL_PROVIDER_RUNNER,
};
use crate::registry::input_schemas::{
    list_project_files_input_schema, list_project_tracked_files_input_schema,
    project_overview_input_schema, read_files_input_schema, search_project_texts_input_schema,
};

pub(super) const SEARCH_DEFINITIONS: &[ToolDefinition] = &[
    context_reobservable(model_spec(
        def(
            "project_overview",
            super::ToolAuditPolicy::TYPED_CANONICAL,
            ModelVisible,
            TOOL_CATEGORY_PROJECT,
            Some(FileRead),
            TOOL_PROVIDER_RUNNER,
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
            super::ToolSessionEvidencePolicy::NONE.review(super::ToolReviewEvidence::ReadOnlyInspection),
        ).with_composition_policy(super::ToolCompositionPolicy::Parallel),
        "Deterministic, bounded, metadata-only overview of an unfamiliar project: conventional project types, manifests, key files, roots, and direct children. Reads no file contents, uses no LLM, and is not semantic/LSP analysis; use read_files for contents.",
        project_overview_input_schema,
    )),
    context_reobservable(model_spec(
        def(
            "list_project_files",
            super::ToolAuditPolicy::TYPED_CANONICAL,
            ModelVisible,
            TOOL_CATEGORY_FILE,
            Some(FileRead),
            TOOL_PROVIDER_RUNNER,
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
            super::ToolSessionEvidencePolicy::NONE.review(super::ToolReviewEvidence::ReadOnlyInspection),
        ),
        "List one deterministic page of files in a Runner-registered project directory (bounded, read-only). Entries are sorted before offset/limit slicing; use next_offset until null. Successful paging is exposed only when the complete directory source reached the Server—retained-tail truncation fails closed instead of inventing total_entries or a safe continuation. Returns project-relative paths plus a file/dir kind. Routed to the owning registered Runner; the server never reads the Runner project path directly.",
        list_project_files_input_schema,
    )),
    context_reobservable(model_spec(
        def(
            "list_project_tracked_files",
            super::ToolAuditPolicy::TYPED_CANONICAL,
            ModelVisible,
            TOOL_CATEGORY_FILE,
            // Runs `git ls-files` on the Runner, so the shell capability is what
            // the Runner must actually hold — not FileRead's directory op.
            Some(Shell),
            TOOL_PROVIDER_RUNNER,
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
        ).with_composition_policy(super::ToolCompositionPolicy::Parallel),
        "Default discovery tool: what files does this project contain? Lists Git-tracked paths from a bounded producer source, so ignored directories like .venv and target never appear. Supports globs, a project-relative path scope, rollup, and offset paging when source acquisition is complete. If list_truncated=true, the source itself is incomplete: next_offset is null and offset must not be treated as recovery for the full repository; narrow path and retry. Retained-tail source truncation fails closed rather than exposing a false continuation.",
        list_project_tracked_files_input_schema,
    )),
    adaptive_runtime_direct(
        context_reobservable(model_spec(
            def(
                "search_project_texts",
                super::ToolAuditPolicy::TYPED_CANONICAL
                    .session_input(super::ToolAuditSessionInputPolicy::SearchProjectTexts),
                ModelVisible,
                TOOL_CATEGORY_FILE,
                Some(Shell),
                TOOL_PROVIDER_RUNNER,
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
                super::ToolSessionEvidencePolicy::NONE.review(super::ToolReviewEvidence::Search).exploration(super::ToolExplorationEvidence::SearchBatch),
            ).with_composition_policy(super::ToolCompositionPolicy::Parallel),
            "Batch-capable project-text search for 1..8 independent queries when bounded structured results, protected-path policy, isolated failures, or portable Runtime search semantics help. Runs at most two Runner requests in flight. For broad discovery prefer files_with_matches/count or a small bounded match set with little context, then target read_files/native reads. For one small known-scope search, native rg via run_process or a shell command is first-class. Batch only queries already known to be needed; keep result-dependent follow-ups sequential. Queries default to regex; prefer pattern_mode=literal for exact text and request context explicitly. Whole-query continuation uses one parser-ready suggested_call when the remaining batch fits the model result budget; otherwise Runtime truncates without a raw cursor or fake call. A truncated query has no safe match cursor and should be refined.",
            search_project_texts_input_schema,
        ).with_gpt_action_description("Batch-search 1..8 predetermined independent queries. For broad discovery use files/count or small low-context matches, then targeted reads; keep result-dependent follow-ups sequential. Known-scope native rg is first-class. Follow returned batch continuation; never cursor-guess truncation.")),
        40,
    ),
];

pub(super) const READ_DEFINITIONS: &[ToolDefinition] = &[
    adaptive_runtime_direct(
        context_reobservable(model_spec(
            def(
                "read_files",
                super::ToolAuditPolicy::TYPED_CANONICAL,
                ModelVisible,
                TOOL_CATEGORY_FILE,
                Some(FileRead),
                TOOL_PROVIDER_RUNNER,
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
                super::ToolSessionEvidencePolicy::NONE.review(super::ToolReviewEvidence::ReadOnlyInspection).exploration(super::ToolExplorationEvidence::ReadBatch),
            ).with_composition_policy(super::ToolCompositionPolicy::Parallel),
            "Batch/snapshot-aware project inspect: batch related ranges already known to be needed in one items array. Keeps protected-path policy, range normalization and snapshot-bound continuation. If the target symbol/test/implementation region is known, use bounded targeted ranges; do not read an entire large file merely because the budget permits it. A small known one-off observation without downstream snapshot dependency may use native file commands. Complete reads omit read_revision unless include_read_revision=true; request it for guarded edits. Partial reads carry read_revision; continued ranges are fenced and Runtime rejects a continuation if the snapshot changed. Binds exact resolved Project and business session_id. Zero progress may suggest more max_result_bytes; at 512 KiB, no fake continuation.",
            read_files_input_schema,
        ).with_gpt_action_description("Batch-read 1..8 UTF-8 ranges. Complete reads omit read_revision by default; request it before guarded edits. Partial reads always return a revision-fenced continuation and changed snapshots fail closed.")),
        50,
    ),
];
