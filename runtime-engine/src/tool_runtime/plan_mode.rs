//! Plan Mode and Goal Mode runtime surface.
//!
//! Plan Mode is enforced here, at the tool dispatch boundary, not by prompts:
//! while a principal holds an active plan lock, every tool call from that
//! principal is checked against an explicit read-only allowlist (default
//! deny). The lock can only be released by the trusted operator path (the
//! Chadex app over local operator REST), by approval, or by expiry.
//!
//! Goal Mode runs are executed by `goal_supervisor`; this module exposes the
//! start/status/control tools and the operator-only review controls.

use super::kernel::{ToolCallContext, ToolCallRequest, ToolTransport};
use super::sessions::SessionTransport;
use super::{RecoveryKind, ToolResult, ToolRuntime};
use crate::auth::{AuthContext, AuthKind};
use crate::db::{
    plan_line_diff, AcceptanceCheck, Database, GoalBudget, GoalControlAction, GoalRunDetail,
    GoalRunRecord, GoalRunState, GoalStartEnvironment, CHATGPT_GOAL_PROVIDER, PlanApprovalBinding, PlanDecisionRecord,
    PlanDecisionState, PlanDetail, PlanLockRecord, PlanReader, PlanSpec, PlanState, PlanStoreError, DEFAULT_PLAN_APPROVAL_TTL_MS,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

tokio::task_local! {
    /// Set only by the in-process Goal supervisor around its own dispatches.
    /// Remote callers cannot set a task-local, so this cannot be forged.
    static GOAL_RUN_DISPATCH: String;
}

pub(crate) async fn with_goal_run_dispatch<F: std::future::Future>(
    goal_run_id: String,
    future: F,
) -> F::Output {
    GOAL_RUN_DISPATCH.scope(goal_run_id, future).await
}

fn goal_run_dispatch_active() -> bool {
    GOAL_RUN_DISPATCH.try_with(|_| ()).is_ok()
}

/// Tools a principal in Plan Mode may call. Each entry is a reviewed
/// read-only observation of project files, Git history, Chadex control-plane
/// state, or Skills/Memory text. Anything not listed is denied, including
/// every shell/process/script/job start, file or Git write, LSP (language
/// servers may build into the project), Computer Use, Plugin, local MCP
/// gateway, SSH, coding agents, and any tool added in the future.
pub(crate) const PLAN_MODE_READ_ONLY_TOOLS: &[&str] = &[
    "list_tools",
    "tool_manifest",
    "runtime_status",
    "list_projects",
    "list_runners",
    "project_overview",
    "list_project_files",
    "list_project_tracked_files",
    "search_project_texts",
    "read_files",
    "git_status",
    "git_diff_hunks",
    "git_log",
    "git_review_summary",
    "show_changes",
    "memory_search",
    "memory_read",
    "memory_scope_list",
    "skill_list",
    "skill_load",
    "skill_read_file",
    "skill_inventory",
    "skill_versions",
    "list_goals",
    "get_goal",
    "list_jobs",
    "observe_jobs",
    "job_tail",
];

/// Plan Mode's own control-plane tools. They write only Chadex state.
pub(crate) const PLAN_CONTROL_TOOLS: &[&str] = &[
    "plan_start",
    "plan_save",
    "plan_get",
    "plan_list",
    "plan_wait_approval",
    "goal_run_status",
    "goal_run_list",
    "goal_run_control",
];

/// Operator-only tools: never callable over MCP or by a non-operator principal.
pub(crate) const PLAN_OPERATOR_TOOLS: &[&str] = &["plan_review", "goal_operator"];

/// The only tools the Goal supervisor dispatches through the kernel.
pub(crate) const GOAL_DISPATCH_TOOLS: &[&str] =
    &["run_process", "observe_jobs", "stop_job", "list_jobs", "job_tail"];

/// Scopes a Goal run may ever dispatch with, intersected with the caller's.
const GOAL_DISPATCH_SCOPES: &[&str] = &[
    crate::auth::SCOPE_RUNTIME_READ,
    crate::auth::SCOPE_PROJECT_READ,
    crate::auth::SCOPE_PROJECT_WRITE,
    crate::auth::SCOPE_JOB_RUN,
];

pub(crate) fn now_unix_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn auth_kind_name(kind: &AuthKind) -> &'static str {
    match kind {
        AuthKind::Bootstrap => "bootstrap",
        AuthKind::ApiToken => "api_token",
        AuthKind::AgentToken => "agent_token",
        AuthKind::AccountCredential => "account_credential",
        AuthKind::OAuth2Token => "oauth2_token",
        AuthKind::SharedKey => "shared_key",
        AuthKind::ProjectCredential => "project_credential",
        AuthKind::OpenAnonymous => "open_anonymous",
    }
}

fn auth_kind_from_name(name: &str) -> Option<AuthKind> {
    Some(match name {
        "bootstrap" => AuthKind::Bootstrap,
        "api_token" => AuthKind::ApiToken,
        "agent_token" => AuthKind::AgentToken,
        "account_credential" => AuthKind::AccountCredential,
        "oauth2_token" => AuthKind::OAuth2Token,
        "shared_key" => AuthKind::SharedKey,
        "project_credential" => AuthKind::ProjectCredential,
        "open_anonymous" => AuthKind::OpenAnonymous,
        _ => return None,
    })
}

/// Stable, opaque Plan Mode principal. It deliberately includes the credential
/// kind and token kind so that, for example, the ChatGPT connector and the
/// Chadex app operator never share a lock even when they map to one user. It
/// never includes a token id, so OAuth refresh cannot silently drop a lock.
pub(crate) fn plan_principal_key(auth: Option<&AuthContext>) -> String {
    let material = match auth {
        None => "local-development".to_string(),
        Some(auth) => {
            let subject = auth
                .project_grant_id
                .as_deref()
                .or(auth.shared_key_hash.as_deref())
                .or(auth.user_id.as_deref())
                .or(auth.username.as_deref())
                .unwrap_or("anonymous");
            format!(
                "{}\0{}\0{}",
                auth_kind_name(&auth.kind),
                auth.token_kind.as_deref().unwrap_or(""),
                subject
            )
        }
    };
    let mut hasher = Sha256::new();
    hasher.update(b"chadex.plan.principal.v1\0");
    hasher.update(material.as_bytes());
    format!("pp_{:x}", hasher.finalize())
}

/// In-memory mirror of the durable plan locks, rebuilt from SQLite after
/// every plan write. If the store cannot be read the guard is degraded and
/// fails closed for every principal.
#[derive(Default)]
pub(crate) struct PlanGuard {
    db: Option<Arc<Database>>,
    locks: RwLock<HashMap<String, PlanLockRecord>>,
    degraded: AtomicBool,
    /// Serializes query+publish so an older snapshot never overwrites a newer one.
    reload_order: std::sync::Mutex<()>,
}

impl PlanGuard {
    pub(crate) fn with_database(db: Arc<Database>) -> Self {
        let guard = Self {
            db: Some(db),
            locks: RwLock::new(HashMap::new()),
            degraded: AtomicBool::new(false),
            reload_order: std::sync::Mutex::new(()),
        };
        guard.reload();
        guard
    }

    pub(crate) fn reload(&self) {
        let Some(db) = self.db.as_ref() else {
            return;
        };
        let _order = self.reload_order.lock().expect("plan guard reload order");
        match db.plan_active_locks(now_unix_ms()) {
            Ok(records) => {
                let map = records
                    .into_iter()
                    .map(|record| (record.owner_key.clone(), record))
                    .collect();
                *self.locks.write().expect("plan guard lock") = map;
                self.degraded.store(false, Ordering::SeqCst);
            }
            Err(error) => {
                tracing::error!(error = %error, "plan guard reload failed; failing closed");
                self.degraded.store(true, Ordering::SeqCst);
            }
        }
    }

    /// `Err(())` means the guard cannot decide (degraded).
    fn lookup(&self, owner_key: &str) -> Result<Option<PlanLockRecord>, ()> {
        if self.degraded.load(Ordering::SeqCst) {
            self.reload();
            if self.degraded.load(Ordering::SeqCst) {
                return Err(());
            }
        }
        let now = now_unix_ms();
        Ok(self
            .locks
            .read()
            .expect("plan guard lock")
            .get(owner_key)
            .filter(|record| record.expires_at_unix_ms > now)
            .cloned())
    }

    pub(crate) fn active_lock_count(&self) -> usize {
        let now = now_unix_ms();
        self.locks
            .read()
            .expect("plan guard lock")
            .values()
            .filter(|record| record.expires_at_unix_ms > now)
            .count()
    }
}

