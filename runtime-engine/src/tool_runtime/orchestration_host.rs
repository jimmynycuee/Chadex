use super::context_projection::TOOL_CALL_CONTEXT_REQUEST_FIELD;
use super::kernel::{
    HostFileImportTrust, ToolCallContext, ToolCallErrorStatus, ToolCallRequest, ToolTransport,
};
use super::ToolRuntime;
use crate::auth::AuthContext;
use crate::json_measurement::serialized_json_len;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, Weak};
#[cfg(test)]
use tokio::sync::Semaphore;
use tokio::sync::{
    Mutex as AsyncMutex, OwnedMutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard,
};
use webcodex_core::workflow_session_contract::{
    TOOL_ACCEPTED_EXIT_CODES_FIELD, TOOL_ASSERTION_NAME_FIELD,
    TOOL_CALL_ACK_SESSION_CONTEXT_REVISION_FIELD, TOOL_CALL_ACK_SESSION_MESSAGE_IDS_FIELD,
    TOOL_CALL_RECORDING_SESSION_ID_FIELD, TOOL_CALL_SESSION_MESSAGE_RESOLUTION_FIELD,
    TOOL_EXPECTED_FAILURE_FIELD, TOOL_EXPECTED_FAILURE_KIND_FIELD, TOOL_RESULT_EXPECTATION_FIELD,
};
use webcodex_tool_contracts::{
    runtime_tool_composition_policy, runtime_tool_execution_contract, runtime_tool_metadata,
    ToolCompositionPolicy, ToolEffect, ToolExecutionContinuation,
};

/// Process-local serialization for orchestration-originated Project mutation.
///
/// This deliberately does not participate in direct mutation dispatch. It only
/// contains concurrent orchestration frontends targeting the same canonical
/// resolved Project, while unrelated Projects retain independent mutation lanes.
#[derive(Debug)]
pub(crate) struct OrchestrationMutationFenceRegistry {
    projects: Mutex<BTreeMap<String, Weak<AsyncMutex<()>>>>,
    #[cfg(test)]
    acquire_attempted: Semaphore,
    #[cfg(test)]
    acquired: Semaphore,
}

impl Default for OrchestrationMutationFenceRegistry {
    fn default() -> Self {
        Self {
            projects: Mutex::new(BTreeMap::new()),
            #[cfg(test)]
            acquire_attempted: Semaphore::new(0),
            #[cfg(test)]
            acquired: Semaphore::new(0),
        }
    }
}

impl OrchestrationMutationFenceRegistry {
    fn fence(&self, project: &str) -> Arc<AsyncMutex<()>> {
        let mut projects = self
            .projects
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        projects.retain(|_, fence| fence.strong_count() > 0);
        if let Some(fence) = projects.get(project).and_then(Weak::upgrade) {
            fence
        } else {
            let fence = Arc::new(AsyncMutex::new(()));
            projects.insert(project.to_string(), Arc::downgrade(&fence));
            fence
        }
    }

    async fn acquire(&self, project: &str) -> OwnedMutexGuard<()> {
        let fence = self.fence(project);
        #[cfg(test)]
        self.acquire_attempted.add_permits(1);
        let guard = fence.lock_owned().await;
        #[cfg(test)]
        self.acquired.add_permits(1);
        guard
    }

