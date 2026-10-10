//! Durable Goal Mode runs: one run per consumed plan approval, a lease with a
//! monotonically increasing fence for the single active supervisor, an
//! operation journal (intent -> dispatched -> terminal/unknown) that prevents
//! blind re-dispatch after a restart, an append-only event log, and a
//! persistent emergency stop.

use super::communication::allocate_identity;
use super::plan::{
    expire_approvals, load_decision, load_owned_plan, load_revision, plan_store_error,
    PlanApprovalBinding, PlanDecisionState, PlanReader, PlanSpec, PlanStoreError,
};
use super::Database;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde_json::Value;

pub const GOAL_RUN_ID_PREFIX: &str = "wc_grun_";
const MAX_GOAL_RUN_EVENTS_RETURNED: usize = 60;
const MAX_GOAL_EVENT_DETAIL_BYTES: usize = 16 * 1024;
const MAX_OP_RESULT_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoalRunState {
    Queued,
    Running,
    Paused,
    NeedsAttention,
    Succeeded,
    Failed,
    Cancelled,
    Stopped,
}

impl GoalRunState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::NeedsAttention => "needs_attention",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
            Self::Stopped => "stopped",
        }
    }

    pub fn from_db(value: &str) -> rusqlite::Result<Self> {
        Ok(match value {
            "queued" => Self::Queued,
            "running" => Self::Running,
            "paused" => Self::Paused,
            "needs_attention" => Self::NeedsAttention,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "cancelled" => Self::Cancelled,
            "stopped" => Self::Stopped,
            other => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    format!("unsupported Goal run state: {other}").into(),
                ))
            }
        })
    }

    pub const fn terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::Cancelled | Self::Stopped
        )
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoalOpState {
    Intent,
    Dispatched,
    Succeeded,
    Failed,
    Unknown,
}