fn plan_mode_denied(
    tool_name: &str,
    lock: &PlanLockRecord,
    why: &str,
    change_request: Option<&PlanDecisionRecord>,
) -> ToolResult {
    let (message, recovery) = match change_request {
        // The approver sent the plan back: the model can resolve this itself,
        // so point it at plan_save instead of asking the user to unlock.
        Some(request) => (
            format!(
                "Plan Mode is active (plan {}): '{tool_name}' is blocked because {why}. The approver requested changes to revision {}: \"{}\". Submit a revised plan with plan_save (parent_revision {}) that addresses this feedback.",
                lock.plan_id,
                request.revision,
                request.reason.as_deref().unwrap_or(""),
                request.revision,
            ),
            RecoveryKind::FixInput,
        ),
        None => (
            format!(
                "Plan Mode is active (plan {}): '{tool_name}' is blocked because {why}. Plan Mode allows only read-only inspection and saving plan revisions. Ask the user to approve the plan or exit Plan Mode in the Chadex app.",
                lock.plan_id
            ),
            RecoveryKind::UserAction,
        ),
    };
    ToolResult::err_with_output(
        message,
        json!({
            "error_kind": "plan_mode_read_only",
            "tool": tool_name,
            "plan_id": lock.plan_id,
            "project": lock.project_id,
            "plan_mode_expires_at_unix_ms": lock.expires_at_unix_ms,
            "pending_change_request": change_request.map(change_request_projection),
            "state_changed": false,
        }),
    )
    .with_recovery(recovery)
}

/// The approver's "request changes" decision on the latest revision that no
/// newer revision has answered yet.
fn pending_change_request(detail: &PlanDetail) -> Option<&PlanDecisionRecord> {
    if detail.plan.state == PlanState::Closed {
        return None;
    }
    detail
        .decisions
        .first()
        .filter(|decision| {
            decision.state == PlanDecisionState::Rejected
                && decision.revision == detail.plan.latest_revision
        })
}

/// Keeps the approver's feedback visible when a revision fails to save.
fn with_change_request(mut result: ToolResult, change_request: Option<Value>) -> ToolResult {
    if let (Some(request), Some(output)) = (change_request, result.output.as_object_mut()) {
        output.insert("pending_change_request".into(), request);
    }
    result
}

fn change_request_projection(request: &PlanDecisionRecord) -> Value {
    json!({
        "approval_id": request.approval_id,
        "revision": request.revision,
        "feedback": request.reason.as_deref().unwrap_or(""),
        "approver": request.approver,
        "requested_at_unix_ms": request.created_at_unix_ms,
        "next": format!(
            "Revise the plan to address this feedback and submit it with plan_save (parent_revision {}).",
            request.revision
        ),
    })
}

impl ToolRuntime {
    /// The single Plan Mode decision used by the kernel and by every MCP path
    /// that dispatches outside the kernel (local MCP gateway, Plugin, SSH).
    pub(crate) fn plan_mode_denial(
        &self,
        auth: Option<&AuthContext>,
        tool_name: &str,
        project: Option<&str>,
    ) -> Option<ToolResult> {
        if goal_run_dispatch_active() {
            if GOAL_DISPATCH_TOOLS.contains(&tool_name) {
                return None;
            }
            return Some(
                ToolResult::err_with_output(
                    format!("Goal supervisor may not dispatch '{tool_name}'"),
                    json!({"error_kind": "goal_dispatch_tool_not_allowed", "state_changed": false}),
                )
                .with_recovery(RecoveryKind::NoAction),
            );
        }
        if PLAN_CONTROL_TOOLS.contains(&tool_name) || PLAN_OPERATOR_TOOLS.contains(&tool_name) {
            return None;
        }
        let key = plan_principal_key(auth);
        let lock = match self.plan_guard.lookup(&key) {
            Ok(Some(lock)) => lock,
            Ok(None) => return None,
            Err(()) => {
                if PLAN_MODE_READ_ONLY_TOOLS.contains(&tool_name) {
                    return None;
                }
                return Some(
                    ToolResult::err_with_output(
                        "Plan Mode state is unavailable, so non-read-only tools are blocked (fail closed)",
                        json!({"error_kind": "plan_guard_unavailable", "state_changed": false}),
                    )
                    .with_recovery(RecoveryKind::UserAction),
                );
            }
        };
        let why = if !PLAN_MODE_READ_ONLY_TOOLS.contains(&tool_name) {
            "it is not on the Plan Mode read-only allowlist"
        } else if project.is_some_and(|project| !project.is_empty() && project != lock.project_id) {
            "Plan Mode is limited to the plan's project"
        } else {
            return None;
        };
        let detail = self.communication_db.as_ref().and_then(|db| {
            db.plan_read(PlanReader::Owner(&key), &lock.plan_id, None, now_unix_ms()).ok()
        });
        let change_request = detail.as_ref().and_then(pending_change_request);
        Some(plan_mode_denied(tool_name, &lock, why, change_request))
    }

    fn plan_db(&self) -> Result<&Arc<Database>, ToolResult> {
        self.communication_db.as_ref().ok_or_else(|| {
            ToolResult::err_with_output(
                "Durable Plan storage is unavailable in this runtime",
                json!({"error_kind": "plan_store_unavailable", "state_changed": false}),
            )
            .with_recovery(RecoveryKind::UserAction)
        })
    }

    async fn plan_project_environment(
        &self,
        project: &str,
        auth: Option<&AuthContext>,
    ) -> Result<(String, crate::projects::ProjectConfig), ToolResult> {
        match self.resolve_project_input_for_auth(project, auth).await {
            Ok(resolved) => Ok((resolved.resolved_id, resolved.config)),
            Err(error) => Err(ToolResult::err_with_output(
                format!("Project is not available to this principal: {error}"),
                json!({"error_kind": "project_unavailable", "project": project, "state_changed": false}),
            )
            .with_recovery(RecoveryKind::FixInput)),
        }
    }

    /// An explicit project as given, or a default with where it came from:
    /// this principal's active Plan Mode lock first (so a resent plan_start
    /// stays idempotent after the user switches projects in the app), then the
    /// operator-side default (the project open in the Chadex app).
    async fn project_or_default(
        &self,
        project: Option<String>,
        auth: Option<&AuthContext>,
    ) -> Result<(String, Option<super::active_project::DefaultProject>), ToolResult> {
        if let Some(project) = project {
            return Ok((project, None));
        }
        let default = match self.plan_guard.lookup(&plan_principal_key(auth)) {
            Ok(Some(lock)) => super::active_project::DefaultProject {
                project: lock.project_id,
                source: super::active_project::SOURCE_ACTIVE_PLAN,
            },
            _ => self.default_project_for_auth(auth).await?,
        };
        Ok((default.project.clone(), Some(default)))
    }

    /// The sentence telling the model to tell the user which defaulted
    /// project it is using (name and path when this principal can see them).
    async fn defaulted_project_notice(
        &self,
        auth: Option<&AuthContext>,
        default: &super::active_project::DefaultProject,
        project_id: &str,
        path: &str,
    ) -> String {
        let listed = self
            .list_projects_with_options(
                auth,
                super::projects::ListProjectsOptions {
                    project: Some(project_id.to_string()),
                    limit: Some(1),
                    summary_only: true,
                    ..Default::default()
                },
            )
            .await;
        let name = listed.output["projects"][0]["name"].as_str().map(str::to_string);
        let path = (!path.is_empty()).then_some(path);
        default.tell_user(name.as_deref(), path)
    }

    pub(crate) async fn plan_start(
        &self,
        auth: Option<&AuthContext>,
        project: Option<String>,
        title: String,
    ) -> ToolResult {
        let db = match self.plan_db() {
            Ok(db) => db.clone(),
            Err(result) => return result,
        };
        let (project, defaulted) = match self.project_or_default(project, auth).await {
            Ok(value) => value,
            Err(result) => return result,
        };
        let (project_id, config) = match self.plan_project_environment(&project, auth).await {
            Ok(value) => value,
            Err(result) => return result,
        };
        let owner = plan_principal_key(auth);
        let outcome = db.plan_start(&owner, &project_id, &title, now_unix_ms());
        self.plan_guard.reload();
        // After the lock, so the check runs exactly as the locked principal.
        let git_readiness = match &outcome {
            Ok(_) => self.plan_git_readiness(auth, &project_id).await,
            Err(_) => None,
        };
        let mut note = "This Chadex connection is now read-only for the project. Do not stop or wait for the user now: in this same reply, inspect the project with read tools, save the full plan with plan_save (include acceptance checks), then call plan_wait_approval and keep calling it until it returns a decision. The user approves the exact revision in the Chadex app; on approval Plan Mode ends and you implement it yourself right away (it is not a Goal). Plan Mode also ends on exit in the app or after expiry.".to_string();
        if let (Ok(_), Some(default)) = (&outcome, defaulted.as_ref()) {
            note.push(' ');
            note.push_str(&self.defaulted_project_notice(auth, default, &project_id, &config.path).await);
        }
        match outcome {
            Ok(outcome) => ToolResult::ok(json!({
                "project_resolution": defaulted.as_ref().map(|default| default.projection()),
                "git_readiness": git_readiness,
                "pending_change_request": db
                    .plan_read(PlanReader::Owner(&owner), &outcome.plan.plan_id, None, now_unix_ms())
                    .ok()
                    .and_then(|detail| pending_change_request(&detail).map(change_request_projection)),
                "plan": outcome.plan,
                "created": outcome.created,
                "plan_mode": {
                    "active": true,
                    "enforcement": "backend",
                    "allowed_tools": PLAN_MODE_READ_ONLY_TOOLS,
                    "note": note,
                },
            })),
            Err(error) => plan_error(error),
        }
    }

