use super::{RecoveryKind, ToolResult, ToolRuntime};
use crate::auth::{AuthContext, SCOPE_RUNTIME_READ};
use crate::db::{
    GoalCorrelationKind, GoalDetail, GoalLifecycle, GoalPatch, GoalStoreError, NewGoal,
    MAX_GOAL_LIST_LIMIT,
};
use serde::Serialize;
use serde_json::{json, to_value};
use std::collections::{BTreeSet, HashMap};

const DEFAULT_GOAL_LIST_LIMIT: usize = 50;
pub(crate) const GOAL_ACTIVITY_ATTENTION_AFTER_MS: i64 = 5 * 60_000;
const GOAL_ACTIVITY_SESSION_SCAN_LIMIT: usize = 16;
const GOAL_ACTIVITY_WINDOW_SCAN_LIMIT: usize = 16;
const GOAL_ACTIVITY_EVENT_SCAN_LIMIT: usize = 64;
const GOAL_ACTIVITY_PROJECT_VISIBILITY_LIMIT: usize = 32;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GoalActivityState {
    Active,
    AttentionNeeded,
    Unobserved,
    NotApplicable,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct GoalActivityObservation {
    pub available: bool,
    pub state: GoalActivityState,
    pub idle_threshold_ms: i64,
    pub last_seen_at_ms: Option<i64>,
    pub last_meaningful_activity_at_ms: Option<i64>,
    pub quiet_for_ms: Option<i64>,
    pub linked_window_count: Option<usize>,
    pub active_meaningful_request_count: Option<usize>,
    pub coverage_partial: bool,
}

impl GoalActivityObservation {
    fn unavailable(state: GoalActivityState) -> Self {
        Self {
            available: false,
            state,
            idle_threshold_ms: GOAL_ACTIVITY_ATTENTION_AFTER_MS,
            last_seen_at_ms: None,
            last_meaningful_activity_at_ms: None,
            quiet_for_ms: None,
            linked_window_count: None,
            active_meaningful_request_count: None,
            coverage_partial: false,
        }
    }

    fn not_applicable(available: bool) -> Self {
        if !available {
            return Self::unavailable(GoalActivityState::NotApplicable);
        }
        Self {
            available: true,
            state: GoalActivityState::NotApplicable,
            idle_threshold_ms: GOAL_ACTIVITY_ATTENTION_AFTER_MS,
            last_seen_at_ms: None,
            last_meaningful_activity_at_ms: None,
            quiet_for_ms: None,
            linked_window_count: None,
            active_meaningful_request_count: None,
            coverage_partial: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct GoalPlanProjection {
    pub version: u8,
    pub goal_id: String,
    pub title: String,
    pub objective: String,
    pub lifecycle: GoalLifecycle,
    pub revision: i64,
    pub updated_at_unix_ms: i64,
    pub terminal_at_unix_ms: Option<i64>,
    pub agent_task_count: i64,
    pub workflow_session_count: i64,
    pub activity: GoalActivityObservation,
}

fn goal_plan_projection(goal: GoalDetail, activity: GoalActivityObservation) -> GoalPlanProjection {
    GoalPlanProjection {
        // Activity is an additive observation field. Keep the wire projection at
        // v1 so an already-mounted pre-liveness Goal Plan View can continue to
        // accept authoritative state across a Server upgrade and simply ignore
        // the new field until that View is remounted with the current resource.
        version: 1,
        goal_id: goal.summary.goal_id,
        title: goal.summary.title,
        objective: goal.objective,
        lifecycle: goal.summary.lifecycle,
        revision: goal.summary.revision,
        updated_at_unix_ms: goal.summary.updated_at_unix_ms,
        terminal_at_unix_ms: goal.summary.terminal_at_unix_ms,
        agent_task_count: goal.summary.agent_task_count,
        workflow_session_count: goal.summary.workflow_session_count,
        activity,
    }
}

fn goal_principal(
    auth: Option<&AuthContext>,
) -> Result<crate::db::CommunicationPrincipal, ToolResult> {
    super::communication::communication_principal(auth)
}

fn goal_store_unavailable() -> ToolResult {
    ToolResult::err_with_output(
        "Durable Goal storage is unavailable in this runtime",
        json!({
            "error_kind": "goal_store_unavailable",
            "state_changed": false,
        }),
    )
    .with_recovery(RecoveryKind::UserAction)
}

fn goal_error(error: GoalStoreError, store_failure_recovery: RecoveryKind) -> ToolResult {
    let recovery = match error.code() {
        "goal_store_unavailable" => store_failure_recovery,
        "goal_not_found" | "goal_revision_changed" | "goal_terminal" => RecoveryKind::Reobserve,
        "goal_idempotency_conflict" => RecoveryKind::Reobserve,
        _ => RecoveryKind::FixInput,
    };
    ToolResult::err_with_output(
        error.message(),
        json!({
            "error_kind": error.code(),
            "message": error.message(),
            "current_revision": error.current_revision(),
            "state_changed": false,
        }),
    )
    .with_recovery(recovery)
}

fn target_authorization_error(error: crate::db::CommunicationStoreError) -> ToolResult {
    ToolResult::err_with_output(
        error.message(),
        json!({
            "error_kind": error.code(),
            "message": error.message(),
            "state_changed": false,
        }),
    )
    .with_recovery(RecoveryKind::Reobserve)
}

fn serialized_goal_success<T: Serialize>(value: T) -> ToolResult {
    match to_value(value) {
        Ok(value) => ToolResult::ok(value),
        Err(error) => ToolResult::err_with_output(
            format!("Failed to serialize durable Goal result: {error}"),
            json!({
                "error_kind": "goal_result_serialization_failed",
                "state_changed": false,
            }),
        )
        .with_recovery(RecoveryKind::NoAction),
    }
}

fn event_visibility_budget_available(
    event: &webcodex_store::models::WindowActivityEventRecord,
    cache: &HashMap<String, bool>,
) -> bool {
    let mut unknown = BTreeSet::new();
    if let Some(project) = event.project.as_deref() {
        if cache.contains_key(project) {
            return true;
        }
        unknown.insert(project);
    } else {
        if event.workflow_links.is_empty()
            || event
                .workflow_links
                .iter()
                .any(|link| link.project.is_none())
            || event.workflow_links.iter().any(|link| {
                link.project
                    .as_deref()
                    .and_then(|project| cache.get(project))
                    == Some(&true)
            })
        {
            return true;
        }
        for project in event
            .workflow_links
            .iter()
            .filter_map(|link| link.project.as_deref())
        {
            if !cache.contains_key(project) {
                unknown.insert(project);
            }
        }
    }
    cache.len().saturating_add(unknown.len()) <= GOAL_ACTIVITY_PROJECT_VISIBILITY_LIMIT
}

fn request_visibility_budget_available(
    request: &super::ActiveWindowRequest,
    cache: &HashMap<String, bool>,
) -> bool {
    request.project.as_deref().is_none_or(|project| {
        cache.contains_key(project) || cache.len() < GOAL_ACTIVITY_PROJECT_VISIBILITY_LIMIT
    })
}

impl ToolRuntime {
    pub(crate) fn create_goal(
        &self,
        auth: Option<&AuthContext>,
        title: String,
        objective: String,
        idempotency_key: String,
    ) -> ToolResult {
        let principal = match goal_principal(auth) {
            Ok(principal) => principal,
            Err(result) => return result,
        };
        let Some(db) = self.communication_db.as_ref() else {
            return goal_store_unavailable();
        };
        match db.create_goal(
            &principal,
            NewGoal {
                title,
                objective,
                idempotency_key,
            },
        ) {
            Ok(result) => serialized_goal_success(result),
            Err(error) => goal_error(error, RecoveryKind::RetrySame),
        }
    }

    pub(crate) fn get_goal(&self, auth: Option<&AuthContext>, goal_id: String) -> ToolResult {
        let principal = match goal_principal(auth) {
            Ok(principal) => principal,
            Err(result) => return result,
        };
        let Some(db) = self.communication_db.as_ref() else {
            return goal_store_unavailable();
        };
        match db.read_goal(&principal, &goal_id) {
            Ok(goal) => serialized_goal_success(json!({"goal": goal})),
            Err(error) => goal_error(error, RecoveryKind::Reobserve),
        }
    }

    async fn goal_activity_observation_at(
        &self,
        auth: Option<&AuthContext>,
        goal: &GoalDetail,
        now_ms: i64,
    ) -> GoalActivityObservation {
        let runtime_observation_available = auth
            .is_some_and(|auth| auth.has_scope(SCOPE_RUNTIME_READ))
            && self.window_activity_db.is_some();
        if goal.summary.lifecycle != GoalLifecycle::Active {
            return GoalActivityObservation::not_applicable(runtime_observation_available);
        }
        if !runtime_observation_available {
            return GoalActivityObservation::unavailable(GoalActivityState::Unobserved);
        }
        let Some(auth) = auth else {
            return GoalActivityObservation::unavailable(GoalActivityState::Unobserved);
        };
        let Ok((principal_kind, principal_id)) = super::runtime_observation_principal(Some(auth))
        else {
            return GoalActivityObservation::unavailable(GoalActivityState::Unobserved);
        };
        let principal = Some((principal_kind.as_str(), principal_id.as_str()));
        let Some(db) = self.window_activity_db.as_ref() else {
            return GoalActivityObservation::unavailable(GoalActivityState::Unobserved);
        };

        let mut correlations = goal
            .correlations
            .iter()
            .filter(|correlation| correlation.kind == GoalCorrelationKind::WorkflowSession)
            .collect::<Vec<_>>();
        correlations.sort_by(|a, b| {
            b.created_at_unix_ms
                .cmp(&a.created_at_unix_ms)
                .then_with(|| a.reference_id.cmp(&b.reference_id))
        });
        let mut coverage_partial = correlations.len() > GOAL_ACTIVITY_SESSION_SCAN_LIMIT;
        correlations.truncate(GOAL_ACTIVITY_SESSION_SCAN_LIMIT);

        let mut visibility_cache = HashMap::new();
        let mut candidate_windows = BTreeSet::new();
        for correlation in correlations {
            let Ok(Some(resolved_project)) = self
                .authorize_session_target(
                    &correlation.reference_id,
                    "goal_activity_observation",
                    Some(auth),
                )
                .await
            else {
                continue;
            };
            if !visibility_cache.contains_key(&resolved_project.resolved_id)
                && visibility_cache.len() >= GOAL_ACTIVITY_PROJECT_VISIBILITY_LIMIT
            {
                coverage_partial = true;
                continue;
            }
            if !super::window_activity::window_project_visible_cached(
                self,
                auth,
                &mut visibility_cache,
                Some(&resolved_project.resolved_id),
            )
            .await
            {
                continue;
            }
            let mut linked = match db.list_session_linked_windows(
                &correlation.reference_id,
                principal,
                GOAL_ACTIVITY_WINDOW_SCAN_LIMIT + 1,
            ) {
                Ok(linked) => linked,
                Err(_) => {
                    coverage_partial = true;
                    continue;
                }
            };
            if linked.len() > GOAL_ACTIVITY_WINDOW_SCAN_LIMIT {
                coverage_partial = true;
                linked.truncate(GOAL_ACTIVITY_WINDOW_SCAN_LIMIT);
            }
            for window in linked {
                if candidate_windows.len() >= GOAL_ACTIVITY_WINDOW_SCAN_LIMIT
                    && !candidate_windows.contains(&window.client_window_key)
                {
                    coverage_partial = true;
                    continue;
                }
                candidate_windows.insert(window.client_window_key);
            }
        }

        let linked_window_count = candidate_windows.len();
        let mut last_seen_at_ms = None;
        let mut last_meaningful_activity_at_ms = None;
        let mut active_meaningful_request_count = 0usize;
        coverage_partial |= self.window_activity.coverage_partial_for(principal);

        for window_key in candidate_windows {
            let events = match db.list_window_activity_events(
                &window_key,
                principal,
                GOAL_ACTIVITY_EVENT_SCAN_LIMIT,
            ) {
                Ok(events) => events,
                Err(_) => {
                    coverage_partial = true;
                    continue;
                }
            };
            if events.len() == GOAL_ACTIVITY_EVENT_SCAN_LIMIT
                && events.last().is_some_and(|oldest_scanned| {
                    now_ms.saturating_sub(oldest_scanned.ended_at_ms)
                        <= GOAL_ACTIVITY_ATTENTION_AFTER_MS
                })
            {
                // Events are newest-first. Only a full page whose oldest row is
                // still inside the attention horizon can hide omitted evidence
                // capable of changing an inactivity conclusion.
                coverage_partial = true;
            }
            for event in events {
                if !event_visibility_budget_available(&event, &visibility_cache) {
                    coverage_partial = true;
                    continue;
                }
                if !super::window_activity::window_event_visible_cached(
                    self,
                    auth,
                    &mut visibility_cache,
                    &event,
                )
                .await
                {
                    continue;
                }
                last_seen_at_ms = Some(last_seen_at_ms.unwrap_or(i64::MIN).max(event.ended_at_ms));
                if event.meaningful {
                    last_meaningful_activity_at_ms = Some(
                        last_meaningful_activity_at_ms
                            .unwrap_or(i64::MIN)
                            .max(event.ended_at_ms),
                    );
                }
            }
            for request in self.window_activity.list_for_window(&window_key, principal) {
                if !request_visibility_budget_available(&request, &visibility_cache) {
                    coverage_partial = true;
                    continue;
                }
                if !super::window_activity::active_window_request_visible_cached(
                    self,
                    auth,
                    &mut visibility_cache,
                    &request,
                )
                .await
                {
                    continue;
                }
                last_seen_at_ms = Some(
                    last_seen_at_ms
                        .unwrap_or(i64::MIN)
                        .max(request.started_at_ms),
                );
                if request.is_meaningful() {
                    active_meaningful_request_count =
                        active_meaningful_request_count.saturating_add(1);
                }
            }
        }

        let quiet_for_ms =
            last_meaningful_activity_at_ms.map(|last| now_ms.saturating_sub(last).max(0));
        let state = if active_meaningful_request_count > 0
            || quiet_for_ms.is_some_and(|quiet| quiet <= GOAL_ACTIVITY_ATTENTION_AFTER_MS)
        {
            GoalActivityState::Active
        } else if coverage_partial {
            // Missing bounded evidence might contain a recent meaningful call or
            // a still-running request, so never manufacture an inactivity alert.
            GoalActivityState::Unobserved
        } else if quiet_for_ms.is_some() {
            GoalActivityState::AttentionNeeded
        } else {
            GoalActivityState::Unobserved
        };
        GoalActivityObservation {
            available: true,
            state,
            idle_threshold_ms: GOAL_ACTIVITY_ATTENTION_AFTER_MS,
            last_seen_at_ms,
            last_meaningful_activity_at_ms,
            quiet_for_ms,
            linked_window_count: Some(linked_window_count),
            active_meaningful_request_count: Some(active_meaningful_request_count),
            coverage_partial,
        }
    }

    async fn exact_goal_plan_at(
        &self,
        auth: Option<&AuthContext>,
        goal_id: String,
        now_ms: i64,
    ) -> ToolResult {
        let principal = match goal_principal(auth) {
            Ok(principal) => principal,
            Err(result) => return result,
        };
        let Some(db) = self.communication_db.as_ref() else {
            return goal_store_unavailable();
        };
        match db.read_goal(&principal, &goal_id) {
            Ok(goal) => {
                let activity = self.goal_activity_observation_at(auth, &goal, now_ms).await;
                serialized_goal_success(json!({
                    "goal_plan": goal_plan_projection(goal, activity),
                }))
            }
            Err(error) => goal_error(error, RecoveryKind::Reobserve),
        }
    }

    async fn exact_goal_plan(&self, auth: Option<&AuthContext>, goal_id: String) -> ToolResult {
        self.exact_goal_plan_at(auth, goal_id, chrono::Utc::now().timestamp_millis())
            .await
    }

    #[cfg(test)]
    pub(crate) async fn goal_plan_state_at(
        &self,
        auth: Option<&AuthContext>,
        goal_id: String,
        now_ms: i64,
    ) -> ToolResult {
        self.exact_goal_plan_at(auth, goal_id, now_ms).await
    }

    pub(crate) async fn present_goal_plan(
        &self,
        auth: Option<&AuthContext>,
        goal_id: String,
    ) -> ToolResult {
        self.exact_goal_plan(auth, goal_id).await
    }

    pub(crate) async fn goal_plan_state(
        &self,
        auth: Option<&AuthContext>,
        goal_id: String,
    ) -> ToolResult {
        self.exact_goal_plan(auth, goal_id).await
    }

    pub(crate) fn list_goals(
        &self,
        auth: Option<&AuthContext>,
        lifecycle: Option<String>,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> ToolResult {
        let principal = match goal_principal(auth) {
            Ok(principal) => principal,
            Err(result) => return result,
        };
        let lifecycle = match lifecycle
            .as_deref()
            .map(GoalLifecycle::from_input)
            .transpose()
        {
            Ok(lifecycle) => lifecycle,
            Err(error) => return goal_error(error, RecoveryKind::FixInput),
        };
        let limit = limit.unwrap_or(DEFAULT_GOAL_LIST_LIMIT);
        if limit == 0 || limit > MAX_GOAL_LIST_LIMIT {
            return goal_error(
                GoalStoreError::new(
                    "invalid_goal_list_limit",
                    format!("limit must be within 1..={MAX_GOAL_LIST_LIMIT}"),
                ),
                RecoveryKind::FixInput,
            );
        }
        let Some(db) = self.communication_db.as_ref() else {
            return goal_store_unavailable();
        };
        match db.list_goals(&principal, lifecycle, offset.unwrap_or(0), limit) {
            Ok(page) => serialized_goal_success(page),
            Err(error) => goal_error(error, RecoveryKind::Reobserve),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn update_goal(
        &self,
        auth: Option<&AuthContext>,
        goal_id: String,
        expected_revision: i64,
        title: Option<String>,
        objective: Option<String>,
        lifecycle: Option<String>,
        terminal_reason: Option<String>,
        idempotency_key: String,
    ) -> ToolResult {
        let principal = match goal_principal(auth) {
            Ok(principal) => principal,
            Err(result) => return result,
        };
        let lifecycle = match lifecycle
            .as_deref()
            .map(GoalLifecycle::from_input)
            .transpose()
        {
            Ok(lifecycle) => lifecycle,
            Err(error) => return goal_error(error, RecoveryKind::FixInput),
        };
        let Some(db) = self.communication_db.as_ref() else {
            return goal_store_unavailable();
        };
        match db.update_goal(
            &principal,
            &goal_id,
            expected_revision,
            GoalPatch {
                title,
                objective,
                lifecycle,
                terminal_reason,
            },
            &idempotency_key,
        ) {
            Ok(result) => serialized_goal_success(result),
            Err(error) => goal_error(error, RecoveryKind::RetrySame),
        }
    }

    pub(crate) fn associate_goal_agent_task(
        &self,
        auth: Option<&AuthContext>,
        goal_id: String,
        task_id: String,
        idempotency_key: String,
    ) -> ToolResult {
        let principal = match goal_principal(auth) {
            Ok(principal) => principal,
            Err(result) => return result,
        };
        let Some(db) = self.communication_db.as_ref() else {
            return goal_store_unavailable();
        };
        // Authorize the Goal first so a foreign Goal cannot be used to probe target ids.
        if let Err(error) = db.read_goal(&principal, &goal_id) {
            return goal_error(error, RecoveryKind::Reobserve);
        }
        // Re-authorize the AgentTask in its own domain. The subsequent Goal record stores
        // only the exact identity; this check is never converted into inherited authority.
        if let Err(error) = db.read_agent_task(&principal, &task_id) {
            return target_authorization_error(error);
        }
        match db.associate_goal_agent_task(&principal, &goal_id, &task_id, &idempotency_key) {
            Ok(result) => serialized_goal_success(result),
            Err(error) => goal_error(error, RecoveryKind::RetrySame),
        }
    }

    pub(crate) async fn associate_goal_workflow_session(
        &self,
        auth: Option<&AuthContext>,
        goal_id: String,
        session_id: String,
        idempotency_key: String,
    ) -> ToolResult {
        let principal = match goal_principal(auth) {
            Ok(principal) => principal,
            Err(result) => return result,
        };
        let Some(db) = self.communication_db.as_ref() else {
            return goal_store_unavailable();
        };
        // Check Goal ownership before looking at the target Session to preserve existence hiding.
        if let Err(error) = db.read_goal(&principal, &goal_id) {
            return goal_error(error, RecoveryKind::Reobserve);
        }
        // This is the existing authoritative Session fence: it verifies the immutable
        // creation-time authority fingerprint and independently re-authorizes a bound Project.
        if let Err(result) = self
            .authorize_session_target(&session_id, "associate_goal_workflow_session", auth)
            .await
        {
            return result;
        }
        match db.associate_goal_workflow_session(
            &principal,
            &goal_id,
            &session_id,
            &idempotency_key,
        ) {
            Ok(result) => serialized_goal_success(result),
            Err(error) => goal_error(error, RecoveryKind::RetrySame),
        }
    }
}