impl GoalOpState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Intent => "intent",
            Self::Dispatched => "dispatched",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::Unknown => "unknown",
        }
    }

    fn from_db(value: &str) -> rusqlite::Result<Self> {
        Ok(match value {
            "intent" => Self::Intent,
            "dispatched" => Self::Dispatched,
            "succeeded" => Self::Succeeded,
            "failed" => Self::Failed,
            "unknown" => Self::Unknown,
            other => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    format!("unsupported Goal op state: {other}").into(),
                ))
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GoalRunRecord {
    pub goal_run_id: String,
    pub plan_id: String,
    pub revision: i64,
    pub approval_id: String,
    pub content_sha256: String,
    #[serde(skip_serializing)]
    pub owner_key: String,
    pub project_id: String,
    pub binding: PlanApprovalBinding,
    pub spec: PlanSpec,
    /// Non-secret projection of the principal that started the run. It is the
    /// dispatch authority (intersected with the approved scope).
    #[serde(skip_serializing)]
    pub dispatch_auth: Value,
    pub provider: String,
    pub state: GoalRunState,
    pub state_reason: Option<String>,
    pub control_request: Option<String>,
    pub iteration: i64,
    pub same_failure_streak: i64,
    pub last_failure_digest: Option<String>,
    pub workspace_project_id: Option<String>,
    pub workspace_path: Option<String>,
    pub workspace_branch: Option<String>,
    pub tokens_used: Option<i64>,
    pub lease_owner: Option<String>,
    pub lease_generation: i64,
    pub lease_expires_at_unix_ms: Option<i64>,
    pub created_at_unix_ms: i64,
    pub deadline_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
    pub finished_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GoalOpRecord {
    pub op_key: String,
    pub kind: String,
    pub iteration: i64,
    pub state: GoalOpState,
    pub job_id: Option<String>,
    pub result: Option<Value>,
    pub started_at_unix_ms: i64,
    pub finished_at_unix_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GoalEventRecord {
    pub seq: i64,
    pub at_unix_ms: i64,
    pub kind: String,
    pub detail: Value,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GoalRunDetail {
    pub run: GoalRunRecord,
    pub ops: Vec<GoalOpRecord>,
    pub events: Vec<GoalEventRecord>,
    pub kill_switch_engaged: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct GoalRunStartOutcome {
    pub run: GoalRunRecord,
    pub created: bool,
}

/// Lease identity. Every supervisor write is fenced on both values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalLease {
    pub owner: String,
    pub generation: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalControlAction {
    Pause,
    Resume,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalStartEnvironment {
    pub runner_client_id: String,
    pub project_root: String,
}

fn lease_lost() -> PlanStoreError {
    PlanStoreError::new(
        "goal_lease_lost",
        "Goal run lease is no longer held by this supervisor",
    )
}

impl Database {
    pub(super) fn ensure_goal_run_schema(conn: &mut Connection) -> anyhow::Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS wc_goal_runs (
                goal_run_id TEXT PRIMARY KEY,
                plan_id TEXT NOT NULL,
                revision INTEGER NOT NULL,
                approval_id TEXT NOT NULL UNIQUE,
                content_sha256 TEXT NOT NULL,
                owner_key TEXT NOT NULL,
                project_id TEXT NOT NULL,
                binding_json TEXT NOT NULL,
                spec_json TEXT NOT NULL,
                dispatch_auth_json TEXT NOT NULL,
                provider TEXT NOT NULL,
                state TEXT NOT NULL CHECK(state IN (
                    'queued', 'running', 'paused', 'needs_attention',
                    'succeeded', 'failed', 'cancelled', 'stopped'
                )),
                state_reason TEXT,
                control_request TEXT CHECK(control_request IS NULL OR control_request IN ('pause', 'cancel')),
                iteration INTEGER NOT NULL DEFAULT 0,
                same_failure_streak INTEGER NOT NULL DEFAULT 0,
                last_failure_digest TEXT,
                workspace_project_id TEXT,
                workspace_path TEXT,
                workspace_branch TEXT,
                tokens_used INTEGER,
                lease_owner TEXT,
                lease_generation INTEGER NOT NULL DEFAULT 0,
                lease_expires_at_unix_ms INTEGER,
                created_at_unix_ms INTEGER NOT NULL,
                deadline_at_unix_ms INTEGER NOT NULL,
                updated_at_unix_ms INTEGER NOT NULL,
                finished_at_unix_ms INTEGER,
                FOREIGN KEY(plan_id, revision) REFERENCES wc_plan_revisions(plan_id, revision)
            );
            CREATE INDEX IF NOT EXISTS idx_wc_goal_runs_state
                ON wc_goal_runs(state, updated_at_unix_ms);
            CREATE INDEX IF NOT EXISTS idx_wc_goal_runs_owner
                ON wc_goal_runs(owner_key, updated_at_unix_ms DESC);

            CREATE TABLE IF NOT EXISTS wc_goal_run_ops (
                goal_run_id TEXT NOT NULL,
                op_key TEXT NOT NULL,
                kind TEXT NOT NULL,
                iteration INTEGER NOT NULL,
                state TEXT NOT NULL CHECK(state IN ('intent', 'dispatched', 'succeeded', 'failed', 'unknown')),
                job_id TEXT,
                result_json TEXT,
                lease_generation INTEGER NOT NULL,
                started_at_unix_ms INTEGER NOT NULL,
                finished_at_unix_ms INTEGER,
                PRIMARY KEY(goal_run_id, op_key),
                FOREIGN KEY(goal_run_id) REFERENCES wc_goal_runs(goal_run_id)
            );

            CREATE TABLE IF NOT EXISTS wc_goal_run_events (
                goal_run_id TEXT NOT NULL,
                seq INTEGER NOT NULL,
                at_unix_ms INTEGER NOT NULL,
                kind TEXT NOT NULL,
                detail_json TEXT NOT NULL,
                PRIMARY KEY(goal_run_id, seq),
                FOREIGN KEY(goal_run_id) REFERENCES wc_goal_runs(goal_run_id)
            );

            CREATE TABLE IF NOT EXISTS wc_goal_kill_switch (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                engaged INTEGER NOT NULL CHECK(engaged IN (0, 1)),
                reason TEXT,
                updated_at_unix_ms INTEGER NOT NULL
            );
            ",
        )?;
        Ok(())
    }

    /// Consume one active approval and create the run, atomically. A repeated
    /// start for the same approval by the same owner replays the existing run,
    /// so a retried or duplicated start can never create a second dispatcher.
    #[allow(clippy::too_many_arguments)]
    pub fn goal_run_start(
        &self,
        owner_key: &str,
        plan_id: &str,
        environment: &GoalStartEnvironment,
        dispatch_auth: &Value,
        provider: &str,
        now: i64,
    ) -> Result<GoalRunStartOutcome, PlanStoreError> {
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        expire_approvals(&tx, now)?;
        let plan = load_owned_plan(&tx, PlanReader::Owner(owner_key), plan_id, now)?;
        // Replay: the newest approval of this plan already started a run.
        let latest_consumed: Option<String> = tx
            .query_row(
                "SELECT consumed_by_goal_run_id FROM wc_plan_decisions
                 WHERE plan_id = ?1 AND state = 'consumed' AND revision = ?2
                 ORDER BY decided_at_unix_ms DESC LIMIT 1",
                params![plan_id, plan.latest_revision],
                |row| row.get(0),
            )
            .optional()
            .map_err(plan_store_error)?
            .flatten();
        let active: Option<String> = tx
            .query_row(
                "SELECT approval_id FROM wc_plan_decisions WHERE plan_id = ?1 AND state = 'active'",
                params![plan_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(plan_store_error)?;
        let Some(approval_id) = active else {
            if let Some(run_id) = latest_consumed {
                let run = load_run(&tx, &run_id)?;
                tx.commit().map_err(plan_store_error)?;
                return Ok(GoalRunStartOutcome {
                    run,
                    created: false,
                });
            }
            return Err(PlanStoreError::new(
                "plan_not_approved",
                "This plan has no active approval. Approve the exact revision in the Chadex app first.",
            ));
        };
        let decision = load_decision(&tx, &approval_id)?;
        debug_assert_eq!(decision.state, PlanDecisionState::Active);
        if decision.revision != plan.latest_revision {
            return Err(PlanStoreError::new(
                "plan_approval_stale",
                "The approved revision is no longer the latest revision",
            ));
        }
        let binding = decision.binding.clone().ok_or_else(|| {
            PlanStoreError::new("plan_store_corrupt", "Approval is missing its binding")
        })?;
        if binding.runner_client_id != environment.runner_client_id
            || binding.project_root != environment.project_root
        {
            return Err(PlanStoreError::new(
                "plan_approval_environment_changed",
                "The project's Runner or root changed since approval; review and approve again",
            ));
        }
        if kill_switch_engaged(&tx)? {
            return Err(PlanStoreError::new(
                "goal_emergency_stop_engaged",
                "Goal emergency stop is engaged in the Chadex app",
            ));
        }
        let revision = load_revision(&tx, plan_id, decision.revision)?.ok_or_else(|| {
            PlanStoreError::new("plan_revision_not_found", "Approved revision is missing")
        })?;
        if revision.content_sha256 != decision.content_sha256 {
            return Err(PlanStoreError::new(
                "plan_content_hash_mismatch",
                "Approved content hash does not match the stored revision",
            ));
        }
        let run_id = allocate_identity(
            &tx,
            GOAL_RUN_ID_PREFIX,
            "SELECT EXISTS(SELECT 1 FROM wc_goal_runs WHERE goal_run_id = ?1)",
        )
        .map_err(|error| PlanStoreError::new("plan_store_unavailable", error.message().to_string()))?;
        let consumed = tx
            .execute(
                "UPDATE wc_plan_decisions SET state = 'consumed', consumed_by_goal_run_id = ?2,
                     decided_at_unix_ms = ?3
                 WHERE approval_id = ?1 AND state = 'active'",
                params![approval_id, run_id, now],
            )
            .map_err(plan_store_error)?;
        if consumed != 1 {
            return Err(PlanStoreError::new(
                "plan_approval_not_active",
                "Approval was consumed concurrently",
            ));
        }
        let deadline = now + i64::from(revision.spec.budget.max_minutes) * 60_000;
        tx.execute(
            "INSERT INTO wc_goal_runs (
                goal_run_id, plan_id, revision, approval_id, content_sha256, owner_key,
                project_id, binding_json, spec_json, dispatch_auth_json, provider, state,
                created_at_unix_ms, deadline_at_unix_ms, updated_at_unix_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'queued', ?12, ?13, ?12)",
            params![
                run_id,
                plan_id,
                decision.revision,
                approval_id,
                decision.content_sha256,
                owner_key,
                plan.project_id,
                serde_json::to_string(&binding).expect("binding serializes"),
                serde_json::to_string(&revision.spec).expect("spec serializes"),
                serde_json::to_string(dispatch_auth).expect("auth serializes"),
                provider,
                now,
                deadline
            ],
        )
        .map_err(plan_store_error)?;
        append_event(
            &tx,
            &run_id,
            now,
            "created",
            &serde_json::json!({
                "plan_id": plan_id,
                "revision": decision.revision,
                "approval_id": approval_id,
                "content_sha256": decision.content_sha256,
                "provider": provider,
            }),
        )?;
        let run = load_run(&tx, &run_id)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(GoalRunStartOutcome { run, created: true })
    }

    /// Claim runnable runs whose lease is free or expired. Each claim bumps
    /// the fence generation, invalidating every write of a previous holder.
    pub fn goal_run_claim(
        &self,
        owner: &str,
        now: i64,
        lease_ms: i64,
        limit: usize,
    ) -> Result<Vec<(GoalRunRecord, GoalLease)>, PlanStoreError> {
        self.goal_run_claim_excluding(owner, now, lease_ms, limit, &[])
    }

    /// Claim, skipping runs this process is still driving (e.g. its lease
    /// expired while the Mac slept): those keep their current driver.
    pub fn goal_run_claim_excluding(
        &self,
        owner: &str,
        now: i64,
        lease_ms: i64,
        limit: usize,
        exclude: &[String],
    ) -> Result<Vec<(GoalRunRecord, GoalLease)>, PlanStoreError> {
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        if kill_switch_engaged(&tx)? {
            tx.commit().map_err(plan_store_error)?;
            return Ok(Vec::new());
        }
        let held: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM wc_goal_runs WHERE state IN ('queued', 'running')
                   AND lease_owner = ?1 AND lease_expires_at_unix_ms > ?2",
                params![owner, now],
                |row| row.get(0),
            )
            .map_err(plan_store_error)?;
        let held = held.max(exclude.len() as i64);
        let available = (limit as i64 - held).max(0);
        let ids: Vec<String> = {
            let mut statement = tx
                .prepare(
                    "SELECT goal_run_id FROM wc_goal_runs
                     WHERE state IN ('queued', 'running')
                       AND (lease_owner IS NULL OR lease_expires_at_unix_ms <= ?1)
                     ORDER BY created_at_unix_ms",
                )
                .map_err(plan_store_error)?;
            let rows = statement
                .query_map(params![now], |row| row.get::<_, String>(0))
                .map_err(plan_store_error)?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(plan_store_error)?
                .into_iter()
                .filter(|id| !exclude.contains(id))
                .take(available as usize)
                .collect()
        };
        let mut claimed = Vec::new();
        for id in ids {
            tx.execute(
                "UPDATE wc_goal_runs SET lease_owner = ?2,
                     lease_generation = lease_generation + 1,
                     lease_expires_at_unix_ms = ?3,
                     state = 'running', updated_at_unix_ms = ?4
                 WHERE goal_run_id = ?1",
                params![id, owner, now + lease_ms, now],
            )
            .map_err(plan_store_error)?;
            let run = load_run(&tx, &id)?;
            append_event(
                &tx,
                &id,
                now,
                "lease_claimed",
                &serde_json::json!({"generation": run.lease_generation}),
            )?;
            let lease = GoalLease {
                owner: owner.to_string(),
                generation: run.lease_generation,
            };
            claimed.push((run, lease));
        }
        tx.commit().map_err(plan_store_error)?;
        Ok(claimed)
    }

    pub fn goal_run_renew_lease(
        &self,
        run_id: &str,
        lease: &GoalLease,
        now: i64,
        lease_ms: i64,
    ) -> Result<(), PlanStoreError> {
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        let changed = conn
            .execute(
                "UPDATE wc_goal_runs SET lease_expires_at_unix_ms = ?4
                 WHERE goal_run_id = ?1 AND lease_owner = ?2 AND lease_generation = ?3
                   AND state = 'running'",
                params![run_id, lease.owner, lease.generation, now + lease_ms],
            )
            .map_err(plan_store_error)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(lease_lost())
        }
    }

    /// Fenced read used by the supervisor at every safe point.
    pub fn goal_run_fenced(
        &self,
        run_id: &str,
        lease: &GoalLease,
    ) -> Result<GoalRunRecord, PlanStoreError> {
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        let run = load_run(&conn, run_id)?;
        if run.lease_owner.as_deref() != Some(lease.owner.as_str())
            || run.lease_generation != lease.generation
        {
            return Err(lease_lost());
        }
        Ok(run)
    }

    /// Fenced terminal or parking transition. Releases the lease.
    pub fn goal_run_transition(
        &self,
        run_id: &str,
        lease: &GoalLease,
        state: GoalRunState,
        reason: &str,
        now: i64,
    ) -> Result<GoalRunRecord, PlanStoreError> {
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        // A cancel the user requested must not be lost because the driver
        // parked for another reason at the same time.
        let pending_cancel: Option<String> = tx
            .query_row(
                "SELECT control_request FROM wc_goal_runs WHERE goal_run_id = ?1",
                params![run_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(plan_store_error)?
            .flatten();
        let (state, reason) = if pending_cancel.as_deref() == Some("cancel")
            && matches!(state, GoalRunState::Paused | GoalRunState::NeedsAttention)
        {
            (GoalRunState::Cancelled, format!("cancel requested (was: {reason})"))
        } else {
            (state, reason.to_string())
        };
        let reason = reason.as_str();
        let finished = state.terminal().then_some(now);
        let changed = tx
            .execute(
                "UPDATE wc_goal_runs SET state = ?4, state_reason = ?5, control_request = NULL,
                     lease_owner = NULL, lease_expires_at_unix_ms = NULL,
                     updated_at_unix_ms = ?6, finished_at_unix_ms = COALESCE(?7, finished_at_unix_ms)
                 WHERE goal_run_id = ?1 AND lease_owner = ?2 AND lease_generation = ?3
                   AND state = 'running'",
                params![
                    run_id,
                    lease.owner,
                    lease.generation,
                    state.as_str(),
                    reason,
                    now,
                    finished
                ],
            )
            .map_err(plan_store_error)?;
        if changed != 1 {
            return Err(lease_lost());
        }
        append_event(
            &tx,
            run_id,
            now,
            "state",
            &serde_json::json!({"state": state.as_str(), "reason": reason}),
        )?;
        let run = load_run(&tx, run_id)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(run)
    }

    /// Fenced progress update after an iteration.
    pub fn goal_run_progress(
        &self,
        run_id: &str,
        lease: &GoalLease,
        iteration: i64,
        same_failure_streak: i64,
        last_failure_digest: Option<&str>,
        tokens_used: Option<i64>,
        now: i64,
    ) -> Result<(), PlanStoreError> {
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        let changed = conn
            .execute(
                "UPDATE wc_goal_runs SET iteration = ?4, same_failure_streak = ?5,
                     last_failure_digest = ?6,
                     tokens_used = COALESCE(?7, tokens_used),
                     updated_at_unix_ms = ?8
                 WHERE goal_run_id = ?1 AND lease_owner = ?2 AND lease_generation = ?3
                   AND state = 'running'",
                params![
                    run_id,
                    lease.owner,
                    lease.generation,
                    iteration,
                    same_failure_streak,
                    last_failure_digest,
                    tokens_used,
                    now
                ],
            )
            .map_err(plan_store_error)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(lease_lost())
        }
    }

    pub fn goal_run_set_workspace(
        &self,
        run_id: &str,
        lease: &GoalLease,
        project_id: &str,
        path: &str,
        branch: &str,
        now: i64,
    ) -> Result<(), PlanStoreError> {
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        let changed = conn
            .execute(
                "UPDATE wc_goal_runs SET workspace_project_id = ?4, workspace_path = ?5,
                     workspace_branch = ?6, updated_at_unix_ms = ?7
                 WHERE goal_run_id = ?1 AND lease_owner = ?2 AND lease_generation = ?3",
                params![run_id, lease.owner, lease.generation, project_id, path, branch, now],
            )
            .map_err(plan_store_error)?;
        if changed == 1 {
            Ok(())
        } else {
            Err(lease_lost())
        }
    }

    pub fn goal_op_get(
        &self,
        run_id: &str,
        op_key: &str,
    ) -> Result<Option<GoalOpRecord>, PlanStoreError> {
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        load_op(&conn, run_id, op_key)
    }

    /// Record the intent before anything leaves the process. Fails if the op
    /// already exists, so the caller must reconcile instead of re-dispatching.
    pub fn goal_op_intent(
        &self,
        run_id: &str,
        lease: &GoalLease,
        op_key: &str,
        kind: &str,
        iteration: i64,
        now: i64,
    ) -> Result<(), PlanStoreError> {
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        fence(&tx, run_id, lease)?;
        if load_op(&tx, run_id, op_key)?.is_some() {
            return Err(PlanStoreError::new(
                "goal_op_exists",
                format!("Goal operation {op_key} already exists; reconcile it instead"),
            ));
        }
        tx.execute(
            "INSERT INTO wc_goal_run_ops (
                goal_run_id, op_key, kind, iteration, state, job_id, result_json,
                lease_generation, started_at_unix_ms, finished_at_unix_ms
             ) VALUES (?1, ?2, ?3, ?4, 'intent', NULL, NULL, ?5, ?6, NULL)",
            params![run_id, op_key, kind, iteration, lease.generation, now],
        )
        .map_err(plan_store_error)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(())
    }

    pub fn goal_op_dispatched(
        &self,
        run_id: &str,
        lease: &GoalLease,
        op_key: &str,
        job_id: &str,
    ) -> Result<(), PlanStoreError> {
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        fence(&tx, run_id, lease)?;
        tx.execute(
            "UPDATE wc_goal_run_ops SET state = 'dispatched', job_id = ?3
             WHERE goal_run_id = ?1 AND op_key = ?2 AND state IN ('intent', 'dispatched')",
            params![run_id, op_key, job_id],
        )
        .map_err(plan_store_error)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(())
    }

    pub fn goal_op_finish(
        &self,
        run_id: &str,
        lease: &GoalLease,
        op_key: &str,
        state: GoalOpState,
        result: &Value,
        now: i64,
    ) -> Result<(), PlanStoreError> {
        let mut result_json = serde_json::to_string(result).expect("op result serializes");
        if result_json.len() > MAX_OP_RESULT_BYTES {
            result_json = serde_json::json!({"truncated": true}).to_string();
        }
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        fence(&tx, run_id, lease)?;
        tx.execute(
            "UPDATE wc_goal_run_ops SET state = ?3, result_json = ?4, finished_at_unix_ms = ?5
             WHERE goal_run_id = ?1 AND op_key = ?2 AND state IN ('intent', 'dispatched')",
            params![run_id, op_key, state.as_str(), result_json, now],
        )
        .map_err(plan_store_error)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(())
    }

    pub fn goal_run_event(
        &self,
        run_id: &str,
        kind: &str,
        detail: &Value,
        now: i64,
    ) -> Result<(), PlanStoreError> {
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        append_event(&conn, run_id, now, kind, detail)
    }

    /// Owner (ChatGPT) control: pause, cancel, or resume a user-paused run.
    /// Resuming from `needs_attention` is operator-only.
    pub fn goal_run_control(
        &self,
        reader: PlanReader<'_>,
        run_id: &str,
        action: GoalControlAction,
        now: i64,
    ) -> Result<GoalRunRecord, PlanStoreError> {
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        let run = load_visible_run(&tx, reader, run_id)?;
        if run.state.terminal() {
            return Err(PlanStoreError::new(
                "goal_run_terminal",
                format!("Goal run is already {}", run.state.as_str()),
            ));
        }
        let detail = match action {
            GoalControlAction::Pause | GoalControlAction::Cancel => {
                // Pausing is only a way to stop active work. It must never turn
                // a run that needs attention into a resumable one.
                if action == GoalControlAction::Pause
                    && !matches!(run.state, GoalRunState::Queued | GoalRunState::Running)
                {
                    return Err(PlanStoreError::new(
                        "goal_pause_not_allowed",
                        format!("A {} run cannot be paused", run.state.as_str()),
                    ));
                }
                let request = if action == GoalControlAction::Pause {
                    "pause"
                } else {
                    "cancel"
                };
                if run.state == GoalRunState::Running {
                    // The supervisor applies it at the next safe point and
                    // never dispatches after observing it.
                    tx.execute(
                        "UPDATE wc_goal_runs SET control_request = ?2, updated_at_unix_ms = ?3
                         WHERE goal_run_id = ?1",
                        params![run_id, request, now],
                    )
                    .map_err(plan_store_error)?;
                } else {
                    // Not leased: apply directly.
                    let (state, finished) = if action == GoalControlAction::Cancel {
                        ("cancelled", Some(now))
                    } else {
                        ("paused", None)
                    };
                    tx.execute(
                        "UPDATE wc_goal_runs SET state = ?2, state_reason = ?3,
                             control_request = NULL, lease_owner = NULL,
                             lease_expires_at_unix_ms = NULL, updated_at_unix_ms = ?4,
                             finished_at_unix_ms = ?5
                         WHERE goal_run_id = ?1",
                        params![run_id, state, format!("{request} requested"), now, finished],
                    )
                    .map_err(plan_store_error)?;
                }
                request
            }
            GoalControlAction::Resume => {
                let allowed = match run.state {
                    GoalRunState::Paused => true,
                    GoalRunState::NeedsAttention => reader == PlanReader::Operator,
                    _ => false,
                };
                if !allowed {
                    return Err(PlanStoreError::new(
                        "goal_resume_not_allowed",
                        if run.state == GoalRunState::NeedsAttention {
                            "This run needs attention; only the Chadex app can resume it after review"
                        } else {
                            "Only paused runs can be resumed"
                        },
                    ));
                }
                if kill_switch_engaged(&tx)? {
                    return Err(PlanStoreError::new(
                        "goal_emergency_stop_engaged",
                        "Goal emergency stop is engaged in the Chadex app",
                    ));
                }
                tx.execute(
                    "UPDATE wc_goal_runs SET state = 'queued', state_reason = 'resumed',
                         control_request = NULL, updated_at_unix_ms = ?2
                     WHERE goal_run_id = ?1",
                    params![run_id, now],
                )
                .map_err(plan_store_error)?;
                "resume"
            }
        };
        append_event(
            &tx,
            run_id,
            now,
            "control",
            &serde_json::json!({
                "action": detail,
                "by": if reader == PlanReader::Operator { "chadex_app" } else { "owner" },
            }),
        )?;
        let run = load_run(&tx, run_id)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(run)
    }

    /// Operator kill switch. Engaging it blocks every new claim and dispatch
    /// and requests cancellation of every non-terminal run.
    pub fn goal_kill_switch_set(
        &self,
        engaged: bool,
        reason: &str,
        now: i64,
    ) -> Result<usize, PlanStoreError> {
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        tx.execute(
            "INSERT INTO wc_goal_kill_switch (id, engaged, reason, updated_at_unix_ms)
             VALUES (1, ?1, ?2, ?3)
             ON CONFLICT(id) DO UPDATE SET engaged = ?1, reason = ?2, updated_at_unix_ms = ?3",
            params![engaged as i64, reason, now],
        )
        .map_err(plan_store_error)?;
        let mut affected = 0;
        if engaged {
            let ids: Vec<(String, String)> = {
                let mut statement = tx
                    .prepare(
                        "SELECT goal_run_id, state FROM wc_goal_runs
                         WHERE state IN ('queued', 'running', 'paused', 'needs_attention')",
                    )
                    .map_err(plan_store_error)?;
                let rows = statement
                    .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
                    .map_err(plan_store_error)?;
                rows.collect::<Result<_, _>>().map_err(plan_store_error)?
            };
            for (id, state) in ids {
                if state == "running" {
                    tx.execute(
                        "UPDATE wc_goal_runs SET control_request = 'cancel', updated_at_unix_ms = ?2
                         WHERE goal_run_id = ?1",
                        params![id, now],
                    )
                    .map_err(plan_store_error)?;
                } else {
                    tx.execute(
                        "UPDATE wc_goal_runs SET state = 'stopped', state_reason = 'emergency_stop',
                             control_request = NULL, lease_owner = NULL,
                             lease_expires_at_unix_ms = NULL, updated_at_unix_ms = ?2,
                             finished_at_unix_ms = ?2
                         WHERE goal_run_id = ?1",
                        params![id, now],
                    )
                    .map_err(plan_store_error)?;
                }
                append_event(
                    &tx,
                    &id,
                    now,
                    "emergency_stop",
                    &serde_json::json!({"reason": reason}),
                )?;
                affected += 1;
            }
        }
        tx.commit().map_err(plan_store_error)?;
        Ok(affected)
    }

    /// Test support: move a run's wall-clock deadline.
    #[doc(hidden)]
    pub fn goal_run_override_deadline(&self, run_id: &str, deadline: i64) -> Result<(), PlanStoreError> {
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        conn.execute(
            "UPDATE wc_goal_runs SET deadline_at_unix_ms = ?2 WHERE goal_run_id = ?1",
            params![run_id, deadline],
        )
        .map_err(plan_store_error)?;
        Ok(())
    }

    pub fn goal_kill_switch_engaged(&self) -> Result<bool, PlanStoreError> {
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        kill_switch_engaged(&conn)
    }

    pub fn goal_run_read(
        &self,
        reader: PlanReader<'_>,
        run_id: &str,
    ) -> Result<GoalRunDetail, PlanStoreError> {
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        let run = load_visible_run(&conn, reader, run_id)?;
        let ops = {
            let mut statement = conn
                .prepare(
                    "SELECT op_key, kind, iteration, state, job_id, result_json,
                            started_at_unix_ms, finished_at_unix_ms
                     FROM wc_goal_run_ops WHERE goal_run_id = ?1
                     ORDER BY started_at_unix_ms, op_key",
                )
                .map_err(plan_store_error)?;
            let rows = statement
                .query_map(params![run_id], op_from_row)
                .map_err(plan_store_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(plan_store_error)?
        };
        let events = {
            let mut statement = conn
                .prepare(
                    "SELECT seq, at_unix_ms, kind, detail_json FROM wc_goal_run_events
                     WHERE goal_run_id = ?1 ORDER BY seq DESC LIMIT ?2",
                )
                .map_err(plan_store_error)?;
            let rows = statement
                .query_map(params![run_id, MAX_GOAL_RUN_EVENTS_RETURNED as i64], |row| {
                    let detail: String = row.get(3)?;
                    Ok(GoalEventRecord {
                        seq: row.get(0)?,
                        at_unix_ms: row.get(1)?,
                        kind: row.get(2)?,
                        detail: serde_json::from_str(&detail).unwrap_or(Value::Null),
                    })
                })
                .map_err(plan_store_error)?;
            let mut events = rows.collect::<Result<Vec<_>, _>>().map_err(plan_store_error)?;
            events.reverse();
            events
        };
        Ok(GoalRunDetail {
            run,
            ops,
            events,
            kill_switch_engaged: kill_switch_engaged(&conn)?,
        })
    }

    pub fn goal_run_list(
        &self,
        reader: PlanReader<'_>,
        limit: usize,
    ) -> Result<Vec<GoalRunRecord>, PlanStoreError> {
        let limit = limit.clamp(1, 50) as i64;
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        let owner = match reader {
            PlanReader::Owner(owner) => Some(owner.to_string()),
            PlanReader::Operator => None,
        };
        let ids: Vec<String> = {
            let mut statement = conn
                .prepare(
                    "SELECT goal_run_id FROM wc_goal_runs
                     WHERE (?1 IS NULL OR owner_key = ?1)
                     ORDER BY updated_at_unix_ms DESC, goal_run_id LIMIT ?2",
                )
                .map_err(plan_store_error)?;
            let rows = statement
                .query_map(params![owner, limit], |row| row.get(0))
                .map_err(plan_store_error)?;
            rows.collect::<Result<_, _>>().map_err(plan_store_error)?
        };
        ids.iter().map(|id| load_run(&conn, id)).collect()
    }
}

fn fence(conn: &Connection, run_id: &str, lease: &GoalLease) -> Result<(), PlanStoreError> {
    let held: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM wc_goal_runs WHERE goal_run_id = ?1
                 AND lease_owner = ?2 AND lease_generation = ?3 AND state = 'running')",
            params![run_id, lease.owner, lease.generation],
            |row| row.get(0),
        )
        .map_err(plan_store_error)?;
    if held {
        Ok(())
    } else {
        Err(lease_lost())
    }
}

fn kill_switch_engaged(conn: &Connection) -> Result<bool, PlanStoreError> {
    Ok(conn
        .query_row(
            "SELECT engaged FROM wc_goal_kill_switch WHERE id = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()
        .map_err(plan_store_error)?
        == Some(1))
}

fn append_event(
    conn: &Connection,
    run_id: &str,
    now: i64,
    kind: &str,
    detail: &Value,
) -> Result<(), PlanStoreError> {
    let mut detail_json = serde_json::to_string(detail).expect("event serializes");
    if detail_json.len() > MAX_GOAL_EVENT_DETAIL_BYTES {
        detail_json = serde_json::json!({"truncated": true}).to_string();
    }
    conn.execute(
        "INSERT INTO wc_goal_run_events (goal_run_id, seq, at_unix_ms, kind, detail_json)
         VALUES (?1, (SELECT COALESCE(MAX(seq), 0) + 1 FROM wc_goal_run_events WHERE goal_run_id = ?1),
                 ?2, ?3, ?4)",
        params![run_id, now, kind, detail_json],
    )
    .map_err(plan_store_error)?;
    Ok(())
}

fn op_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<GoalOpRecord> {
    let result: Option<String> = row.get(5)?;
    Ok(GoalOpRecord {
        op_key: row.get(0)?,
        kind: row.get(1)?,
        iteration: row.get(2)?,
        state: GoalOpState::from_db(&row.get::<_, String>(3)?)?,
        job_id: row.get(4)?,
        result: result.and_then(|value| serde_json::from_str(&value).ok()),
        started_at_unix_ms: row.get(6)?,
        finished_at_unix_ms: row.get(7)?,
    })
}

fn load_op(
    conn: &Connection,
    run_id: &str,
    op_key: &str,
) -> Result<Option<GoalOpRecord>, PlanStoreError> {
    conn.query_row(
        "SELECT op_key, kind, iteration, state, job_id, result_json,
                started_at_unix_ms, finished_at_unix_ms
         FROM wc_goal_run_ops WHERE goal_run_id = ?1 AND op_key = ?2",
        params![run_id, op_key],
        op_from_row,
    )
    .optional()
    .map_err(plan_store_error)
}

fn load_visible_run(
    conn: &Connection,
    reader: PlanReader<'_>,
    run_id: &str,
) -> Result<GoalRunRecord, PlanStoreError> {
    let run = load_run(conn, run_id)?;
    if let PlanReader::Owner(owner) = reader {
        if run.owner_key != owner {
            return Err(PlanStoreError::new("goal_run_not_found", "Goal run not found"));
        }
    }
    Ok(run)
}

pub(crate) fn load_run(conn: &Connection, run_id: &str) -> Result<GoalRunRecord, PlanStoreError> {
    conn.query_row(
        "SELECT goal_run_id, plan_id, revision, approval_id, content_sha256, owner_key,
                project_id, binding_json, spec_json, dispatch_auth_json, provider, state,
                state_reason, control_request, iteration, same_failure_streak,
                last_failure_digest, workspace_project_id, workspace_path, workspace_branch,
                tokens_used, lease_owner, lease_generation, lease_expires_at_unix_ms,
                created_at_unix_ms, deadline_at_unix_ms, updated_at_unix_ms, finished_at_unix_ms
         FROM wc_goal_runs WHERE goal_run_id = ?1",
        params![run_id],
        |row| {
            let binding: String = row.get(7)?;
            let spec: String = row.get(8)?;
            let auth: String = row.get(9)?;
            let corrupt = |index: usize| {
                move |_| {
                    rusqlite::Error::FromSqlConversionFailure(
                        index,
                        rusqlite::types::Type::Text,
                        "corrupt Goal run JSON".into(),
                    )
                }
            };
            Ok(GoalRunRecord {
                goal_run_id: row.get(0)?,
                plan_id: row.get(1)?,
                revision: row.get(2)?,
                approval_id: row.get(3)?,
                content_sha256: row.get(4)?,
                owner_key: row.get(5)?,
                project_id: row.get(6)?,
                binding: serde_json::from_str(&binding).map_err(corrupt(7))?,
                spec: serde_json::from_str(&spec).map_err(corrupt(8))?,
                dispatch_auth: serde_json::from_str(&auth).map_err(corrupt(9))?,
                provider: row.get(10)?,
                state: GoalRunState::from_db(&row.get::<_, String>(11)?)?,
                state_reason: row.get(12)?,
                control_request: row.get(13)?,
                iteration: row.get(14)?,
                same_failure_streak: row.get(15)?,
                last_failure_digest: row.get(16)?,
                workspace_project_id: row.get(17)?,
                workspace_path: row.get(18)?,
                workspace_branch: row.get(19)?,
                tokens_used: row.get(20)?,
                lease_owner: row.get(21)?,
                lease_generation: row.get(22)?,
                lease_expires_at_unix_ms: row.get(23)?,
                created_at_unix_ms: row.get(24)?,
                deadline_at_unix_ms: row.get(25)?,
                updated_at_unix_ms: row.get(26)?,
                finished_at_unix_ms: row.get(27)?,
            })
        },
    )
    .optional()
    .map_err(plan_store_error)?
    .ok_or_else(|| PlanStoreError::new("goal_run_not_found", "Goal run not found"))
}