    pub(crate) async fn plan_save(
        &self,
        auth: Option<&AuthContext>,
        plan_id: String,
        expected_revision: i64,
        content: String,
        acceptance: Option<Vec<AcceptanceCheck>>,
        budget: Option<GoalBudget>,
    ) -> ToolResult {
        let db = match self.plan_db() {
            Ok(db) => db.clone(),
            Err(result) => return result,
        };
        let owner = plan_principal_key(auth);
        let now = now_unix_ms();
        let change_request = db
            .plan_read(PlanReader::Owner(&owner), &plan_id, None, now)
            .ok()
            .and_then(|detail| pending_change_request(&detail).map(change_request_projection));
        let previous = if expected_revision > 0 {
            db.plan_read(PlanReader::Owner(&owner), &plan_id, Some(expected_revision), now)
                .ok()
                .and_then(|detail| detail.revision)
        } else {
            None
        };
        let spec = PlanSpec {
            acceptance: acceptance.unwrap_or_else(|| {
                previous
                    .as_ref()
                    .map(|revision| revision.spec.acceptance.clone())
                    .unwrap_or_default()
            }),
            budget: budget.unwrap_or_else(|| {
                previous
                    .as_ref()
                    .map(|revision| revision.spec.budget.clone())
                    .unwrap_or_default()
            }),
        };
        let outcome =
            db.plan_save_revision(&owner, &plan_id, expected_revision, &content, spec, now);
        self.plan_guard.reload();
        match outcome {
            Ok(outcome) => {
                // Lets the operator approve and start this revision in one
                // step. Recorded per revision, and cleared when it cannot be
                // computed now, so a stale principal is never reused.
                let dispatch = self
                    .goal_dispatch_for_project(&outcome.plan.project_id, auth)
                    .await
                    .ok()
                    .map(|(_, _, dispatch)| dispatch);
                let _ = db.plan_record_dispatch(
                    &owner,
                    &plan_id,
                    outcome.revision.revision,
                    dispatch.as_ref(),
                    now,
                );
                let diff = previous
                    .as_ref()
                    .and_then(|previous| plan_line_diff(&previous.content, content.trim()))
                    .map(|lines| bounded_diff(lines, 200));
                let goal_origin = outcome.plan.origin == "goal";
                let suggested_call = if goal_origin {
                    json!({"tool": "goal_run_list", "arguments": {}})
                } else {
                    json!({
                        "tool": "plan_wait_approval",
                        "arguments": {
                            "plan_id": plan_id.clone(),
                            "wait_secs": GOAL_STATUS_MAX_WAIT_SECS,
                        },
                    })
                };
                let next = if goal_origin {
                    "This is a Goal request, not a Plan. Follow it with goal_run_list and goal_run_status; do not implement it until the user starts it in the Chadex app."
                } else {
                    "Tell the user the plan id, revision and acceptance checks, and that they approve this exact revision in the Chadex app (Plan & Goal). Then call plan_wait_approval and keep calling it in this same reply; when it returns approved, start implementing right away. Never claim the plan is approved before plan_wait_approval says so."
                };
                ToolResult::ok(json!({
                    "plan": outcome.plan,
                    "revision": outcome.revision,
                    "superseded_approvals": outcome.superseded_approvals,
                    "addressed_change_request": change_request,
                    "git_readiness": self.plan_git_readiness(auth, &outcome.plan.project_id).await,
                    "diff_from_previous": diff,
                    "suggested_call": suggested_call,
                    "next": next,
                }))
            }
            Err(error) => with_change_request(plan_error(error), change_request),
        }
    }

    pub(crate) fn plan_get(
        &self,
        auth: Option<&AuthContext>,
        plan_id: String,
        revision: Option<i64>,
        diff_from: Option<i64>,
    ) -> ToolResult {
        let db = match self.plan_db() {
            Ok(db) => db.clone(),
            Err(result) => return result,
        };
        let owner = plan_principal_key(auth);
        plan_read_result(&db, PlanReader::Owner(&owner), &plan_id, revision, diff_from)
    }

    pub(crate) fn plan_list(&self, auth: Option<&AuthContext>, include_closed: bool) -> ToolResult {
        let db = match self.plan_db() {
            Ok(db) => db.clone(),
            Err(result) => return result,
        };
        let owner = plan_principal_key(auth);
        match db.plan_list(PlanReader::Owner(&owner), include_closed, 20, now_unix_ms()) {
            Ok(plans) => ToolResult::ok(json!({"plans": plans})),
            Err(error) => plan_error(error),
        }
    }

    pub(crate) async fn goal_run_start(&self, auth: Option<&AuthContext>, plan_id: String) -> ToolResult {
        let db = match self.plan_db() {
            Ok(db) => db.clone(),
            Err(result) => return result,
        };
        let owner = plan_principal_key(auth);
        // Goals start only in the Chadex app (approve_and_start), where the
        // user also picks Codex or ChatGPT mode. The model can only find the
        // run an earlier start created.
        match db.goal_run_for_plan(&owner, &plan_id, now_unix_ms()) {
            Ok(Some(run)) => ToolResult::ok(json!({
                "goal_run": goal_run_projection(&run),
                "created": false,
            })),
            Ok(None) => {
                // A Plan Mode plan is not a Goal: once approved, the model
                // implements it itself. Point it there instead of at Start Goal.
                if let Ok(detail) = db.plan_read(PlanReader::Owner(&owner), &plan_id, None, now_unix_ms()) {
                    if detail.plan.origin != "goal" {
                        return ToolResult::err_with_output(
                            "This is a Plan Mode plan, not a Goal, and has no Start Goal step. Call plan_wait_approval with this plan_id: when it returns approved, implement the plan yourself right away in this reply and run its acceptance checks. Do not ask the user to start a Goal.",
                            json!({"error_kind": "not_a_goal", "plan_id": plan_id, "next_tool": "plan_wait_approval", "state_changed": false}),
                        );
                    }
                }
                ToolResult::err_with_output(
                "This Goal has not been started. Only the user can start it, in the Chadex app (Plan & Goal), where they also choose Codex or ChatGPT mode.",
                json!({"error_kind": "goal_not_started", "state_changed": false}),
            )
            .with_recovery(RecoveryKind::UserAction)
            }
            Err(error) => plan_error(error),
        }
    }

    /// Direct Goal: record objective + acceptance as a one-revision request
    /// that the user starts in the Chadex app. The model never starts it.
    pub(crate) async fn goal_request(
        &self,
        auth: Option<&AuthContext>,
        project: Option<String>,
        title: Option<String>,
        objective: String,
        acceptance: Option<Vec<AcceptanceCheck>>,
        budget: Option<GoalBudget>,
    ) -> ToolResult {
        let db = match self.plan_db() {
            Ok(db) => db.clone(),
            Err(result) => return result,
        };
        let Some(title) = title else {
            return invalid_action("A direct Goal needs title, objective and acceptance (project defaults to the project open in the Chadex app)");
        };
        let (project, defaulted) = match self.project_or_default(project, auth).await {
            Ok(value) => value,
            Err(result) => return result,
        };
        let (project_id, config, dispatch) = match self.goal_dispatch_for_project(&project, auth).await {
            Ok(value) => value,
            Err(result) => return result,
        };
        let spec = PlanSpec {
            acceptance: acceptance.unwrap_or_default(),
            budget: budget.unwrap_or_default(),
        };
        let owner = plan_principal_key(auth);
        let created = db.goal_request_create(&owner, &project_id, &title, &objective, spec, &dispatch, now_unix_ms());
        let git_readiness = match &created {
            Ok(_) => self.plan_git_readiness(auth, &project_id).await,
            Err(_) => None,
        };
        let mut next = "Tell the user to press Start Goal in the Chadex app (Plan & Goal); the model cannot start it. Afterwards find the run with goal_run_list (match this plan_id) and follow it with goal_run_status. Chadex notifies the user on the Mac when the Goal finishes; ChatGPT is not notified.".to_string();
        if let (Ok(_), Some(default)) = (&created, defaulted.as_ref()) {
            next.push(' ');
            next.push_str(&self.defaulted_project_notice(auth, default, &project_id, &config.path).await);
        }
        match created {
            Ok(outcome) => ToolResult::ok(json!({
                "project_resolution": defaulted.as_ref().map(|default| default.projection()),
                "git_readiness": git_readiness,
                "plan": outcome.plan,
                "revision": outcome.revision,
                "state": "awaiting_start",
                "next": next,
            })),
            Err(error) => plan_error(error),
        }
    }

    /// Resolve a project for `auth` plus the restricted principal a Goal run
    /// will dispatch as.
    async fn goal_dispatch_for_project(
        &self,
        project: &str,
        auth: Option<&AuthContext>,
    ) -> Result<(String, crate::projects::ProjectConfig, Value), ToolResult> {
        let (project_id, config) = self.plan_project_environment(project, auth).await?;
        let runner_owner = self
            .runner_registry
            .get_runner_semantic_view_for_auth(&config.client_id, None)
            .await
            .and_then(|runner| runner.view.owner);
        let dispatch = goal_dispatch_auth_json(auth, runner_owner.as_deref())?;
        Ok((project_id, config, dispatch))
    }

