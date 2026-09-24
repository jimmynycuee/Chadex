use serde_json::Value;
use std::collections::HashMap;
use std::sync::OnceLock;

mod agent_tasks;
mod agent_waits;
mod artifacts;
mod chadex_tasks;
#[cfg(feature = "workspace-checkpoints")]
mod checkpoints;
#[cfg(feature = "experimental-code-mode")]
mod code_mode;
mod coding_agents;
mod coding_tasks;
mod common;
mod communication;
mod computer;
mod discovery;
mod edits;
mod files;
mod git;
mod goals;
mod hygiene;
mod jobs;
mod lsp;
mod memory;
mod projects;
mod runner_config;
mod sessions;
mod skills;
mod ssh_resources;
mod testing;

use common::default_output_schema;
pub use common::{
    continuation_semantics_schema, suggested_tool_call_schema, suggested_tool_call_schema_target,
};

/// Return a caller-owned copy of the immutable canonical output contract.
///
/// Output schemas are also used after every tool call to project
/// model-facing recovery/suggested-call values. Building the complete schema
/// DOM on every response made that hot path pay the registry construction cost
/// repeatedly. The cache contains only static contract values; callers still
/// receive a clone so adapter-specific projection can mutate it safely.
pub fn output_schema_for_tool(name: &str) -> Value {
    static SCHEMAS: OnceLock<HashMap<String, Value>> = OnceLock::new();
    SCHEMAS
        .get_or_init(|| {
            let mut schemas = HashMap::new();
            for definition in super::super::tool_definition::tool_definitions() {
                schemas.insert(
                    definition.name.to_string(),
                    uncached_output_schema_for_tool(definition.name),
                );
            }
            schemas
        })
        .get(name)
        .cloned()
        .unwrap_or_else(default_output_schema)
}

fn uncached_output_schema_for_tool(name: &str) -> Value {
    if let Some(schema) = chadex_tasks::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = agent_tasks::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = agent_waits::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = goals::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = coding_agents::output_schema_for_tool(name) {
        return schema;
    }
    #[cfg(feature = "experimental-code-mode")]
    if let Some(schema) = code_mode::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = computer::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = communication::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = jobs::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = discovery::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = projects::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = runner_config::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = coding_tasks::output_schema_for_tool(name) {
        return schema;
    }
    #[cfg(feature = "workspace-checkpoints")]
    if let Some(schema) = checkpoints::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = artifacts::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = git::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = edits::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = sessions::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = memory::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = skills::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = hygiene::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = files::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = lsp::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = ssh_resources::output_schema_for_tool(name) {
        return schema;
    }
    if let Some(schema) = testing::output_schema_for_tool(name) {
        return schema;
    }

    default_output_schema()
}

#[cfg(any(test, feature = "root-test-support"))]
pub fn coding_workflow_diagnostic_output_schema_for_test() -> Value {
    coding_tasks::coding_workflow_diagnostic_output_schema()
}