    #[cfg(test)]
    pub(crate) async fn hold_project_for_test(&self, project: &str) -> OwnedMutexGuard<()> {
        self.fence(project).lock_owned().await
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_acquire_attempt_for_test(&self) {
        self.acquire_attempted
            .acquire()
            .await
            .expect("orchestration mutation fence attempt semaphore closed")
            .forget();
    }

    #[cfg(test)]
    pub(crate) async fn wait_for_acquired_for_test(&self) {
        self.acquired
            .acquire()
            .await
            .expect("orchestration mutation fence acquired semaphore closed")
            .forget();
    }
}

/// Immutable admission and authority-shaping policy for one orchestration frontend.
///
/// This policy does not grant tool authority. It only narrows which nested calls
/// may re-enter the canonical ToolRuntime and which argument fields remain owned
/// by the outer trusted request context.
#[derive(Debug, Clone, Copy)]
pub(crate) struct OrchestrationPolicy {
    pub(crate) frontend: &'static str,
    pub(crate) policy_name: &'static str,
    pub(crate) admitted_tools: &'static [&'static str],
    pub(crate) denied_tools: &'static [&'static str],
    /// Frontend-specific argument fields that are additionally reserved. The
    /// canonical Server-owned target/invocation fields below are always denied
    /// by the host and cannot be weakened by a frontend policy.
    pub(crate) additional_forbidden_argument_fields: &'static [&'static str],
    /// Optional frontend-only cap for the synchronous handoff preference of
    /// canonical tools whose continuation is observe_jobs. It never changes the
    /// child's total execution timeout or Job identity.
    pub(crate) nested_sync_wait_max_secs: Option<u64>,
    /// Optional per-cell budget for canonical mutation attempts. Classification
    /// comes only from ToolEffect::Mutate; a rejected over-budget call never
    /// crosses canonical business dispatch.
    pub(crate) max_mutation_calls: Option<usize>,
}

impl OrchestrationPolicy {
    pub(crate) fn is_admitted(self, tool_name: &str) -> bool {
        !self.denied_tools.contains(&tool_name) && self.admitted_tools.contains(&tool_name)
    }
}

/// These fields are owned by the canonical orchestration boundary rather than
/// by an individual frontend program. A frontend may further narrow arguments,
/// but it cannot opt back into target selection, recorder/context metadata, or
/// result-expectation evidence shaping.
const SERVER_OWNED_ARGUMENT_FIELDS: &[&str] = &[
    "project",
    "session_id",
    TOOL_CALL_RECORDING_SESSION_ID_FIELD,
    TOOL_CALL_ACK_SESSION_MESSAGE_IDS_FIELD,
    TOOL_CALL_SESSION_MESSAGE_RESOLUTION_FIELD,
    TOOL_CALL_CONTEXT_REQUEST_FIELD,
    TOOL_CALL_ACK_SESSION_CONTEXT_REVISION_FIELD,
    TOOL_EXPECTED_FAILURE_FIELD,
    TOOL_EXPECTED_FAILURE_KIND_FIELD,
    TOOL_RESULT_EXPECTATION_FIELD,
    TOOL_ACCEPTED_EXIT_CODES_FIELD,
    TOOL_ASSERTION_NAME_FIELD,
];

pub(crate) fn is_server_owned_orchestration_argument(field: &str) -> bool {
    SERVER_OWNED_ARGUMENT_FIELDS.contains(&field) || field.starts_with("__webcodex_")
}

/// Payload-free diagnostic summary for one outer orchestration program. It is
/// observability only and never participates in authority, routing, Session,
/// Window, retry, or idempotency decisions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OrchestrationCompositionSummary {
    pub(crate) nested_calls: usize,
    pub(crate) nested_successes: usize,
    pub(crate) nested_failures: usize,
    pub(crate) max_in_flight: usize,
    pub(crate) duration_ms: u64,
    pub(crate) slot_wait_ms: u64,
    pub(crate) returned_bytes: usize,
    pub(crate) nested_raw_result_bytes_total: usize,
    pub(crate) nested_tool_counts: BTreeMap<String, usize>,
    pub(crate) consequential_calls: usize,
    pub(crate) known_results: usize,
    pub(crate) job_handoffs: usize,
    pub(crate) outcome_unknown: usize,
}

#[derive(Debug, Default)]
struct OrchestrationCompositionAccumulator {
    nested_calls: usize,
    nested_successes: usize,
    nested_failures: usize,
    in_flight: usize,
    max_in_flight: usize,
    nested_raw_result_bytes_total: usize,
    nested_tool_counts: BTreeMap<String, usize>,
}

