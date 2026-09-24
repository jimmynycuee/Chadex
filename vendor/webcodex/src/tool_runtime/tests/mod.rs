//! Domain-organized test modules for tool_runtime.

mod support;

mod agent_tasks;
mod agent_waits;
mod apply_text_edits;
mod assignment_fence;
mod builtin_coding_workflow;
mod chadex_task_executor;
mod changes;
#[cfg(feature = "workspace-checkpoints")]
mod checkpoint;
#[cfg(feature = "experimental-code-mode")]
mod code_mode;
#[cfg(feature = "experimental-code-mode")]
mod code_mode_e2b;
mod coding_task;
mod coding_task_semantic_navigation;
mod collaboration;
mod computer_budgets;
mod context_projection;
mod continuation_feedback;
mod diagnostics;
mod dispatch;
mod edit_tool_telemetry;
mod execution_context;
mod files;
mod files_helpers;
mod git;
mod goals;
mod handoff;
mod handoff_brief;
mod hygiene;
mod jobs;
mod lsp;
mod memory;
mod metadata;
mod observe_jobs;
mod permission_gate;
mod process;
mod read_files;
mod reconnect;
mod schema;
mod script;
mod search_project_texts;
mod session_shells;
mod sessions;
mod sessions_git;
mod sessions_guards;
mod sessions_instructions;
mod sessions_resolver;
mod skills;
mod specialized_dispatch;
mod startup_brief;
mod startup_catalog;
mod sync_timeout;
mod targeted_inventory;
mod trusted_smoke;
mod unified_diff;
mod validation_events;
mod validation_handoff;
mod validation_identity;
mod validation_summary;
mod work_on_project;
mod work_result;
mod write_project_file;