    fn start_goal_run(
        &self,
        db: &Database,
        owner: &str,
        plan_id: &str,
        config: &crate::projects::ProjectConfig,
        dispatch_auth: &Value,
        provider: &str,
        now: i64,
    ) -> ToolResult {
        let environment = GoalStartEnvironment {
            runner_client_id: config.client_id.clone(),
            project_root: config.path.clone(),
        };
        match db.goal_run_start(owner, plan_id, &environment, dispatch_auth, provider, now) {
            Ok(outcome) => {
                if let Some(wake) = self.goal_supervisor_wake.as_ref() {
                    wake.notify_one();
                }
                if outcome.run.provider == CHATGPT_GOAL_PROVIDER {
                    return ToolResult::ok(json!({
                        "goal_run": goal_run_projection(&outcome.run),
                        "created": outcome.created,
                        "supervisor_running": self.goal_supervisor_wake.is_some(),
                        "note": "ChatGPT mode: you do the work in this conversation, no Codex quota is used. Chadex prepares an isolated worktree (workspace_project in goal_run_status). Edit files there, then call goal_check; repeat until achieved is true. Chadex only runs the approved acceptance checks.",
                    }));
                }
                ToolResult::ok(json!({
                    "goal_run": goal_run_projection(&outcome.run),
                    "created": outcome.created,
                    "supervisor_running": self.goal_supervisor_wake.is_some(),
                    "note": "The Goal now runs inside Chadex on this Mac, independent of this ChatGPT conversation: it iterates agent work, commits each iteration in an isolated worktree, and only succeeds when every acceptance check passes. Closing ChatGPT does not stop it; quitting the Chadex app or sleeping the Mac pauses it until Chadex runs again. ChatGPT is not notified when it finishes; Chadex notifies the user on the Mac. Check progress with goal_run_status (wait_secs waits for completion).",
                }))
            }
            Err(error) => plan_error(error),
        }
    }

    /// Bounded wait for the user's decision on the latest revision, so an
    /// approval in the Chadex app lets the model start work in the same reply.
    pub(crate) async fn plan_wait_approval(
        &self,
        auth: Option<&AuthContext>,
        plan_id: String,
        wait_secs: u64,
    ) -> ToolResult {
        let db = match self.plan_db() {
            Ok(db) => db.clone(),
            Err(result) => return result,
        };
        let owner = plan_principal_key(auth);
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(wait_secs.min(GOAL_STATUS_MAX_WAIT_SECS));
        loop {
            let detail = match db.plan_read(PlanReader::Owner(&owner), &plan_id, None, now_unix_ms()) {
                Ok(detail) => detail,
                Err(error) => return plan_error(error),
            };
            let latest = detail.plan.latest_revision;
            if detail.plan.origin == "goal" {
                return ToolResult::err_with_output(
                    "This is a Goal request, not a Plan. Follow it with goal_run_list and goal_run_status; do not implement it until the user starts it in the Chadex app.".to_string(),
                    json!({
                        "error_kind": "not_a_plan",
                        "plan_id": plan_id.clone(),
                        "suggested_call": {"tool": "goal_run_list", "arguments": {}},
                        "state_changed": false,
                    }),
                );
            }
            let decision = detail.decisions.first().filter(|decision| decision.revision == latest);
            match decision.map(|decision| decision.state) {
                Some(PlanDecisionState::Consumed) => {
                    return ToolResult::ok(json!({
                        "status": "goal_started",
                        "plan": detail.plan,
                        "goal_run_id": decision.and_then(|decision| decision.consumed_by_goal_run_id.clone()),
                        "next": "This revision was started as a Goal in the Chadex app. Do not implement it yourself; follow the Goal with goal_run_status.",
                    }));
                }
                Some(PlanDecisionState::Revoked) | Some(PlanDecisionState::Expired) => {
                    return ToolResult::ok(json!({
                        "status": "approval_withdrawn",
                        "plan": detail.plan,
                        "next": "The approval was withdrawn or expired. Do not implement; ask the user what they want.",
                    }));
                }
                _ => {}
            }
            let approved = decision.is_some_and(|decision| decision.state == PlanDecisionState::Active);
            if approved {
                // The approval released Plan Mode; make sure this connection
                // sees that before it starts writing.
                self.plan_guard.reload();
                return ToolResult::ok(json!({
                    "status": "approved",
                    "plan": detail.plan,
                    "revision": detail.revision,
                }));
            }
            if let Some(request) = pending_change_request(&detail) {
                return ToolResult::ok(json!({
                    "status": "changes_requested",
                    "plan": detail.plan,
                    "pending_change_request": change_request_projection(request),
                    "next": "The user asked for changes. Revise the plan with plan_save (still read-only), tell them what changed, then call plan_wait_approval again.",
                }));
            }
            if detail.plan.state == PlanState::Closed {
                return ToolResult::ok(json!({
                    "status": "closed",
                    "plan": detail.plan,
                    "next": "The user closed this plan in the Chadex app. Stop and ask what they want next.",
                }));
            }
            if std::time::Instant::now() >= deadline {
                return ToolResult::ok(json!({
                    "status": "waiting",
                    "plan": detail.plan,
                    "suggested_call": {
                        "tool": "plan_wait_approval",
                        "arguments": {
                            "plan_id": plan_id.clone(),
                            "wait_secs": GOAL_STATUS_MAX_WAIT_SECS,
                        },
                    },
                    "next": "Still waiting for the user's decision in the Chadex app (Plan & Goal). Call plan_wait_approval again now.",
                }));
            }
            tokio::time::sleep(GOAL_STATUS_POLL).await;
        }
    }

    /// ChatGPT-mode turn: mark this round's edits as done, let the supervisor
    /// commit and run the approved checks, and tell the model whether to
    /// keep going. Only the stored, approved acceptance argv ever runs.
    pub(crate) async fn goal_check(
        &self,
        auth: Option<&AuthContext>,
        goal_run_id: String,
        wait_secs: u64,
    ) -> ToolResult {
        let db = match self.plan_db() {
            Ok(db) => db.clone(),
            Err(result) => return result,
        };
        let owner = plan_principal_key(auth);
        let run = match db.goal_run_chatgpt_ready(&owner, &goal_run_id, now_unix_ms()) {
            Ok(run) => run,
            Err(error) => return plan_error(error),
        };
        if let Some(wake) = self.goal_supervisor_wake.as_ref() {
            wake.notify_one();
        }
        let target = run.chatgpt_ready_iteration;
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(wait_secs.min(GOAL_STATUS_MAX_WAIT_SECS));
        loop {
            let detail = match db.goal_run_read(PlanReader::Owner(&owner), &goal_run_id) {
                Ok(detail) => detail,
                Err(error) => return plan_error(error),
            };
            let run = &detail.run;
            let checked = target > 0 && run.iteration >= target;
            let settled = run.state.terminal()
                || matches!(run.state, GoalRunState::Paused | GoalRunState::NeedsAttention);
            let shown = run.iteration > run.chatgpt_reported_iteration && run.workspace_path.is_some();
            if checked || settled || shown || std::time::Instant::now() >= deadline {
                if run.iteration > 0 {
                    let _ = db.goal_run_chatgpt_reported(&owner, &goal_run_id, run.iteration);
                }
                return ToolResult::ok(goal_check_projection(&db, &detail, target, checked || shown));
            }
            tokio::time::sleep(GOAL_STATUS_POLL).await;
        }
    }

    pub(crate) async fn goal_run_status(
        &self,
        auth: Option<&AuthContext>,
        goal_run_id: String,
        wait_secs: u64,
    ) -> ToolResult {
        let db = match self.plan_db() {
            Ok(db) => db.clone(),
            Err(result) => return result,
        };
        let owner = plan_principal_key(auth);
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(wait_secs.min(GOAL_STATUS_MAX_WAIT_SECS));
        loop {
            let detail = match db.goal_run_read(PlanReader::Owner(&owner), &goal_run_id) {
                Ok(detail) => detail,
                Err(error) => return plan_error(error),
            };
            let settled = !matches!(detail.run.state, GoalRunState::Queued | GoalRunState::Running);
            if settled || std::time::Instant::now() >= deadline {
                return ToolResult::ok(goal_run_detail_projection(&detail));
            }
            tokio::time::sleep(GOAL_STATUS_POLL).await;
        }
    }

    pub(crate) fn goal_run_list(&self, auth: Option<&AuthContext>) -> ToolResult {
        let db = match self.plan_db() {
            Ok(db) => db.clone(),
            Err(result) => return result,
        };
        let owner = plan_principal_key(auth);
        match db.goal_run_list(PlanReader::Owner(&owner), 20) {
            Ok(runs) => ToolResult::ok(json!({
                "goal_runs": runs.iter().map(goal_run_projection).collect::<Vec<_>>(),
            })),
            Err(error) => plan_error(error),
        }
    }