impl OrchestrationCompositionAccumulator {
    fn begin_call(&mut self, tool_name: &str) -> usize {
        self.nested_calls = self.nested_calls.saturating_add(1);
        self.in_flight = self.in_flight.saturating_add(1);
        self.max_in_flight = self.max_in_flight.max(self.in_flight);
        *self
            .nested_tool_counts
            .entry(tool_name.to_string())
            .or_default() += 1;
        self.nested_calls
    }

    fn finish_call(&mut self, success: bool, raw_result_bytes: usize) {
        self.in_flight = self.in_flight.saturating_sub(1);
        self.nested_raw_result_bytes_total = self
            .nested_raw_result_bytes_total
            .saturating_add(raw_result_bytes);
        if success {
            self.nested_successes = self.nested_successes.saturating_add(1);
        } else {
            self.nested_failures = self.nested_failures.saturating_add(1);
        }
    }

    fn summary(
        &self,
        duration_ms: u64,
        returned_bytes: usize,
        slot_wait_ms: u64,
    ) -> OrchestrationCompositionSummary {
        OrchestrationCompositionSummary {
            nested_calls: self.nested_calls,
            nested_successes: self.nested_successes,
            nested_failures: self.nested_failures,
            max_in_flight: self.max_in_flight,
            duration_ms,
            slot_wait_ms,
            returned_bytes,
            nested_raw_result_bytes_total: self.nested_raw_result_bytes_total,
            nested_tool_counts: self.nested_tool_counts.clone(),
            consequential_calls: 0,
            known_results: 0,
            job_handoffs: 0,
            outcome_unknown: 0,
        }
    }
}

struct NestedCallGuard<'a> {
    composition: &'a Mutex<OrchestrationCompositionAccumulator>,
    finished: bool,
}

impl NestedCallGuard<'_> {
    fn finish(mut self, success: bool, raw_result_bytes: usize) {
        self.composition
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .finish_call(success, raw_result_bytes);
        self.finished = true;
    }
}

