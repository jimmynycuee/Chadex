//! Cross-surface runtime observations.
//!
//! Records the last successful *meaningful* tool call, scoped by principal,
//! project, surface, and session.
//!
//! Never stores tool arguments, output bodies, command text, or secrets.

use std::collections::VecDeque;
use std::sync::Mutex;

/// Bounded number of retained tool-call observations.
const MAX_TOOL_CALL_OBSERVATIONS: usize = 64;

/// One successful meaningful tool call. Scope fields only — no payloads.
#[derive(Debug, Clone)]
pub(crate) struct ToolCallObservation {
    pub(crate) principal_kind: String,
    pub(crate) principal_id: String,
    pub(crate) project: Option<String>,
    /// Calling surface, for example `api` or `mcp`.
    pub(crate) surface: String,
    pub(crate) session_id: Option<String>,
    pub(crate) tool: String,
    pub(crate) observed_at: i64,
}

#[derive(Debug, Default)]
pub(crate) struct RuntimeObservations {
    tool_calls: Mutex<VecDeque<ToolCallObservation>>,
}

impl RuntimeObservations {
    /// Record a successful tool call. Non-meaningful activity is rejected here
    /// so the canonical ToolDefinition interaction policy is enforced at the
    /// single recording funnel as a defensive backstop.
    pub(crate) fn record_successful_tool_call(&self, observation: ToolCallObservation) {
        if !webcodex_tool_contracts::runtime_tool_activity_interaction(&observation.tool)
            .is_meaningful()
        {
            return;
        }
        let mut calls = self.tool_calls.lock().expect("tool call observation lock");
        if calls.len() >= MAX_TOOL_CALL_OBSERVATIONS {
            calls.pop_front();
        }
        calls.push_back(observation);
    }

    /// Latest meaningful call for a specific principal (any project/surface).
    pub(crate) fn latest_tool_call_for_principal(
        &self,
        principal_kind: &str,
        principal_id: &str,
    ) -> Option<ToolCallObservation> {
        let calls = self.tool_calls.lock().expect("tool call observation lock");
        calls
            .iter()
            .rev()
            .find(|obs| obs.principal_kind == principal_kind && obs.principal_id == principal_id)
            .cloned()
    }

    /// Latest meaningful call across all principals.
    pub(crate) fn latest_tool_call(&self) -> Option<ToolCallObservation> {
        let calls = self.tool_calls.lock().expect("tool call observation lock");
        calls.back().cloned()
    }
}