    pub(crate) fn goal_run_control(
        &self,
        auth: Option<&AuthContext>,
        goal_run_id: String,
        action: String,
    ) -> ToolResult {
        let db = match self.plan_db() {
            Ok(db) => db.clone(),
            Err(result) => return result,
        };
        let Some(action) = parse_control_action(&action) else {
            return invalid_action("action must be one of: pause, resume, cancel");
        };
        let owner = plan_principal_key(auth);
        // Plan Mode may stop work but never start it again.
        if action == GoalControlAction::Resume
            && !matches!(self.plan_guard.lookup(&owner), Ok(None))
        {
            {
                return ToolResult::err_with_output(
                    "Resuming a Goal is blocked while Plan Mode is active for this connection",
                    json!({"error_kind": "plan_mode_read_only", "state_changed": false}),
                )
                .with_recovery(RecoveryKind::UserAction);
            }
        }
        match db.goal_run_control(PlanReader::Owner(&owner), &goal_run_id, action, now_unix_ms()) {
            Ok(run) => {
                if let Some(wake) = self.goal_supervisor_wake.as_ref() {
                    wake.notify_one();
                }
                ToolResult::ok(json!({"goal_run": goal_run_projection(&run)}))
            }
            Err(error) => plan_error(error),
        }
    }