impl Drop for NestedCallGuard<'_> {
    fn drop(&mut self) {
        if !self.finished {
            self.composition
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .finish_call(false, 0);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConsequentialChildOutcome {
    KnownResult,
    JobHandoff,
    OutcomeUnknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ConsequentialChildReceipt {
    pub(crate) ordinal: usize,
    pub(crate) tool: String,
    pub(crate) outcome: ConsequentialChildOutcome,
    /// Authoritative workspace state-change truth for canonical mutation only.
    /// Non-mutations and uncertain mutation outcomes deliberately omit it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) state_changed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) job_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) continuation: Option<Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub(crate) struct OrchestrationEffectReceipt {
    pub(crate) consequential_calls: usize,
    pub(crate) known_results: usize,
    pub(crate) job_handoffs: usize,
    pub(crate) outcome_unknown: usize,
    pub(crate) children: Vec<ConsequentialChildReceipt>,
}

#[derive(Debug, Default)]
struct OrchestrationEffectAccumulator {
    children: BTreeMap<usize, ConsequentialChildReceipt>,
}

impl OrchestrationEffectAccumulator {
    fn begin_if_consequential(&mut self, ordinal: usize, tool_name: &str) {
        if runtime_tool_metadata(tool_name).effect == ToolEffect::Observe {
            return;
        }
        self.children.insert(
            ordinal,
            ConsequentialChildReceipt {
                ordinal,
                tool: tool_name.to_string(),
                // Until canonical ToolRuntime returns trustworthy evidence, an
                // already-dispatched consequential child is conservatively unknown.
                outcome: ConsequentialChildOutcome::OutcomeUnknown,
                state_changed: None,
                job_id: None,
                continuation: None,
            },
        );
    }

    fn remove(&mut self, ordinal: usize) {
        self.children.remove(&ordinal);
    }

    fn finish(&mut self, ordinal: usize, result: &super::ToolResult) {
        let output = &result.output;
        let execution_state = output.get("execution_state").and_then(Value::as_str);
        let failure_kind = output.get("failure_kind").and_then(Value::as_str);
        if execution_state == Some("outcome_unknown") || failure_kind == Some("outcome_unknown") {
            if let Some(child) = self.children.get_mut(&ordinal) {
                child.outcome = ConsequentialChildOutcome::OutcomeUnknown;
                child.state_changed = None;
                child.job_id = None;
                child.continuation = None;
            }
            return;
        }
        if output.get("terminal").and_then(Value::as_bool) != Some(true) {
            if let (Some(job_id), Some(continuation)) = (
                output.get("job_id").and_then(Value::as_str),
                output.get("continuation").filter(|value| value.is_object()),
            ) {
                if let Some(child) = self.children.get_mut(&ordinal) {
                    child.outcome = ConsequentialChildOutcome::JobHandoff;
                    child.state_changed = None;
                    child.job_id = Some(job_id.to_string());
                    child.continuation = Some(continuation.clone());
                }
                return;
            }
        }
        if execution_state == Some("not_started")
            || output.get("command_started").and_then(Value::as_bool) == Some(false)
        {
            self.children.remove(&ordinal);
            return;
        }
        let is_mutation = self
            .children
            .get(&ordinal)
            .is_some_and(|child| runtime_tool_metadata(&child.tool).effect == ToolEffect::Mutate);
        let mutation_state_changed = is_mutation
            .then(|| output.get("state_changed").and_then(Value::as_bool))
            .flatten();
        if let Some(child) = self.children.get_mut(&ordinal) {
            child.job_id = None;
            child.continuation = None;
            if is_mutation {
                if let Some(state_changed) = mutation_state_changed {
                    child.outcome = ConsequentialChildOutcome::KnownResult;
                    child.state_changed = Some(state_changed);
                } else {
                    // A mutation without authoritative state-change truth is not
                    // a known effect result, even when the business ToolResult
                    // itself is otherwise well-formed.
                    child.outcome = ConsequentialChildOutcome::OutcomeUnknown;
                    child.state_changed = None;
                }
            } else {
                child.outcome = ConsequentialChildOutcome::KnownResult;
                child.state_changed = None;
            }
        }
    }

    fn receipt(&self) -> OrchestrationEffectReceipt {
        let children = self.children.values().cloned().collect::<Vec<_>>();
        let known_results = children
            .iter()
            .filter(|child| child.outcome == ConsequentialChildOutcome::KnownResult)
            .count();
        let job_handoffs = children
            .iter()
            .filter(|child| child.outcome == ConsequentialChildOutcome::JobHandoff)
            .count();
        let outcome_unknown = children
            .iter()
            .filter(|child| child.outcome == ConsequentialChildOutcome::OutcomeUnknown)
            .count();
        OrchestrationEffectReceipt {
            consequential_calls: children.len(),
            known_results,
            job_handoffs,
            outcome_unknown,
            children,
        }
    }
}

enum CompositionSchedulingGuard<'a> {
    Parallel(RwLockReadGuard<'a, ()>),
    Sequential(RwLockWriteGuard<'a, ()>),
}

impl CompositionSchedulingGuard<'_> {
    fn keep_alive(&self) {
        match self {
            Self::Parallel(guard) => {
                let _ = &**guard;
            }
            Self::Sequential(guard) => {
                let _ = &**guard;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct OrchestrationToolResponse {
    pub(crate) success: bool,
    pub(crate) output: Value,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OrchestrationHostError {
    message: String,
}

impl OrchestrationHostError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub(crate) fn into_message(self) -> String {
        self.message
    }
}

/// Canonical nested-tool host shared by orchestration frontends.
///
/// The host owns no new authority or effect semantics. It freezes the exact outer
/// Project/Session/auth/transport context, applies a frontend-specific admission
/// policy, injects server-owned target fields, and then re-enters the same
/// ToolRuntime path used by direct model calls. Frontend runtimes such as V8 Code
/// Mode continue to own program evaluation, scheduling/concurrency bounds, timeout
/// and cancellation, and final output shaping; this host owns the canonical child
/// invocation boundary they share.
pub(crate) struct CanonicalOrchestrationHost {
    tools: Arc<ToolRuntime>,
    auth: Option<AuthContext>,
    project: String,
    session_id: String,
    transport: ToolTransport,
    composition_parent_invocation_id: Option<String>,
    policy: OrchestrationPolicy,
    composition: Mutex<OrchestrationCompositionAccumulator>,
    scheduling: RwLock<()>,
    accepting_nested_calls: Mutex<bool>,
    effects: Mutex<OrchestrationEffectAccumulator>,
    mutation_calls: Mutex<usize>,
}

impl CanonicalOrchestrationHost {
    pub(crate) fn new(
        tools: ToolRuntime,
        auth: Option<&AuthContext>,
        project: String,
        session_id: String,
        transport: ToolTransport,
        composition_parent_invocation_id: Option<String>,
        policy: OrchestrationPolicy,
    ) -> Self {
        Self {
            tools: Arc::new(tools),
            auth: auth.cloned(),
            project,
            session_id,
            transport,
            composition_parent_invocation_id,
            policy,
            composition: Mutex::new(OrchestrationCompositionAccumulator::default()),
            scheduling: RwLock::new(()),
            accepting_nested_calls: Mutex::new(true),
            effects: Mutex::new(OrchestrationEffectAccumulator::default()),
            mutation_calls: Mutex::new(0),
        }
    }

    fn begin_nested_call(&self, tool_name: &str) -> (usize, NestedCallGuard<'_>) {
        let ordinal = self
            .composition
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .begin_call(tool_name);
        (
            ordinal,
            NestedCallGuard {
                composition: &self.composition,
                finished: false,
            },
        )
    }

    pub(crate) fn stop_accepting_nested_calls(&self) {
        *self
            .accepting_nested_calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
    }

    async fn acquire_scheduling_guard(
        &self,
        tool_name: &str,
    ) -> Result<CompositionSchedulingGuard<'_>, OrchestrationHostError> {
        match runtime_tool_composition_policy(tool_name) {
            ToolCompositionPolicy::Denied => Err(OrchestrationHostError::new(format!(
                "nested tool `{tool_name}` is denied by canonical composition policy"
            ))),
            ToolCompositionPolicy::Sequential => Ok(CompositionSchedulingGuard::Sequential(
                self.scheduling.write().await,
            )),
            ToolCompositionPolicy::Parallel => Ok(CompositionSchedulingGuard::Parallel(
                self.scheduling.read().await,
            )),
        }
    }

    #[cfg(test)]
    pub(crate) async fn assert_scheduling_policy_fences_for_test(&self) {
        let first_parallel = self
            .acquire_scheduling_guard("read_files")
            .await
            .expect("read_files is Parallel");
        assert!(
            self.scheduling.try_read().is_ok(),
            "Parallel + Parallel must be able to overlap"
        );
        assert!(
            self.scheduling.try_write().is_err(),
            "a Sequential child may not overlap an active Parallel child"
        );
        drop(first_parallel);

        let sequential = self
            .acquire_scheduling_guard("cargo_check")
            .await
            .expect("cargo_check is Sequential");
        assert!(
            self.scheduling.try_read().is_err(),
            "Sequential + Parallel must not overlap"
        );
        assert!(
            self.scheduling.try_write().is_err(),
            "Sequential + Sequential must not overlap"
        );
        drop(sequential);

        assert!(self.scheduling.try_read().is_ok());
        assert!(self.scheduling.try_write().is_ok());
        assert!(self.acquire_scheduling_guard("run_shell").await.is_err());
        assert!(self
            .acquire_scheduling_guard("future_unknown_tool")
            .await
            .is_err());
    }

    pub(crate) fn composition_summary(
        &self,
        duration_ms: u64,
        returned_bytes: usize,
        slot_wait_ms: u64,
    ) -> OrchestrationCompositionSummary {
        let effects = self.effect_receipt();
        let mut summary = self
            .composition
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .summary(duration_ms, returned_bytes, slot_wait_ms);
        summary.consequential_calls = effects.consequential_calls;
        summary.known_results = effects.known_results;
        summary.job_handoffs = effects.job_handoffs;
        summary.outcome_unknown = effects.outcome_unknown;
        summary
    }

    pub(crate) fn effect_receipt(&self) -> OrchestrationEffectReceipt {
        self.effects
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .receipt()
    }

    fn prepare_arguments(
        &self,
        tool_name: &str,
        arguments: Value,
    ) -> Result<Value, OrchestrationHostError> {
        if !self.policy.is_admitted(tool_name) {
            return Err(OrchestrationHostError::new(format!(
                "nested tool `{tool_name}` is not admitted by {}",
                self.policy.policy_name
            )));
        }
        let Some(mut arguments) = arguments.as_object().cloned() else {
            return Err(OrchestrationHostError::new(
                "nested tool arguments must be a JSON object",
            ));
        };
        if let Some(field) = arguments
            .keys()
            .find(|field| is_server_owned_orchestration_argument(field))
        {
            return Err(OrchestrationHostError::new(format!(
                "nested tool arguments may not set server-owned field `{field}`"
            )));
        }
        if let Some(field) = self
            .policy
            .additional_forbidden_argument_fields
            .iter()
            .find(|field| arguments.contains_key(**field))
        {
            return Err(OrchestrationHostError::new(format!(
                "nested tool arguments may not set frontend-reserved field `{field}`"
            )));
        }
        if let Some(max_secs) = self.policy.nested_sync_wait_max_secs {
            if runtime_tool_execution_contract(tool_name).is_some_and(|execution| {
                execution.continuation == ToolExecutionContinuation::ObserveJobs
            }) {
                match arguments.get_mut("sync_wait_secs") {
                    None => {
                        arguments.insert("sync_wait_secs".to_string(), Value::from(max_secs));
                    }
                    Some(value) => {
                        if value.as_u64().is_some_and(|seconds| seconds > max_secs) {
                            *value = Value::from(max_secs);
                        }
                    }
                }
            }
        }
        arguments.insert("project".to_string(), Value::String(self.project.clone()));
        arguments.insert(
            "session_id".to_string(),
            Value::String(self.session_id.clone()),
        );
        Ok(Value::Object(arguments))
    }

    pub(crate) async fn invoke_tool(
        &self,
        tool_name: String,
        arguments: Value,
    ) -> Result<OrchestrationToolResponse, OrchestrationHostError> {
        let arguments = self.prepare_arguments(&tool_name, arguments)?;
        let scheduling_guard = self.acquire_scheduling_guard(&tool_name).await?;
        let mutation_guard = if runtime_tool_metadata(&tool_name).effect == ToolEffect::Mutate {
            Some(
                self.tools
                    .orchestration_mutation_fences
                    .acquire(&self.project)
                    .await,
            )
        } else {
            None
        };
        let (child_ordinal, child_guard) = {
            let accepting = self
                .accepting_nested_calls
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if !*accepting {
                return Err(OrchestrationHostError::new(
                    "orchestration frontend is closed; nested call was not dispatched",
                ));
            }
            // The acceptance gate plus scheduling/Project mutation fences establish
            // one linear dispatch boundary with stop_accepting_nested_calls(): once
            // admission closes, a waiter that later acquires either fence cannot start.
            if runtime_tool_metadata(&tool_name).effect == ToolEffect::Mutate {
                if let Some(max_mutation_calls) = self.policy.max_mutation_calls {
                    let mut mutation_calls = self
                        .mutation_calls
                        .lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    if *mutation_calls >= max_mutation_calls {
                        return Err(OrchestrationHostError::new(format!(
                            "{} permits at most {max_mutation_calls} mutation attempt per cell; start a new outer Code Mode call for another mutation",
                            self.policy.policy_name
                        )));
                    }
                    *mutation_calls = mutation_calls.saturating_add(1);
                }
            }
            self.begin_nested_call(&tool_name)
        };
        self.effects
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .begin_if_consequential(child_ordinal, &tool_name);
        scheduling_guard.keep_alive();
        tracing::debug!(
            orchestration_frontend = self.policy.frontend,
            composition_parent_invocation_id = self
                .composition_parent_invocation_id
                .as_deref()
                .unwrap_or("unavailable"),
            composition_child_ordinal = child_ordinal,
            nested_tool = tool_name.as_str(),
            "orchestration_nested_call_started"
        );
        let outcome = self
            .tools
            .call_tool_with_context(
                ToolCallRequest {
                    tool_name: tool_name.clone(),
                    arguments,
                },
                ToolCallContext {
                    transport: self.transport,
                    session_id: Some(self.session_id.as_str()),
                    auth: self.auth.as_ref(),
                    window: None,
                    // Nested scope denials are real Session evidence. The
                    // canonical kernel still owns the scope decision.
                    record_oauth_scope_denials: true,
                    host_file_import_trust: HostFileImportTrust::Untrusted,
                },
            )
            .await;
        // Both orchestration fences cover exactly the canonical ToolRuntime
        // invocation. Direct mutations never acquire the Project fence, and a
        // returned durable Job owns its own lifecycle after this point.
        drop(mutation_guard);
        drop(scheduling_guard);
        {
            let mut effects = self
                .effects
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if outcome.error_status.is_some() {
                effects.remove(child_ordinal);
            } else if let Some(result) = outcome.result.as_ref() {
                effects.finish(child_ordinal, result);
            }
        }
        let nested_success = outcome.error_status.is_none()
            && outcome.result.as_ref().is_some_and(|result| result.success);
        let raw_result_bytes = outcome
            .result
            .as_ref()
            .and_then(|result| serialized_json_len(result).ok())
            .unwrap_or(0);
        child_guard.finish(nested_success, raw_result_bytes);
        tracing::debug!(
            orchestration_frontend = self.policy.frontend,
            composition_parent_invocation_id = self
                .composition_parent_invocation_id
                .as_deref()
                .unwrap_or("unavailable"),
            composition_child_ordinal = child_ordinal,
            nested_tool = tool_name.as_str(),
            success = nested_success,
            "orchestration_nested_call_finished"
        );
        if let Some(error_status) = outcome.error_status {
            let message = match error_status {
                ToolCallErrorStatus::InvalidArguments { message } => message,
                ToolCallErrorStatus::InsufficientScope { description, .. } => description,
            };
            return Err(OrchestrationHostError::new(message));
        }
        let result = outcome.result.ok_or_else(|| {
            OrchestrationHostError::new("canonical ToolRuntime returned no nested ToolResult")
        })?;
        Ok(OrchestrationToolResponse {
            success: result.success,
            output: result.output,
            error: result.error,
        })
    }
}

#[cfg(test)]
mod receipt_tests {
    use super::*;

    #[test]
    fn mutation_result_without_authoritative_state_changed_fails_closed() {
        let mut effects = OrchestrationEffectAccumulator::default();
        effects.begin_if_consequential(1, "apply_text_edits");
        effects.finish(
            1,
            &crate::tool_runtime::ToolResult::ok(serde_json::json!({
                "execution_state": "completed"
            })),
        );

        let receipt = effects.receipt();
        assert_eq!(receipt.consequential_calls, 1);
        assert_eq!(receipt.known_results, 0);
        assert_eq!(receipt.outcome_unknown, 1);
        assert_eq!(
            receipt.children[0].outcome,
            ConsequentialChildOutcome::OutcomeUnknown
        );
        assert_eq!(receipt.children[0].state_changed, None);
    }
}