    /// Operator-only plan review: inspect/list/approve/reject/revoke/close.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn plan_review(
        &self,
        auth: Option<&AuthContext>,
        transport: SessionTransport,
        action: String,
        plan_id: Option<String>,
        revision: Option<i64>,
        content_sha256: Option<String>,
        reason: Option<String>,
        approval_id: Option<String>,
        ttl_minutes: Option<i64>,
        mode: Option<String>,
    ) -> ToolResult {
        if let Some(denied) = operator_denial(auth, transport) {
            return denied;
        }
        let db = match self.plan_db() {
            Ok(db) => db.clone(),
            Err(result) => return result,
        };
        let now = now_unix_ms();
        let approver = operator_identity(auth);
        let require_plan = || {
            plan_id.clone().ok_or_else(|| invalid_action("plan_id is required"))
        };
        let result = match action.as_str() {
            "list" => match db.plan_list(PlanReader::Operator, false, 50, now) {
                Ok(plans) => ToolResult::ok(json!({"plans": plans})),
                Err(error) => plan_error(error),
            },
            "inspect" => match require_plan() {
                Ok(plan_id) => {
                    // Reviewers always see what changed since the previous revision.
                    let shown = match revision {
                        Some(revision) => Some(revision),
                        None => db
                            .plan_read(PlanReader::Operator, &plan_id, None, now)
                            .ok()
                            .map(|detail| detail.plan.latest_revision),
                    };
                    let diff_from = shown.map(|revision| revision - 1).filter(|value| *value >= 1);
                    let mut result =
                        plan_read_result(&db, PlanReader::Operator, &plan_id, revision, diff_from);
                    let open_project = result.output["plan"]
                        .as_object()
                        .filter(|plan| plan.get("state").and_then(Value::as_str) != Some("closed"))
                        .and_then(|plan| plan.get("project_id").and_then(Value::as_str))
                        .map(str::to_string);
                    if let (Some(project), true) = (open_project, result.success) {
                        let readiness = self.plan_git_readiness(auth, &project).await;
                        if let Some(output) = result.output.as_object_mut() {
                            output.insert("git_readiness".into(), readiness.unwrap_or(Value::Null));
                        }
                    }
                    result
                }
                Err(result) => result,
            },
            "approve" => {
                let plan_id = match require_plan() {
                    Ok(value) => value,
                    Err(result) => return result,
                };
                let (Some(revision), Some(content_sha256)) = (revision, content_sha256) else {
                    return invalid_action("approve requires revision and content_sha256");
                };
                match self
                    .operator_approve(&db, auth, &plan_id, revision, &content_sha256, &approver, ttl_minutes, false, now)
                    .await
                {
                    Ok(decision) => ToolResult::ok(json!({"decision": decision})),
                    Err(result) => result,
                }
            }
            "approve_and_start" => {
                let plan_id = match require_plan() {
                    Ok(value) => value,
                    Err(result) => return result,
                };
                let (Some(revision), Some(content_sha256)) = (revision, content_sha256) else {
                    return invalid_action("approve_and_start requires revision and content_sha256");
                };
                let provider = match goal_provider_for_mode(mode.as_deref()) {
                    Ok(provider) => provider,
                    Err(result) => return result,
                };
                self.operator_approve_and_start(
                    &db, auth, &plan_id, revision, &content_sha256, &approver, ttl_minutes, &provider, now,
                )
                .await
            }
            "reject" => {
                let plan_id = match require_plan() {
                    Ok(value) => value,
                    Err(result) => return result,
                };
                let (Some(revision), Some(content_sha256)) = (revision, content_sha256) else {
                    return invalid_action("reject requires revision and content_sha256");
                };
                match db.plan_reject(
                    &plan_id,
                    revision,
                    &content_sha256,
                    &approver,
                    reason.as_deref().unwrap_or(""),
                    now,
                ) {
                    Ok(decision) => ToolResult::ok(json!({"decision": decision})),
                    Err(error) => plan_error(error),
                }
            }
            "revoke" => {
                let Some(approval_id) = approval_id else {
                    return invalid_action("revoke requires approval_id");
                };
                match db.plan_revoke_approval(&approval_id, now) {
                    Ok(decision) => ToolResult::ok(json!({"decision": decision})),
                    Err(error) => plan_error(error),
                }
            }
            "close" => match require_plan() {
                Ok(plan_id) => {
                    match db.plan_close(&plan_id, reason.as_deref().unwrap_or("closed in Chadex app"), now) {
                        Ok(plan) => ToolResult::ok(json!({"plan": plan})),
                        Err(error) => plan_error(error),
                    }
                }
                Err(result) => result,
            },
            _ => invalid_action(
                "action must be one of: list, inspect, approve, approve_and_start, reject, revoke, close",
            ),
        };
        self.plan_guard.reload();
        result
    }

    /// Operator-only Goal controls, including the persistent emergency stop.
    pub(crate) async fn goal_operator(
        &self,
        auth: Option<&AuthContext>,
        transport: SessionTransport,
        action: String,
        goal_run_id: Option<String>,
        reason: Option<String>,
        project: Option<String>,
    ) -> ToolResult {
        if let Some(denied) = operator_denial(auth, transport) {
            return denied;
        }
        let db = match self.plan_db() {
            Ok(db) => db.clone(),
            Err(result) => return result,
        };
        let now = now_unix_ms();
        let result = match action.as_str() {
            "overview" => {
                let plans = db.plan_list(PlanReader::Operator, false, 50, now);
                let runs = db.goal_run_list(PlanReader::Operator, 30);
                let engaged = db.goal_kill_switch_engaged();
                match (plans, runs, engaged) {
                    (Ok(plans), Ok(runs), Ok(engaged)) => ToolResult::ok(json!({
                        "plans": plans,
                        "goal_runs": runs.iter().map(goal_run_projection).collect::<Vec<_>>(),
                        "emergency_stop_engaged": engaged,
                        "plan_mode_locks": self.plan_guard.active_lock_count(),
                        "supervisor_running": self.goal_supervisor_wake.is_some(),
                    })),
                    (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => plan_error(error),
                }
            }
            "status" => {
                let Some(run_id) = goal_run_id else {
                    return invalid_action("status requires goal_run_id");
                };
                match db.goal_run_read(PlanReader::Operator, &run_id) {
                    Ok(detail) => ToolResult::ok(goal_run_detail_projection(&detail)),
                    Err(error) => plan_error(error),
                }
            }
            "clear_board" => {
                let cleared = db.plan_goal_clear_board(project.as_deref(), now);
                self.plan_guard.reload();
                match cleared {
                    Ok((plans, runs)) => ToolResult::ok(json!({
                        "closed_plans": plans,
                        "dismissed_runs": runs,
                    })),
                    Err(error) => plan_error(error),
                }
            }
            "dismiss" => {
                let Some(run_id) = goal_run_id else {
                    return invalid_action("dismiss requires goal_run_id");
                };
                match db.goal_run_dismiss(&run_id, now) {
                    Ok(run) => ToolResult::ok(json!({"dismissed": true, "goal_run": goal_run_projection(&run)})),
                    Err(error) => plan_error(error),
                }
            }
            "pause" | "resume" | "cancel" => {
                let Some(run_id) = goal_run_id else {
                    return invalid_action("goal_run_id is required");
                };
                let action = parse_control_action(&action).expect("matched above");
                match db.goal_run_control(PlanReader::Operator, &run_id, action, now) {
                    Ok(run) => ToolResult::ok(json!({"goal_run": goal_run_projection(&run)})),
                    Err(error) => plan_error(error),
                }
            }
            "emergency_stop" | "release_emergency_stop" => {
                let engaged = action == "emergency_stop";
                match db.goal_kill_switch_set(
                    engaged,
                    reason.as_deref().unwrap_or(if engaged { "emergency stop" } else { "released" }),
                    now,
                ) {
                    Ok(affected) => ToolResult::ok(json!({
                        "emergency_stop_engaged": engaged,
                        "affected_runs": affected,
                    })),
                    Err(error) => plan_error(error),
                }
            }
            _ => invalid_action(
                "action must be one of: overview, status, pause, resume, cancel, dismiss, clear_board, emergency_stop, release_emergency_stop",
            ),
        };
        if let Some(wake) = self.goal_supervisor_wake.as_ref() {
            wake.notify_one();
        }
        result
    }

    /// One-step "approve and start" from the Chadex app. Everything that can
    /// refuse is checked before approving; if the start still fails, the new
    /// approval is withdrawn so a failed click never leaves a startable plan.
    #[allow(clippy::too_many_arguments)]
    async fn operator_approve_and_start(
        &self,
        db: &Database,
        auth: Option<&AuthContext>,
        plan_id: &str,
        revision: i64,
        content_sha256: &str,
        approver: &str,
        ttl_minutes: Option<i64>,
        provider: &str,
        now: i64,
    ) -> ToolResult {
        let refuse = |kind: &str, message: &str| {
            ToolResult::err_with_output(
                message.to_string(),
                json!({"error_kind": kind, "state_changed": false}),
            )
            .with_recovery(RecoveryKind::UserAction)
        };
        let context = match db.plan_start_context(plan_id, now) {
            Ok(context) => context,
            Err(error) => return plan_error(error),
        };
        if let Some(other) = context.owner_locked_plan {
            return refuse(
                "goal_owner_in_plan_mode",
                &format!("This ChatGPT connection is planning another plan ({other}) in Plan Mode; finish or exit that plan first"),
            );
        }
        let plan = match db.plan_read(PlanReader::Operator, plan_id, None, now) {
            Ok(detail) => detail.plan,
            Err(error) => return plan_error(error),
        };
        let fresh = context
            .dispatch_recorded_at_unix_ms
            .is_some_and(|at| now - at <= GOAL_DISPATCH_MAX_AGE_MS);
        let (Some(dispatch), true) = (context.dispatch, fresh) else {
            return refuse(
                "goal_dispatch_unknown",
                if plan.origin == "goal" {
                    "This Goal request is too old to start. Ask ChatGPT to send the Goal again, then start the new request."
                } else {
                    "Chadex has no current record of which connection this plan runs as. Use Approve only, then ask ChatGPT to start it (goal_run_start)."
                },
            );
        };
        match db.goal_kill_switch_engaged() {
            Ok(false) => {}
            Ok(true) => {
                return refuse(
                    "goal_emergency_stop_engaged",
                    "Goal emergency stop is engaged; release it before starting a Goal",
                )
            }
            Err(error) => return plan_error(error),
        }
        // The run dispatches as the owner's recorded principal: it must still
        // reach the same project, Runner and root that the operator approves.
        let owner_auth = match goal_dispatch_auth(&dispatch) {
            Ok(value) => value,
            Err(message) => return refuse("goal_dispatch_unknown", &message),
        };
        let project = plan.project_id;
        let owner_config = match self.plan_project_environment(&project, owner_auth.as_ref()).await {
            Ok((_, config)) => config,
            Err(result) => return result,
        };
        let operator_config = match self.plan_project_environment(&project, auth).await {
            Ok((_, config)) => config,
            Err(result) => return result,
        };
        if owner_config.client_id != operator_config.client_id || owner_config.path != operator_config.path {
            return refuse(
                "plan_approval_environment_changed",
                "The plan's connection resolves this project to a different Runner or root",
            );
        }
        let decision = match self
            .operator_approve(db, auth, plan_id, revision, content_sha256, approver, ttl_minutes, true, now)
            .await
        {
            Ok(decision) => decision,
            Err(result) => return result,
        };
        let started =
            self.start_goal_run(db, &context.owner_key, plan_id, &owner_config, &dispatch, provider, now);
        if started.success {
            return ToolResult::ok(json!({
                "decision": decision,
                "goal_run": started.output.get("goal_run"),
                "created": started.output.get("created"),
            }));
        }
        let withdrawn = db.plan_revoke_approval(&decision.approval_id, now).is_ok();
        let mut failed = started;
        if let Some(output) = failed.output.as_object_mut() {
            output.insert("approval_withdrawn".into(), json!(withdrawn));
            output.insert("state_changed".into(), json!(!withdrawn));
        }
        failed
    }

    /// Approve one exact revision, bound to the project's Runner, root and HEAD.
    #[allow(clippy::too_many_arguments)]
    async fn operator_approve(
        &self,
        db: &Database,
        auth: Option<&AuthContext>,
        plan_id: &str,
        revision: i64,
        content_sha256: &str,
        approver: &str,
        ttl_minutes: Option<i64>,
        require_commit: bool,
        now: i64,
    ) -> Result<crate::db::PlanDecisionRecord, ToolResult> {
        let plan = db
            .plan_read(PlanReader::Operator, plan_id, None, now)
            .map_err(plan_error)?
            .plan;
        let (project_id, config) = self.plan_project_environment(&plan.project_id, auth).await?;
        // Approving a plan for ChatGPT to implement needs no commit; only a
        // Goal starts from one, and goal_run_start refuses an unborn base.
        let base_commit = match self.operator_head_commit(auth, &project_id).await {
            Ok(commit) => commit,
            Err(result) if !require_commit => {
                let no_commit = self
                    .plan_git_readiness(auth, &project_id)
                    .await
                    .and_then(|readiness| readiness.get("blocks_goal_start").and_then(Value::as_bool))
                    == Some(true);
                if !no_commit {
                    return Err(result);
                }
                crate::db::UNBORN_BASE_COMMIT.to_string()
            }
            Err(result) => return Err(result),
        };
        let binding = PlanApprovalBinding {
            project_id,
            runner_client_id: config.client_id,
            project_root: config.path,
            base_commit,
        };
        let ttl_ms = ttl_minutes
            .map(|minutes| minutes.saturating_mul(60_000))
            .unwrap_or(DEFAULT_PLAN_APPROVAL_TTL_MS);
        // Only a Goal run needs an acceptance check; plain approval does not.
        db.plan_approve_with(plan_id, revision, content_sha256, binding, approver, ttl_ms, require_commit, now)
            .map_err(plan_error)
    }

    /// Resolve the commit the reviewer is approving against.
    async fn operator_head_commit(
        &self,
        auth: Option<&AuthContext>,
        project: &str,
    ) -> Result<String, ToolResult> {
        // Boxed: this re-enters the kernel from inside a dispatched tool.
        let outcome = Box::pin(self.call_tool_with_context(
                ToolCallRequest {
                    tool_name: "run_process".to_string(),
                    arguments: json!({
                        "project": project,
                        "executable": "git",
                        "args": ["rev-parse", "--verify", "HEAD"],
                        "timeout_secs": 30,
                        "sync_wait_secs": 30,
                    }),
                },
                ToolCallContext {
                    transport: ToolTransport::Api,
                    session_id: None,
                    auth,
                    window: None,
                    record_oauth_scope_denials: false,
                    host_file_import_trust: Default::default(),
                },
            ))
            .await;
        let output = outcome.result.map(|result| result.output).unwrap_or(Value::Null);
        let commit = super::goal_supervisor::process_stdout(&output)
            .lines()
            .map(str::trim)
            .find(|line| line.len() == 40 && line.chars().all(|c| c.is_ascii_hexdigit()))
            .map(str::to_string);
        if let Some(commit) = commit {
            return Ok(commit);
        }
        let readiness = self.plan_git_readiness(auth, project).await;
        let message = match readiness.as_ref() {
            Some(readiness) if readiness["blocks_goal_start"] == json!(true) => format!(
                "Goal Mode starts from a Git commit, and this project has none yet. Make a first commit, then approve again: {}",
                readiness["how_to_commit"]
                    .as_array()
                    .map(|steps| steps.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" && "))
                    .unwrap_or_default()
            ),
            _ => "Could not read the project's Git HEAD; Goal Mode requires a Git repository with at least one commit".to_string(),
        };
        Err(ToolResult::err_with_output(
            message,
            json!({
                "error_kind": "plan_base_commit_unavailable",
                "git_readiness": readiness,
                "state_changed": false,
            }),
        )
        .with_recovery(RecoveryKind::UserAction))
    }

    /// Read-only Git check for Goal Mode, through the Plan Mode allowlisted
    /// git tools so it also works for a locked principal. `None` when the Git
    /// state cannot be determined.
    ///
    /// Runs on its own task, so the nested kernel calls do not deepen the
    /// caller's stack, and gives up after a few seconds so an unresponsive
    /// Runner never delays plan tools.
    pub(crate) async fn plan_git_readiness(
        &self,
        auth: Option<&AuthContext>,
        project: &str,
    ) -> Option<Value> {
        let mut probe = tokio::spawn(self.clone().git_readiness_probe(auth.cloned(), project.to_string()));
        match tokio::time::timeout(GIT_READINESS_TIMEOUT, &mut probe).await {
            Ok(joined) => joined.ok().flatten(),
            Err(_) => {
                probe.abort();
                None
            }
        }
    }

    /// A concrete boxed future: naming the type breaks the kernel -> plan tool
    /// -> kernel cycle that an `async fn` here would create.
    fn git_readiness_probe(
        self,
        auth: Option<AuthContext>,
        project: String,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Option<Value>> + Send>> {
        Box::pin(async move {
            let auth = auth.as_ref();
            let (project_id, config) = self.plan_project_environment(&project, auth).await.ok()?;
            let status = self
                .plan_kernel_read(auth, "git_status", json!({"project": project_id}))
                .await?;
            let log = if status.get("exit_code").and_then(Value::as_i64) == Some(0) {
                self.plan_kernel_read(auth, "git_log", json!({"project": project_id, "limit": 1}))
                    .await
            } else {
                None
            };
            git_readiness_from_outputs(&status, log.as_ref(), &config.path)
        })
    }

    async fn plan_kernel_read(
        &self,
        auth: Option<&AuthContext>,
        tool_name: &str,
        arguments: Value,
    ) -> Option<Value> {
        // Boxed: this re-enters the kernel from inside a dispatched tool.
        let call = Box::pin(self.call_tool_with_context(
            ToolCallRequest {
                tool_name: tool_name.to_string(),
                arguments,
            },
            ToolCallContext {
                transport: ToolTransport::Api,
                session_id: None,
                auth,
                window: None,
                record_oauth_scope_denials: false,
                host_file_import_trust: Default::default(),
            },
        ));
        let outcome = call.await;
        outcome.result.filter(|result| result.success).map(|result| result.output)
    }
}

const GOAL_STATUS_MAX_WAIT_SECS: u64 = 45;

/// "codex" (default: the configured background agent) or "chatgpt".
fn goal_provider_for_mode(mode: Option<&str>) -> Result<String, ToolResult> {
    match mode {
        None | Some("codex") => Ok(super::goal_supervisor::GoalProvider::from_env().name().to_string()),
        Some("chatgpt") => Ok(CHATGPT_GOAL_PROVIDER.to_string()),
        Some(_) => Err(invalid_action("mode must be codex or chatgpt")),
    }
}

fn goal_check_projection(db: &Database, detail: &GoalRunDetail, target: i64, checked: bool) -> Value {
    let run = &detail.run;
    let workspace = run.workspace_project_id.clone();
    let mut result = json!({
        "goal_run": goal_run_projection(run),
        "achieved": run.state == GoalRunState::Succeeded,
    });
    let next = if run.state == GoalRunState::Succeeded {
        "Goal achieved: every acceptance check passed. Tell the user the result branch; nothing was merged or pushed. Stop here.".to_string()
    } else if run.state.terminal() || run.state == GoalRunState::NeedsAttention {
        format!(
            "The Goal stopped ({}). Tell the user why; it continues only after review in the Chadex app.",
            run.state_reason.as_deref().unwrap_or("no reason recorded")
        )
    } else if run.state == GoalRunState::Paused {
        "The Goal is paused. Tell the user; it continues only after it is resumed in the Chadex app.".to_string()
    } else if workspace.is_none() || target == 0 {
        "Chadex is still preparing the Goal's isolated worktree. Call goal_check again in a moment; nothing was checked yet.".to_string()
    } else if !checked {
        "The acceptance checks for this round are still running. Call goal_check again; it will not start another round.".to_string()
    } else {
        format!(
            "Not achieved yet (round {} of {}). Fix the failures below in workspace project {}, then call goal_check again in this same reply.",
            run.iteration,
            run.spec.budget.max_iterations,
            workspace.as_deref().unwrap_or("?")
        )
    };
    if checked && run.state != GoalRunState::Succeeded {
        let feedback = super::goal_supervisor::acceptance_feedback(db, &run.goal_run_id, run.iteration);
        result["failures"] = json!(feedback);
    }
    result["next"] = json!(next);
    result
}
/// A recorded Goal principal older than this must be re-recorded by the model.
const GOAL_DISPATCH_MAX_AGE_MS: i64 = 7 * 24 * 60 * 60 * 1000;
const GOAL_STATUS_POLL: std::time::Duration = std::time::Duration::from_secs(2);

/// Parse the optional acceptance/budget arguments shared by plan_save and a
/// direct goal_run_start.
#[allow(clippy::type_complexity)]
pub(crate) fn parse_plan_spec_parts(
    acceptance: Option<Value>,
    budget: Option<Value>,
) -> Result<(Option<Vec<AcceptanceCheck>>, Option<GoalBudget>), ToolResult> {
    let acceptance = acceptance
        .map(serde_json::from_value::<Vec<AcceptanceCheck>>)
        .transpose()
        .map_err(|error| {
            ToolResult::err_with_output(
                format!("invalid acceptance: {error}"),
                json!({"error_kind": "invalid_plan_acceptance", "state_changed": false}),
            )
            .with_recovery(RecoveryKind::FixInput)
        })?;
    let budget = budget
        .map(serde_json::from_value::<GoalBudget>)
        .transpose()
        .map_err(|error| {
            ToolResult::err_with_output(
                format!("invalid budget: {error}"),
                json!({"error_kind": "invalid_plan_budget", "state_changed": false}),
            )
            .with_recovery(RecoveryKind::FixInput)
        })?;
    Ok((acceptance, budget))
}

/// The local operator is the Chadex app talking to the runtime over
/// loopback operator REST (`/api/tools/call`) with the owner's user token.
/// ChatGPT reaches the runtime only over MCP through the tunnel, where the
/// helper presents the runtime *bootstrap* credential; so both MCP transport
/// and bootstrap authority are refused here, and model text cannot
/// impersonate the operator.
fn operator_denial(auth: Option<&AuthContext>, transport: SessionTransport) -> Option<ToolResult> {
    let transport_ok = matches!(transport, SessionTransport::Api);
    let principal_ok = match auth {
        // Unauthenticated in-process/test runtimes only.
        None => true,
        Some(auth) => {
            !auth.is_bootstrap
                && matches!(auth.kind, AuthKind::ApiToken)
                && auth.token_kind.as_deref() == Some("user")
        }
    };
    if transport_ok && principal_ok {
        return None;
    }
    Some(
        ToolResult::err_with_output(
            "This control is available only to the local operator in the Chadex app",
            json!({"error_kind": "operator_only", "state_changed": false}),
        )
        .with_recovery(RecoveryKind::UserAction),
    )
}

fn operator_identity(auth: Option<&AuthContext>) -> String {
    match auth {
        None => "local-operator".to_string(),
        Some(auth) => format!(
            "operator:{}",
            auth.username
                .as_deref()
                .or(auth.user_id.as_deref())
                .unwrap_or("owner")
        ),
    }
}

/// Non-secret projection of the Goal dispatch principal, restricted to Goal
/// dispatch scopes. A bootstrap caller (the local ChatGPT connector) never
/// lends its unrestricted authority to a Goal: the run dispatches as the
/// approved Runner's owner with Goal scopes only, so the normal Runner owner
/// boundary still applies.
fn goal_dispatch_auth_json(
    auth: Option<&AuthContext>,
    runner_owner: Option<&str>,
) -> Result<Value, ToolResult> {
    let Some(auth) = auth else {
        return Ok(json!({"local_development": true}));
    };
    if auth.is_bootstrap || matches!(auth.kind, AuthKind::Bootstrap) {
        let Some(owner) = runner_owner.filter(|owner| !owner.trim().is_empty()) else {
            return Err(ToolResult::err_with_output(
                "Goal Mode needs a Runner with a known owner to dispatch as",
                json!({"error_kind": "goal_runner_owner_unknown", "state_changed": false}),
            )
            .with_recovery(RecoveryKind::UserAction));
        };
        return Ok(json!({
            "kind": "api_token",
            "user_id": owner,
            "username": owner,
            "role": "user",
            "scopes": GOAL_DISPATCH_SCOPES,
            "token_kind": "goal_run",
            "allowed_client_id": null,
            "shared_key_hash": null,
            "project_grant_id": null,
        }));
    }
    let scopes: Vec<&str> = GOAL_DISPATCH_SCOPES
        .iter()
        .copied()
        .filter(|scope| auth.has_scope(scope))
        .collect();
    for required in [crate::auth::SCOPE_PROJECT_WRITE, crate::auth::SCOPE_JOB_RUN] {
        if !scopes.contains(&required) {
            return Err(ToolResult::err_with_output(
                format!("Goal Mode requires the '{required}' scope on this connection"),
                json!({"error_kind": "insufficient_scope", "required_scope": required, "state_changed": false}),
            )
            .with_recovery(RecoveryKind::UserAction));
        }
    }
    Ok(json!({
        "kind": auth_kind_name(&auth.kind),
        "user_id": auth.user_id,
        "username": auth.username,
        "role": auth.role,
        "scopes": scopes,
        "token_kind": auth.token_kind,
        "allowed_client_id": auth.allowed_client_id,
        "shared_key_hash": auth.shared_key_hash,
        "project_grant_id": auth.project_grant_id,
    }))
}

/// Rebuild the dispatch principal for a Goal run. `None` = local development.
pub(crate) fn goal_dispatch_auth(value: &Value) -> Result<Option<AuthContext>, String> {
    if value.get("local_development").and_then(Value::as_bool) == Some(true) {
        return Ok(None);
    }
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .and_then(auth_kind_from_name)
        .ok_or("Goal run dispatch principal is unreadable")?;
    if matches!(kind, AuthKind::Bootstrap) {
        return Err("Goal runs never dispatch with bootstrap authority".to_string());
    }
    // is_bootstrap stays false: AuthContext::new never sets it.
    let text = |field: &str| value.get(field).and_then(Value::as_str).map(str::to_string);
    let mut auth = AuthContext::new(kind);
    auth.user_id = text("user_id");
    auth.username = text("username");
    auth.role = text("role");
    auth.token_kind = text("token_kind");
    auth.allowed_client_id = text("allowed_client_id");
    auth.shared_key_hash = text("shared_key_hash");
    auth.project_grant_id = text("project_grant_id");
    auth.scopes = value
        .get("scopes")
        .and_then(Value::as_array)
        .map(|scopes| {
            scopes
                .iter()
                .filter_map(Value::as_str)
                .filter(|scope| GOAL_DISPATCH_SCOPES.contains(scope))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok(Some(auth))
}

fn parse_control_action(action: &str) -> Option<GoalControlAction> {
    match action {
        "pause" => Some(GoalControlAction::Pause),
        "resume" => Some(GoalControlAction::Resume),
        "cancel" => Some(GoalControlAction::Cancel),
        _ => None,
    }
}

fn invalid_action(message: &str) -> ToolResult {
    ToolResult::err_with_output(
        message.to_string(),
        json!({"error_kind": "invalid_arguments", "state_changed": false}),
    )
    .with_recovery(RecoveryKind::FixInput)
}

fn plan_error(error: PlanStoreError) -> ToolResult {
    let recovery = match error.code() {
        "plan_store_unavailable" => RecoveryKind::RetrySame,
        "plan_not_approved" | "plan_approval_stale" | "plan_approval_environment_changed"
        | "plan_mode_active_for_other_project" | "plan_mode_active_for_other_plan"
        | "goal_resume_not_allowed" | "goal_emergency_stop_engaged" => RecoveryKind::UserAction,
        "plan_revision_changed" | "plan_revision_stale" | "goal_run_terminal" => {
            RecoveryKind::Reobserve
        }
        _ => RecoveryKind::FixInput,
    };
    ToolResult::err_with_output(
        error.message().to_string(),
        json!({"error_kind": error.code(), "message": error.message(), "state_changed": false}),
    )
    .with_recovery(recovery)
}

fn bounded_diff(lines: Vec<String>, max: usize) -> Value {
    let changed = lines.iter().filter(|line| !line.starts_with(' ')).count();
    let truncated = lines.len() > max;
    json!({
        "changed_lines": changed,
        "lines": lines.into_iter().take(max).collect::<Vec<_>>(),
        "truncated": truncated,
    })
}

const GIT_READINESS_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// Shell-quote a path for a paste-ready command.
fn shell_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for ch in value.chars() {
        if matches!(ch, '"' | '\\' | '$' | '`') {
            quoted.push('\\');
        }
        quoted.push(ch);
    }
    quoted.push('"');
    quoted
}

/// Whether a Goal can start from this project, built from the git_status
/// (`git status --porcelain`: stdout, stderr, exit_code) and git_log outputs.
/// `None` when the outputs do not settle it, so callers never warn (or block)
/// on a guess.
pub(crate) fn git_readiness_from_outputs(
    status: &Value,
    log: Option<&Value>,
    project_root: &str,
) -> Option<Value> {
    let is_git_repo = match status.get("exit_code")?.as_i64()? {
        0 => true,
        _ if status
            .get("stderr")
            .and_then(Value::as_str)?
            .to_ascii_lowercase()
            .contains("not a git repository") =>
        {
            false
        }
        _ => return None,
    };
    let head_commit = if is_git_repo {
        // git_log reports an unborn HEAD as a successful empty log.
        match log?.get("head_commit")? {
            Value::Null => None,
            Value::String(commit) if !commit.is_empty() => Some(commit.clone()),
            _ => return None,
        }
    } else {
        None
    };
    let has_commit = head_commit.is_some();
    let uncommitted_files = if is_git_repo {
        status
            .get("stdout")
            .and_then(Value::as_str)
            .map(|stdout| stdout.lines().filter(|line| !line.trim().is_empty()).count() as i64)
    } else {
        None
    };
    let short = head_commit.as_deref().map_or("", |commit| &commit[..commit.len().min(12)]);
    let blocks_goal_start = !is_git_repo || !has_commit;
    let root = shell_quote(project_root);
    let git = |args: &str| format!("git -C {root} {args}");
    let mut how_to_commit = Vec::new();
    if !is_git_repo {
        how_to_commit.push(git("init"));
    }
    if blocks_goal_start {
        how_to_commit.push(git("add -A"));
        how_to_commit.push(git("commit -m \"Initial commit\""));
    } else if uncommitted_files.unwrap_or(0) > 0 {
        how_to_commit.push(git("add -A"));
        how_to_commit.push(git("commit -m \"Save work before Goal\""));
    }
    let message = if !is_git_repo {
        "This project is not a Git repository yet. Goal Mode starts from a Git commit, so this plan cannot be approved or started until the user creates a repository and makes a first commit. Tell the user this now and show them the how_to_commit commands.".to_string()
    } else if !has_commit {
        "This project has no Git commit yet. Goal Mode starts from the commit that is current when the plan is approved, so this plan cannot be approved or started until the user makes a first commit. Tell the user this now and show them the how_to_commit commands.".to_string()
    } else if let Some(count) = uncommitted_files.filter(|count| *count > 0) {
        format!(
            "Goal will start from commit {}. The project has {count} uncommitted file(s); Goal only sees committed content, so tell the user to commit them (how_to_commit) before approving if the Goal needs them.",
            short
        )
    } else {
        format!(
            "Ready: Goal will start from commit {}.",
            short
        )
    };
    let tips: Vec<String> = if how_to_commit.is_empty() {
        Vec::new()
    } else {
        vec![
            format!("Review what will be committed first: git -C {root} status"),
            "Before `git add -A`, make sure .gitignore excludes dependencies, build output, caches and secrets (for example node_modules/, __pycache__/, .env).".to_string(),
            "After committing, approve the plan again in the Chadex app (Plan & Goal).".to_string(),
        ]
    };
    Some(json!({
        "is_git_repo": is_git_repo,
        "has_commit": has_commit,
        "head_commit": head_commit,
        "uncommitted_files": uncommitted_files,
        "blocks_goal_start": blocks_goal_start,
        "project_root": project_root,
        "message": message,
        "how_to_commit": how_to_commit,
        "tips": tips,
    }))
}

fn plan_read_result(
    db: &Database,
    reader: PlanReader<'_>,
    plan_id: &str,
    revision: Option<i64>,
    diff_from: Option<i64>,
) -> ToolResult {
    let now = now_unix_ms();
    let detail = match db.plan_read(reader, plan_id, revision, now) {
        Ok(detail) => detail,
        Err(error) => return plan_error(error),
    };
    let diff = match (diff_from, detail.revision.as_ref()) {
        (Some(from), Some(current)) => match db.plan_read(reader, plan_id, Some(from), now) {
            Ok(old) => old
                .revision
                .and_then(|old| plan_line_diff(&old.content, &current.content))
                .map(|lines| bounded_diff(lines, 400)),
            Err(error) => return plan_error(error),
        },
        _ => None,
    };
    ToolResult::ok(json!({
        "pending_change_request": pending_change_request(&detail).map(change_request_projection),
        "plan": detail.plan,
        "revision": detail.revision,
        "revisions": detail.revisions,
        "decisions": detail.decisions,
        "diff": diff,
    }))
}

pub(crate) fn goal_run_projection(run: &GoalRunRecord) -> Value {
    json!({
        "goal_run_id": run.goal_run_id,
        "plan_id": run.plan_id,
        "revision": run.revision,
        "content_sha256": run.content_sha256,
        "project": run.project_id,
        "state": run.state,
        "state_reason": run.state_reason,
        "control_request": run.control_request,
        "iteration": run.iteration,
        "max_iterations": run.spec.budget.max_iterations,
        "deadline_at_unix_ms": run.deadline_at_unix_ms,
        "provider": run.provider,
        "mode": if run.provider == CHATGPT_GOAL_PROVIDER { "chatgpt" } else { "codex" },
        "workspace_project": run.workspace_project_id,
        "workspace_path": run.workspace_path,
        "waiting_for_chatgpt": run.provider == CHATGPT_GOAL_PROVIDER
            && matches!(run.state, GoalRunState::Queued | GoalRunState::Running)
            && run.workspace_path.is_some()
            && run.chatgpt_ready_iteration <= run.iteration,
        "next_action": (run.provider == CHATGPT_GOAL_PROVIDER && !run.state.terminal()).then(|| format!(
            "ChatGPT mode: edit files in workspace project {}, then call goal_check until achieved is true.",
            run.workspace_project_id.as_deref().unwrap_or("(being prepared)")
        )),
        "result_branch": run.workspace_branch,
        "tokens_used": run.tokens_used,
        "cost": if run.tokens_used.is_some() { "tokens_reported_by_provider" } else { "unknown" },
        "created_at_unix_ms": run.created_at_unix_ms,
        "updated_at_unix_ms": run.updated_at_unix_ms,
        "finished_at_unix_ms": run.finished_at_unix_ms,
    })
}

fn goal_run_detail_projection(detail: &GoalRunDetail) -> Value {
    json!({
        "goal_run": goal_run_projection(&detail.run),
        "acceptance": detail.run.spec.acceptance,
        "operations": detail.ops,
        "events": detail.events,
        "emergency_stop_engaged": detail.kill_switch_engaged,
    })
}
