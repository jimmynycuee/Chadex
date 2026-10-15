//! Durable Goal Mode runs: one run per consumed plan approval, a lease with a
//! monotonically increasing fence for the single active supervisor, an
//! operation journal (intent -> dispatched -> terminal/unknown) that prevents
//! blind re-dispatch after a restart, an append-only event log, and a
//! persistent emergency stop.

use super::communication::allocate_identity;
use super::plan::{
    expire_approvals, load_decision, load_owned_plan, load_revision, plan_store_error,
    is_git_oid, is_sha256_hex, AcceptanceCheck, AcceptanceKind, GoalBudget, PlanApprovalBinding,
    PlanDecisionState, PlanReader, PlanSpec, PlanStoreError, WorkspaceDependency,
    WorkspaceRequirement,
};
use super::Database;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use uuid::Uuid;

pub const GOAL_RUN_ID_PREFIX: &str = "wc_grun_";
pub const GOAL_ACCEPTANCE_PROPOSAL_ID_PREFIX: &str = "wc_gprop_";
pub const GOAL_ACCEPTANCE_RECEIPT_ID_PREFIX: &str = "wc_greceipt_";
/// Approval binding of a plan approved while the project had no commit.
pub const UNBORN_BASE_COMMIT: &str = "unborn";
/// Provider name of a Goal whose agent work is done by ChatGPT itself.
pub const CHATGPT_GOAL_PROVIDER: &str = "chatgpt";
/// A ChatGPT-mode run with no goal_check for this long is paused.
pub const CHATGPT_GOAL_IDLE_MS: i64 = 30 * 60 * 1000;
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
    pub checkpoint_sha: Option<String>,
    pub committed_rounds: i64,
    pub completed_acceptance_rounds: i64,
    pub current_attempt: i64,
    pub acceptance_revision: i64,
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
    /// ChatGPT mode: the iteration ChatGPT has finished editing for (the
    /// supervisor checks it once `iteration` catches up).
    pub chatgpt_ready_iteration: i64,
    pub chatgpt_seen_at_unix_ms: Option<i64>,
    /// The last iteration whose result goal_check showed to ChatGPT. A new
    /// round starts only after ChatGPT has seen the previous one.
    pub chatgpt_reported_iteration: i64,
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
    pub acceptance_revisions: Vec<GoalAcceptanceRevisionRecord>,
    pub acceptance_receipts: Vec<GoalAcceptanceReceiptRecord>,
    pub pending_proposals: Vec<GoalAcceptanceRevisionProposal>,
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
    /// Operator-only retry of the accepted checks at the stored checkpoint.
    RetryAcceptance,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalStartEnvironment {
    pub runner_client_id: String,
    pub project_root: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoalAcceptanceOutcome {
    Passed,
    Failed,
    BlockedExternal,
    Unknown,
}

impl GoalAcceptanceOutcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::BlockedExternal => "blocked_external",
            Self::Unknown => "unknown",
        }
    }

    fn from_db(value: &str) -> rusqlite::Result<Self> {
        match value {
            "passed" => Ok(Self::Passed),
            "failed" => Ok(Self::Failed),
            "blocked_external" => Ok(Self::BlockedExternal),
            "unknown" => Ok(Self::Unknown),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                format!("unsupported acceptance receipt outcome: {other}").into(),
            )),
        }
    }
}

/// Trusted Runner observation passed to the fenced Store receipt API.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalAcceptanceReceiptObservation {
    pub acceptance_revision: i64,
    pub checkpoint_round: i64,
    pub checkpoint_sha: String,
    pub check_name: String,
    pub argv_sha256: String,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub output_sha256: String,
    pub observed_at_unix_ms: i64,
    #[serde(default)]
    pub external_attestation: Option<Value>,
}

/// Immutable proof created by the Store from a trusted Runner observation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalAcceptanceReceiptRecord {
    pub receipt_id: String,
    pub goal_run_id: String,
    pub acceptance_revision: i64,
    pub checkpoint_round: i64,
    pub checkpoint_sha: String,
    pub attempt: i64,
    pub check_name: String,
    pub argv_sha256: String,
    pub outcome: GoalAcceptanceOutcome,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub output_sha256: String,
    pub observed_at_unix_ms: i64,
    #[serde(skip_serializing)]
    pub proof_hmac_sha256: String,
}

/// Durable proposal from the plan owner. It has no effect until an operator
/// approves it against the same checkpoint, revision, project, Runner, and root.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalAcceptanceRevisionProposal {
    pub proposal_id: String,
    pub goal_run_id: String,
    pub source_acceptance_revision: i64,
    pub checkpoint_sha: String,
    pub source_acceptance_sha256: String,
    pub proposed_acceptance_sha256: String,
    pub acceptance: Vec<AcceptanceCheck>,
    pub workspace_requirements: Vec<WorkspaceRequirement>,
    pub dependencies: Vec<WorkspaceDependency>,
    pub binding: PlanApprovalBinding,
    pub proposer: GoalAcceptanceProposer,
    pub proposed_at_unix_ms: i64,
    #[serde(skip_serializing)]
    pub proposer_owner_key: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GoalAcceptanceProposer {
    Owner,
    Operator,
}

/// Append-only acceptance revision and the trusted operator approval binding.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GoalAcceptanceRevisionRecord {
    pub goal_run_id: String,
    pub revision: i64,
    pub checkpoint_sha: Option<String>,
    pub source_revision: Option<i64>,
    pub source_acceptance_sha256: String,
    pub acceptance_sha256: String,
    pub acceptance: Vec<AcceptanceCheck>,
    pub workspace_requirements: Vec<WorkspaceRequirement>,
    pub dependencies: Vec<WorkspaceDependency>,
    pub binding: PlanApprovalBinding,
    pub approver: String,
    pub approved_at_unix_ms: i64,
    pub proposal_id: Option<String>,
}

fn lease_lost() -> PlanStoreError {
    PlanStoreError::new(
        "goal_lease_lost",
        "Goal run lease is no longer held by this supervisor",
    )
}

fn legacy_acceptance_sha256(acceptance: &[AcceptanceCheck]) -> Result<String, PlanStoreError> {
    let encoded = serde_json::to_vec(acceptance)
        .map_err(|_| PlanStoreError::new("goal_acceptance_invalid", "Acceptance checks could not be encoded"))?;
    let mut hasher = Sha256::new();
    hasher.update(b"chadex.goal.acceptance.v1\0");
    hasher.update(encoded);
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Serialize)]
struct AcceptanceSpecHashInput<'a> {
    acceptance: &'a [AcceptanceCheck],
    workspace_requirements: &'a [WorkspaceRequirement],
    dependencies: &'a [WorkspaceDependency],
}

/// Hash the complete approved acceptance policy, including workspace inputs
/// and dependency provenance. Hashes a normalized, stable JSON projection.
pub fn goal_acceptance_spec_sha256(
    acceptance: &[AcceptanceCheck],
    workspace_requirements: &[WorkspaceRequirement],
    dependencies: &[WorkspaceDependency],
) -> Result<String, PlanStoreError> {
    let normalized = PlanSpec {
        acceptance: acceptance.to_vec(),
        budget: GoalBudget::default(),
        workspace_requirements: workspace_requirements.to_vec(),
        dependencies: dependencies.to_vec(),
    }
    .normalized()?;
    let encoded = serde_json::to_vec(&AcceptanceSpecHashInput {
        acceptance: &normalized.acceptance,
        workspace_requirements: &normalized.workspace_requirements,
        dependencies: &normalized.dependencies,
    })
    .map_err(|_| PlanStoreError::new("goal_acceptance_invalid", "Acceptance policy could not be encoded"))?;
    let mut hasher = Sha256::new();
    hasher.update(b"chadex.goal.acceptance.spec.v1\0");
    hasher.update(encoded);
    Ok(format!("{:x}", hasher.finalize()))
}

/// Legacy acceptance-array-only digest. Use `goal_acceptance_spec_sha256`
/// for all new proposals and revisions.
pub fn goal_acceptance_sha256(acceptance: &[AcceptanceCheck]) -> Result<String, PlanStoreError> {
    legacy_acceptance_sha256(acceptance)
}

/// Stable digest shared by the trusted Runner and the Store receipt verifier.
pub fn goal_acceptance_argv_sha256(argv: &[String]) -> Result<String, PlanStoreError> {
    let encoded = serde_json::to_vec(argv)
        .map_err(|_| PlanStoreError::new("goal_acceptance_invalid", "Acceptance argv could not be encoded"))?;
    let mut hasher = Sha256::new();
    hasher.update(b"chadex.goal.acceptance.argv.v1\0");
    hasher.update(encoded);
    Ok(format!("{:x}", hasher.finalize()))
}

#[derive(Serialize)]
struct ReceiptMacFields<'a> {
    receipt_id: &'a str,
    goal_run_id: &'a str,
    acceptance_revision: i64,
    checkpoint_round: i64,
    #[serde(rename = "checkpoint_sha256")]
    checkpoint_sha: &'a str,
    attempt: i64,
    check_name: &'a str,
    argv_sha256: &'a str,
    outcome: GoalAcceptanceOutcome,
    exit_code: Option<i32>,
    timed_out: bool,
    output_sha256: &'a str,
    observed_at_unix_ms: i64,
}

fn receipt_mac_fields<'a>(record: &'a GoalAcceptanceReceiptRecord) -> ReceiptMacFields<'a> {
    ReceiptMacFields {
        receipt_id: &record.receipt_id,
        goal_run_id: &record.goal_run_id,
        acceptance_revision: record.acceptance_revision,
        checkpoint_round: record.checkpoint_round,
        checkpoint_sha: &record.checkpoint_sha,
        attempt: record.attempt,
        check_name: &record.check_name,
        argv_sha256: &record.argv_sha256,
        outcome: record.outcome,
        exit_code: record.exit_code,
        timed_out: record.timed_out,
        output_sha256: &record.output_sha256,
        observed_at_unix_ms: record.observed_at_unix_ms,
    }
}

pub(super) fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; 32] {
    const BLOCK: usize = 64;
    let mut normalized = [0_u8; BLOCK];
    if key.len() > BLOCK {
        let digest = Sha256::digest(key);
        normalized[..digest.len()].copy_from_slice(&digest);
    } else {
        normalized[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(BLOCK + message.len());
    let mut outer = Vec::with_capacity(BLOCK + 32);
    for byte in normalized {
        inner.push(byte ^ 0x36);
        outer.push(byte ^ 0x5c);
    }
    inner.extend_from_slice(message);
    outer.extend_from_slice(&Sha256::digest(&inner));
    Sha256::digest(&outer).into()
}

pub(super) fn hex_encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(DIGITS[(byte >> 4) as usize] as char);
        encoded.push(DIGITS[(byte & 0x0f) as usize] as char);
    }
    encoded
}

fn constant_time_hex_eq(expected: &[u8; 32], actual_hex: &str) -> bool {
    if actual_hex.len() != 64 {
        return false;
    }
    let mut difference = 0_u8;
    for (index, expected_byte) in expected.iter().enumerate() {
        let Some(actual_byte) = u8::from_str_radix(&actual_hex[index * 2..index * 2 + 2], 16).ok() else {
            return false;
        };
        difference |= expected_byte ^ actual_byte;
    }
    difference == 0
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
                checkpoint_sha TEXT,
                committed_rounds INTEGER NOT NULL DEFAULT 0,
                completed_acceptance_rounds INTEGER NOT NULL DEFAULT 0,
                current_attempt INTEGER NOT NULL DEFAULT 0,
                acceptance_revision INTEGER NOT NULL DEFAULT 1,
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

            CREATE TABLE IF NOT EXISTS wc_goal_checkpoint_adoptions (
                goal_run_id TEXT NOT NULL,
                checkpoint_round INTEGER NOT NULL,
                commit_op_key TEXT NOT NULL,
                checkpoint_sha TEXT NOT NULL,
                commit_subject TEXT NOT NULL,
                binding_json TEXT NOT NULL,
                approver TEXT NOT NULL,
                adopted_at_unix_ms INTEGER NOT NULL,
                PRIMARY KEY(goal_run_id, checkpoint_round),
                FOREIGN KEY(goal_run_id) REFERENCES wc_goal_runs(goal_run_id)
            );
            CREATE TRIGGER IF NOT EXISTS trg_wc_goal_checkpoint_adoptions_immutable_update
                BEFORE UPDATE ON wc_goal_checkpoint_adoptions
                BEGIN SELECT RAISE(ABORT, 'Goal checkpoint adoptions are immutable'); END;
            CREATE TRIGGER IF NOT EXISTS trg_wc_goal_checkpoint_adoptions_immutable_delete
                BEFORE DELETE ON wc_goal_checkpoint_adoptions
                BEGIN SELECT RAISE(ABORT, 'Goal checkpoint adoptions are immutable'); END;

            CREATE TABLE IF NOT EXISTS wc_goal_kill_switch (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                engaged INTEGER NOT NULL CHECK(engaged IN (0, 1)),
                reason TEXT,
                updated_at_unix_ms INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS wc_goal_acceptance_proposals (
                proposal_id TEXT PRIMARY KEY,
                goal_run_id TEXT NOT NULL,
                source_acceptance_revision INTEGER NOT NULL,
                checkpoint_sha TEXT NOT NULL,
                source_acceptance_sha256 TEXT NOT NULL,
                proposed_acceptance_sha256 TEXT NOT NULL,
                acceptance_json TEXT NOT NULL,
                workspace_requirements_json TEXT NOT NULL,
                dependencies_json TEXT NOT NULL,
                proposer_owner_key TEXT NOT NULL,
                proposer_role TEXT NOT NULL DEFAULT 'owner',
                binding_json TEXT NOT NULL DEFAULT '{}',
                proposed_at_unix_ms INTEGER NOT NULL,
                FOREIGN KEY(goal_run_id) REFERENCES wc_goal_runs(goal_run_id)
            );
            CREATE INDEX IF NOT EXISTS idx_wc_goal_acceptance_proposals_run
                ON wc_goal_acceptance_proposals(goal_run_id, proposed_at_unix_ms);
            CREATE TRIGGER IF NOT EXISTS trg_wc_goal_acceptance_proposals_immutable_update
                BEFORE UPDATE ON wc_goal_acceptance_proposals
                BEGIN SELECT RAISE(ABORT, 'Goal acceptance proposals are immutable'); END;
            CREATE TRIGGER IF NOT EXISTS trg_wc_goal_acceptance_proposals_immutable_delete
                BEFORE DELETE ON wc_goal_acceptance_proposals
                BEGIN SELECT RAISE(ABORT, 'Goal acceptance proposals are immutable'); END;

            CREATE TABLE IF NOT EXISTS wc_goal_acceptance_revisions (
                goal_run_id TEXT NOT NULL,
                revision INTEGER NOT NULL CHECK(revision >= 1),
                plan_id TEXT NOT NULL,
                plan_revision INTEGER NOT NULL,
                checkpoint_sha TEXT,
                source_revision INTEGER,
                source_acceptance_sha256 TEXT NOT NULL,
                acceptance_sha256 TEXT NOT NULL,
                acceptance_json TEXT NOT NULL,
                workspace_requirements_json TEXT NOT NULL,
                dependencies_json TEXT NOT NULL,
                binding_json TEXT NOT NULL,
                approver TEXT NOT NULL,
                approved_at_unix_ms INTEGER NOT NULL,
                proposal_id TEXT UNIQUE,
                PRIMARY KEY(goal_run_id, revision),
                FOREIGN KEY(goal_run_id) REFERENCES wc_goal_runs(goal_run_id),
                FOREIGN KEY(proposal_id) REFERENCES wc_goal_acceptance_proposals(proposal_id)
            );
            CREATE TRIGGER IF NOT EXISTS trg_wc_goal_acceptance_revisions_immutable_update
                BEFORE UPDATE ON wc_goal_acceptance_revisions
                BEGIN SELECT RAISE(ABORT, 'Goal acceptance revisions are immutable'); END;
            CREATE TRIGGER IF NOT EXISTS trg_wc_goal_acceptance_revisions_immutable_delete
                BEFORE DELETE ON wc_goal_acceptance_revisions
                BEGIN SELECT RAISE(ABORT, 'Goal acceptance revisions are immutable'); END;

            CREATE TABLE IF NOT EXISTS wc_goal_acceptance_receipt_mac_key (
                id INTEGER PRIMARY KEY CHECK(id = 1),
                secret BLOB NOT NULL CHECK(length(secret) >= 32),
                created_at_unix_ms INTEGER NOT NULL
            );
            CREATE TRIGGER IF NOT EXISTS trg_wc_goal_acceptance_receipt_mac_key_immutable_update
                BEFORE UPDATE ON wc_goal_acceptance_receipt_mac_key
                BEGIN SELECT RAISE(ABORT, 'Goal acceptance receipt key is immutable'); END;
            CREATE TRIGGER IF NOT EXISTS trg_wc_goal_acceptance_receipt_mac_key_immutable_delete
                BEFORE DELETE ON wc_goal_acceptance_receipt_mac_key
                BEGIN SELECT RAISE(ABORT, 'Goal acceptance receipt key is immutable'); END;

            CREATE TABLE IF NOT EXISTS wc_goal_acceptance_receipts (
                receipt_id TEXT PRIMARY KEY,
                goal_run_id TEXT NOT NULL,
                acceptance_revision INTEGER NOT NULL,
                checkpoint_round INTEGER NOT NULL,
                checkpoint_sha TEXT NOT NULL,
                attempt INTEGER NOT NULL,
                check_name TEXT NOT NULL,
                argv_sha256 TEXT NOT NULL,
                outcome TEXT NOT NULL CHECK(outcome IN ('passed', 'failed', 'blocked_external', 'unknown')),
                exit_code INTEGER,
                timed_out INTEGER NOT NULL CHECK(timed_out IN (0, 1)),
                output_sha256 TEXT NOT NULL,
                observed_at_unix_ms INTEGER NOT NULL,
                proof_hmac_sha256 TEXT NOT NULL,
                FOREIGN KEY(goal_run_id) REFERENCES wc_goal_runs(goal_run_id)
            );
            CREATE INDEX IF NOT EXISTS idx_wc_goal_acceptance_receipts_scope
                ON wc_goal_acceptance_receipts(
                    goal_run_id, acceptance_revision, checkpoint_round, attempt, check_name, observed_at_unix_ms
                );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_wc_goal_acceptance_receipts_once
                ON wc_goal_acceptance_receipts(
                    goal_run_id, acceptance_revision, checkpoint_round, attempt, check_name
                );
            CREATE TRIGGER IF NOT EXISTS trg_wc_goal_acceptance_receipts_immutable_update
                BEFORE UPDATE ON wc_goal_acceptance_receipts
                BEGIN SELECT RAISE(ABORT, 'Goal acceptance receipts are immutable'); END;
            CREATE TRIGGER IF NOT EXISTS trg_wc_goal_acceptance_receipts_immutable_delete
                BEFORE DELETE ON wc_goal_acceptance_receipts
                BEGIN SELECT RAISE(ABORT, 'Goal acceptance receipts are immutable'); END;

            CREATE TABLE IF NOT EXISTS wc_goal_acceptance_rounds (
                goal_run_id TEXT NOT NULL,
                acceptance_revision INTEGER NOT NULL,
                checkpoint_round INTEGER NOT NULL,
                checkpoint_sha TEXT NOT NULL,
                attempt INTEGER NOT NULL,
                completed_at_unix_ms INTEGER NOT NULL,
                PRIMARY KEY(goal_run_id, acceptance_revision, checkpoint_round, attempt),
                FOREIGN KEY(goal_run_id) REFERENCES wc_goal_runs(goal_run_id)
            );
            CREATE TRIGGER IF NOT EXISTS trg_wc_goal_acceptance_rounds_immutable_update
                BEFORE UPDATE ON wc_goal_acceptance_rounds
                BEGIN SELECT RAISE(ABORT, 'Goal acceptance rounds are immutable'); END;
            CREATE TRIGGER IF NOT EXISTS trg_wc_goal_acceptance_rounds_immutable_delete
                BEFORE DELETE ON wc_goal_acceptance_rounds
                BEGIN SELECT RAISE(ABORT, 'Goal acceptance rounds are immutable'); END;
            ",
        )?;
        // This Store schema name is a Git object ID, not a SHA-256 digest.
        // Preserve old run rows without promoting their old, differently
        // defined checkpoint_sha256 value to a trusted Git checkpoint.
        for table in [
            "wc_goal_acceptance_proposals",
            "wc_goal_acceptance_revisions",
            "wc_goal_acceptance_receipts",
            "wc_goal_acceptance_rounds",
        ] {
            let has_legacy_column: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = 'checkpoint_sha256')",
                params![table],
                |row| row.get(0),
            )?;
            let has_git_oid_column: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info(?1) WHERE name = 'checkpoint_sha')",
                params![table],
                |row| row.get(0),
            )?;
            if has_legacy_column && !has_git_oid_column {
                conn.execute_batch(&format!(
                    "ALTER TABLE {table} RENAME COLUMN checkpoint_sha256 TO checkpoint_sha"
                ))?;
            }
        }
        for (column, definition) in [
            ("proposer_role", "TEXT NOT NULL DEFAULT 'owner'"),
            ("binding_json", "TEXT NOT NULL DEFAULT '{}'"),
        ] {
            let exists: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('wc_goal_acceptance_proposals') WHERE name = ?1)",
                params![column],
                |row| row.get(0),
            )?;
            if !exists {
                conn.execute_batch(&format!(
                    "ALTER TABLE wc_goal_acceptance_proposals ADD COLUMN {column} {definition}"
                ))?;
            }
        }
        // Added later: a finished run the operator removed from the lists.
        let dismissed_exists: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('wc_goal_runs') WHERE name = 'dismissed_at_unix_ms')",
            [],
            |row| row.get(0),
        )?;
        if !dismissed_exists {
            conn.execute_batch("ALTER TABLE wc_goal_runs ADD COLUMN dismissed_at_unix_ms INTEGER")?;
        }
        // Added later: ChatGPT-mode turn handshake (see goal_run_chatgpt_ready).
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (column, definition) in [
            ("chatgpt_ready_iteration", "INTEGER NOT NULL DEFAULT 0"),
            ("chatgpt_seen_at_unix_ms", "INTEGER"),
            ("chatgpt_reported_iteration", "INTEGER NOT NULL DEFAULT 0"),
            ("checkpoint_sha", "TEXT"),
            ("committed_rounds", "INTEGER NOT NULL DEFAULT 0"),
            ("completed_acceptance_rounds", "INTEGER NOT NULL DEFAULT 0"),
            ("current_attempt", "INTEGER NOT NULL DEFAULT 0"),
            ("acceptance_revision", "INTEGER NOT NULL DEFAULT 1"),
        ] {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('wc_goal_runs') WHERE name = ?1)",
                params![column],
                |row| row.get(0),
            )?;
            if !exists {
                tx.execute_batch(&format!("ALTER TABLE wc_goal_runs ADD COLUMN {column} {definition}"))?;
            }
        }
        // The operation journal is the only legacy evidence that a commit
        // completed. It can recover the count, never the Runner-verified HEAD.
        tx.execute(
            "UPDATE wc_goal_runs SET committed_rounds = MAX(
                 committed_rounds,
                 COALESCE((SELECT MAX(iteration) FROM wc_goal_run_ops
                           WHERE goal_run_id = wc_goal_runs.goal_run_id
                             AND kind = 'commit' AND state = 'succeeded'
                             AND op_key GLOB ('commit:' || iteration || '@g[0-9]*')
                             AND substr(op_key, length('commit:' || iteration || '@g') + 1)
                                 NOT GLOB '*[^0-9]*'
             ), 0)
             )",
            [],
        )?;
        let legacy_runs: Vec<(String, String, i64, String, String, i64)> = {
            let mut statement = tx.prepare(
                "SELECT goal_run_id, plan_id, revision, spec_json, binding_json, created_at_unix_ms
                 FROM wc_goal_runs",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
                })?;
            rows.collect::<Result<_, _>>()?
        };
        for (run_id, plan_id, plan_revision, spec_json, binding_json, created_at) in legacy_runs {
            let spec: PlanSpec = serde_json::from_str(&spec_json)?;
            let acceptance_json = serde_json::to_string(&spec.acceptance)?;
            let workspace_requirements_json = serde_json::to_string(&spec.workspace_requirements)?;
            let dependencies_json = serde_json::to_string(&spec.dependencies)?;
            let acceptance_sha256 = if spec.workspace_requirements.is_empty() && spec.dependencies.is_empty() {
                // Legacy PlanSpec did not carry these fields, so preserve its
                // original empty-workspace revision digest on migration.
                legacy_acceptance_sha256(&spec.acceptance)?
            } else {
                goal_acceptance_spec_sha256(
                    &spec.acceptance,
                    &spec.workspace_requirements,
                    &spec.dependencies,
                )?
            };
            tx.execute(
                "INSERT OR IGNORE INTO wc_goal_acceptance_revisions (
                    goal_run_id, revision, plan_id, plan_revision, checkpoint_sha,
                    source_revision, source_acceptance_sha256, acceptance_sha256,
                    acceptance_json, workspace_requirements_json, dependencies_json,
                    binding_json, approver, approved_at_unix_ms, proposal_id
                 ) VALUES (?1, 1, ?2, ?3, NULL, NULL, ?4, ?4, ?5, ?6, ?7, ?8,
                           'original_plan_approval', ?9, NULL)",
                params![
                    run_id,
                    plan_id,
                    plan_revision,
                    acceptance_sha256,
                    acceptance_json,
                    workspace_requirements_json,
                    dependencies_json,
                    binding_json,
                    created_at
                ],
            )?;
        }
        let key_exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM wc_goal_acceptance_receipt_mac_key WHERE id = 1)",
            [],
            |row| row.get(0),
        )?;
        if !key_exists {
            // UUID v4 is backed by the operating system CSPRNG. The key is
            // never returned by Store APIs and is immutable after creation.
            let mut secret = Vec::with_capacity(64);
            secret.extend_from_slice(Uuid::new_v4().as_bytes());
            secret.extend_from_slice(Uuid::new_v4().as_bytes());
            secret.extend_from_slice(Uuid::new_v4().as_bytes());
            secret.extend_from_slice(Uuid::new_v4().as_bytes());
            tx.execute(
                "INSERT INTO wc_goal_acceptance_receipt_mac_key (id, secret, created_at_unix_ms)
                 VALUES (1, ?1, ?2)",
                params![secret, crate::communication::now_unix_ms()],
            )?;
        }
        tx.commit()?;
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
        let revision = load_revision(&tx, plan_id, plan.latest_revision)?.ok_or_else(|| {
            PlanStoreError::new("plan_revision_not_found", "Latest plan revision is missing")
        })?;
        // Run identity is the immutable plan revision, not the one-shot
        // approval_id. A repeated approve-and-start can issue another approval
        // for the same revision, so serialize this lookup with consumption and
        // creation in the same immediate transaction.
        let existing_run_id: Option<String> = tx
            .query_row(
                "SELECT goal_run_id FROM wc_goal_runs
                 WHERE owner_key = ?1 AND plan_id = ?2 AND revision = ?3 AND content_sha256 = ?4
                 ORDER BY created_at_unix_ms, goal_run_id LIMIT 1",
                params![owner_key, plan_id, plan.latest_revision, revision.content_sha256],
                |row| row.get(0),
            )
            .optional()
            .map_err(plan_store_error)?;
        if let Some(run_id) = existing_run_id {
            let run = load_run(&tx, &run_id)?;
            if run.binding.project_id != plan.project_id
                || run.binding.runner_client_id != environment.runner_client_id
                || run.binding.project_root != environment.project_root
            {
                return Err(PlanStoreError::new(
                    "plan_approval_environment_changed",
                    "The project's Runner or root changed since the Goal was started",
                ));
            }
            tx.commit().map_err(plan_store_error)?;
            return Ok(GoalRunStartOutcome {
                run,
                created: false,
            });
        }
        // Compatibility replay: a consumed approval whose run row is missing
        // still fails closed instead of making a second run.
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
        if binding.base_commit == UNBORN_BASE_COMMIT {
            return Err(PlanStoreError::new(
                "plan_base_commit_unavailable",
                "This plan was approved without a Git commit. A Goal starts from a commit: commit the project, then approve again.",
            ));
        }
        if kill_switch_engaged(&tx)? {
            return Err(PlanStoreError::new(
                "goal_emergency_stop_engaged",
                "Goal emergency stop is engaged in the Chadex app",
            ));
        }
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
        // Each project shows only its newest run: earlier finished runs leave
        // the lists (records and result branches are kept).
        dismiss_finished_runs(&tx, Some(&plan.project_id), now)?;
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
        let acceptance_hash = goal_acceptance_spec_sha256(
            &revision.spec.acceptance,
            &revision.spec.workspace_requirements,
            &revision.spec.dependencies,
        )?;
        tx.execute(
            "INSERT INTO wc_goal_acceptance_revisions (
                goal_run_id, revision, plan_id, plan_revision, checkpoint_sha,
                source_revision, source_acceptance_sha256, acceptance_sha256,
                acceptance_json, workspace_requirements_json, dependencies_json,
                binding_json, approver, approved_at_unix_ms, proposal_id
             ) VALUES (?1, 1, ?2, ?3, NULL, NULL, ?4, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL)",
            params![
                run_id,
                plan_id,
                decision.revision,
                acceptance_hash,
                serde_json::to_string(&revision.spec.acceptance).expect("acceptance serializes"),
                serde_json::to_string(&revision.spec.workspace_requirements)
                    .expect("workspace requirements serialize"),
                serde_json::to_string(&revision.spec.dependencies).expect("dependencies serialize"),
                serde_json::to_string(&binding).expect("binding serializes"),
                decision.approver,
                now,
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
                       -- A ChatGPT-mode run is driven only when it has work:
                       -- its worktree, a finished ChatGPT turn, a control
                       -- request, or a deadline/idle limit to enforce.
                       AND (provider <> 'chatgpt'
                            OR state = 'queued'
                            OR workspace_path IS NULL
                            OR chatgpt_ready_iteration > iteration
                            OR state_reason = 'acceptance_retry'
                            OR control_request IS NOT NULL
                            OR deadline_at_unix_ms <= ?1
                            OR COALESCE(chatgpt_seen_at_unix_ms, created_at_unix_ms) <= ?1 - ?2)
                     ORDER BY created_at_unix_ms",
                )
                .map_err(plan_store_error)?;
            let rows = statement
                .query_map(params![now, CHATGPT_GOAL_IDLE_MS], |row| row.get::<_, String>(0))
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

    /// Give the lease back without changing state, so a waiting ChatGPT-mode
    /// run is claimed again as soon as it has work.
    pub fn goal_run_release_lease(&self, run_id: &str, lease: &GoalLease) -> Result<(), PlanStoreError> {
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        conn.execute(
            "UPDATE wc_goal_runs SET lease_owner = NULL, lease_expires_at_unix_ms = NULL
             WHERE goal_run_id = ?1 AND lease_owner = ?2 AND lease_generation = ?3",
            params![run_id, lease.owner, lease.generation],
        )
        .map_err(plan_store_error)?;
        Ok(())
    }

    /// ChatGPT mode: the owner reports that its edits for the next iteration
    /// are done. Only the approved acceptance checks run afterwards; nothing
    /// the model sends is executed. Idempotent while that check is pending,
    /// a no-op before the worktree exists, and it resumes a run that was
    /// paused only because ChatGPT went idle.
    pub fn goal_run_chatgpt_ready(
        &self,
        owner_key: &str,
        run_id: &str,
        now: i64,
    ) -> Result<GoalRunRecord, PlanStoreError> {
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        let run = load_run(&tx, run_id)?;
        if run.owner_key != owner_key {
            return Err(PlanStoreError::new("goal_run_not_found", "Goal run not found"));
        }
        if run.provider != CHATGPT_GOAL_PROVIDER {
            return Err(PlanStoreError::new(
                "goal_run_not_chatgpt_mode",
                "This Goal runs in Codex mode inside Chadex; follow it with goal_run_status",
            ));
        }
        if run.state.terminal() {
            tx.commit().map_err(plan_store_error)?;
            return Ok(run);
        }
        let idle_paused = run.state == GoalRunState::Paused
            && run.state_reason.as_deref().is_some_and(|reason| reason.starts_with("chatgpt_idle"));
        if !matches!(run.state, GoalRunState::Queued | GoalRunState::Running) && !idle_paused {
            return Err(PlanStoreError::new(
                "goal_run_not_waiting",
                "This Goal is paused or needs attention; resume it in the Chadex app first",
            ));
        }
        if kill_switch_engaged(&tx)? || run.control_request.is_some() {
            return Err(PlanStoreError::new(
                "goal_run_stopping",
                "This Goal is being paused, cancelled or emergency-stopped; do not continue",
            ));
        }
        // A new round starts only once ChatGPT has seen the previous round's
        // result; otherwise this call just reports where the run is.
        let ready = if run.workspace_path.is_some()
            && run.chatgpt_ready_iteration <= run.iteration
            && run.chatgpt_reported_iteration >= run.iteration
        {
            run.iteration + 1
        } else {
            run.chatgpt_ready_iteration
        };
        tx.execute(
            "UPDATE wc_goal_runs SET chatgpt_ready_iteration = ?2, chatgpt_seen_at_unix_ms = ?3,
                 state = CASE WHEN state = 'paused' THEN 'queued' ELSE state END,
                 state_reason = CASE WHEN state = 'paused' THEN NULL ELSE state_reason END,
                 updated_at_unix_ms = ?3
             WHERE goal_run_id = ?1",
            params![run_id, ready, now],
        )
        .map_err(plan_store_error)?;
        if ready != run.chatgpt_ready_iteration || idle_paused {
            append_event(
                &tx,
                run_id,
                now,
                "chatgpt_ready",
                &serde_json::json!({"iteration": ready, "resumed": idle_paused}),
            )?;
        }
        let run = load_run(&tx, run_id)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(run)
    }

    /// Record that goal_check showed ChatGPT the result of `iteration`.
    pub fn goal_run_chatgpt_reported(
        &self,
        owner_key: &str,
        run_id: &str,
        iteration: i64,
    ) -> Result<(), PlanStoreError> {
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        conn.execute(
            "UPDATE wc_goal_runs SET chatgpt_reported_iteration = MAX(chatgpt_reported_iteration, ?3)
             WHERE goal_run_id = ?1 AND owner_key = ?2 AND iteration >= ?3",
            params![run_id, owner_key, iteration],
        )
        .map_err(plan_store_error)?;
        Ok(())
    }

    /// Record owner-scoped factual activity after the caller observed a
    /// successful file write or actual job start. This is intentionally not a
    /// public heartbeat and cannot advance the ChatGPT iteration handshake.
    pub fn goal_run_note_activity(
        &self,
        owner: &str,
        workspace_project_id: &str,
        now: i64,
    ) -> Result<usize, PlanStoreError> {
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        if kill_switch_engaged(&tx)? {
            tx.commit().map_err(plan_store_error)?;
            return Ok(0);
        }
        let run_ids: Vec<String> = {
            let mut statement = tx
                .prepare(
                    "SELECT goal_run_id FROM wc_goal_runs
                     WHERE owner_key = ?1 AND provider = 'chatgpt'
                       AND workspace_project_id = ?2 AND control_request IS NULL
                       AND COALESCE(chatgpt_seen_at_unix_ms, created_at_unix_ms) < ?3
                       AND (state IN ('queued', 'running')
                            OR (state = 'paused' AND state_reason LIKE 'chatgpt_idle%'))",
                )
                .map_err(plan_store_error)?;
            let rows = statement
                .query_map(params![owner, workspace_project_id, now], |row| row.get(0))
                .map_err(plan_store_error)?;
            let collected = rows
                .collect::<Result<_, _>>()
                .map_err(plan_store_error)?;
            collected
        };
        for run_id in &run_ids {
            tx.execute(
                "UPDATE wc_goal_runs SET chatgpt_seen_at_unix_ms = ?2,
                     state = CASE WHEN state = 'paused' THEN 'queued' ELSE state END,
                     state_reason = CASE WHEN state = 'paused' THEN NULL ELSE state_reason END,
                     updated_at_unix_ms = ?2
                 WHERE goal_run_id = ?1 AND owner_key = ?3 AND workspace_project_id = ?4
                   AND control_request IS NULL",
                params![run_id, now, owner, workspace_project_id],
            )
            .map_err(plan_store_error)?;
            append_event(
                &tx,
                run_id,
                now,
                "owner_activity",
                &serde_json::json!({"workspace_project_id": workspace_project_id}),
            )?;
        }
        tx.commit().map_err(plan_store_error)?;
        Ok(run_ids.len())
    }

    /// Supervisor-only refresh after an independently confirmed registered
    /// running job. Lease, generation, expiry and control state are fenced.
    pub fn goal_run_verified_activity(
        &self,
        run_id: &str,
        lease: &GoalLease,
        now: i64,
    ) -> Result<usize, PlanStoreError> {
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        if kill_switch_engaged(&tx)? {
            tx.commit().map_err(plan_store_error)?;
            return Ok(0);
        }
        let changed = tx
            .execute(
                "UPDATE wc_goal_runs SET chatgpt_seen_at_unix_ms = ?4, updated_at_unix_ms = ?4
                 WHERE goal_run_id = ?1 AND lease_owner = ?2 AND lease_generation = ?3
                   AND lease_expires_at_unix_ms > ?4 AND state = 'running'
                   AND provider = 'chatgpt' AND control_request IS NULL
                   AND COALESCE(chatgpt_seen_at_unix_ms, created_at_unix_ms) < ?4",
                params![run_id, lease.owner, lease.generation, now],
            )
            .map_err(plan_store_error)?;
        if changed == 1 {
            append_event(
                &tx,
                run_id,
                now,
                "verified_job_activity",
                &serde_json::json!({"source": "registered_running_job"}),
            )?;
        }
        tx.commit().map_err(plan_store_error)?;
        Ok(changed)
    }

    /// The run an approval of this plan's latest revision already started,
    /// if any. Never consumes an approval: only the Chadex app starts Goals.
    pub fn goal_run_for_plan(
        &self,
        owner_key: &str,
        plan_id: &str,
        now: i64,
    ) -> Result<Option<GoalRunRecord>, PlanStoreError> {
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        let plan = load_owned_plan(&tx, PlanReader::Owner(owner_key), plan_id, now)?;
        let run_id: Option<String> = tx
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
        let run = run_id.map(|id| load_run(&tx, &id)).transpose()?;
        tx.commit().map_err(plan_store_error)?;
        Ok(run)
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
        fence(&tx, run_id, lease)?;
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
        let run_before = load_run(&tx, run_id)?;
        if state == GoalRunState::Succeeded {
            ensure_goal_acceptance_passed(&tx, &run_before)?;
        }
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
        if state == GoalRunState::Succeeded {
            // The plan is done: close it so the app lists only open work. A
            // newer revision saved meanwhile is new work and stays open.
            tx.execute(
                "UPDATE wc_plans SET state = 'closed', closed_reason = ?3, lock_active = 0,
                     lock_expires_at_unix_ms = NULL, updated_at_unix_ms = ?4
                 WHERE plan_id = ?1 AND latest_revision = ?2 AND state != 'closed'",
                params![
                    run.plan_id,
                    run.revision,
                    format!("Goal {run_id} succeeded"),
                    now
                ],
            )
            .map_err(plan_store_error)?;
        }
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
                     state_reason = CASE
                         WHEN state_reason = 'acceptance_retry' AND EXISTS (
                             SELECT 1 FROM wc_goal_acceptance_rounds r
                             WHERE r.goal_run_id = wc_goal_runs.goal_run_id
                               AND r.acceptance_revision = wc_goal_runs.acceptance_revision
                               AND r.checkpoint_round = wc_goal_runs.committed_rounds
                               AND r.checkpoint_sha = wc_goal_runs.checkpoint_sha
                               AND r.attempt = wc_goal_runs.current_attempt
                         ) THEN NULL ELSE state_reason END,
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

    /// Record a Runner-verified Git HEAD for a successful commit journal entry.
    /// Legacy migrations may recover the committed round, but this method is
    /// the only leased path that fills its checkpoint OID.
    pub fn goal_run_checkpoint_record(
        &self,
        run_id: &str,
        lease: &GoalLease,
        committed_round: i64,
        runner_verified_head_oid: &str,
        now: i64,
    ) -> Result<GoalRunRecord, PlanStoreError> {
        let checkpoint = runner_verified_head_oid.to_ascii_lowercase();
        if committed_round < 1 || !is_git_oid(&checkpoint) {
            return Err(PlanStoreError::new(
                "goal_checkpoint_invalid",
                "A checkpoint requires a positive committed round and a Runner-verified Git object ID",
            ));
        }
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        fence(&tx, run_id, lease)?;
        let run = load_run(&tx, run_id)?;
        if run.lease_expires_at_unix_ms.is_none_or(|expires| expires <= now) {
            return Err(lease_lost());
        }
        let journal_commit: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM wc_goal_run_ops
                 WHERE goal_run_id = ?1 AND kind = 'commit' AND iteration = ?2 AND state = 'succeeded'
                   AND op_key GLOB ('commit:' || ?2 || '@g[0-9]*')
                   AND substr(op_key, length('commit:' || ?2 || '@g') + 1)
                       NOT GLOB '*[^0-9]*')",
                params![run_id, committed_round],
                |row| row.get(0),
            )
            .map_err(plan_store_error)?;
        if !journal_commit {
            return Err(PlanStoreError::new(
                "goal_checkpoint_commit_missing",
                "The operation journal does not contain a successful commit for this round",
            ));
        }
        let is_legacy_sha_fill = committed_round == run.committed_rounds && run.checkpoint_sha.is_none();
        let is_next_round = committed_round == run.committed_rounds + 1;
        if !is_legacy_sha_fill && !is_next_round {
            if committed_round == run.committed_rounds
                && run.checkpoint_sha.as_deref() == Some(checkpoint.as_str())
            {
                tx.commit().map_err(plan_store_error)?;
                return Ok(run);
            }
            return Err(PlanStoreError::new(
                "goal_checkpoint_conflict",
                "Checkpoint round is out of order or conflicts with the recorded HEAD",
            ));
        }
        let attempt = if is_next_round {
            1
        } else {
            run.current_attempt.max(1)
        };
        let changed = tx
            .execute(
                "UPDATE wc_goal_runs SET checkpoint_sha = ?4,
                     committed_rounds = MAX(committed_rounds, ?5),
                     current_attempt = ?6,
                     updated_at_unix_ms = ?7
                 WHERE goal_run_id = ?1 AND lease_owner = ?2 AND lease_generation = ?3
                   AND state = 'running' AND lease_expires_at_unix_ms > ?7",
                params![run_id, lease.owner, lease.generation, checkpoint, committed_round, attempt, now],
            )
            .map_err(plan_store_error)?;
        if changed != 1 {
            return Err(lease_lost());
        }
        append_event(
            &tx,
            run_id,
            now,
            "checkpoint_recorded",
            &serde_json::json!({"round": committed_round, "checkpoint_sha": checkpoint}),
        )?;
        let run = load_run(&tx, run_id)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(run)
    }

    /// Explicit Operator recovery for a legacy successful commit whose old
    /// run row has no trusted Git OID. The caller is the trusted Main Runner
    /// after it has verified HEAD and the exact commit subject.
    #[allow(clippy::too_many_arguments)]
    pub fn goal_run_checkpoint_adopt(
        &self,
        reader: PlanReader<'_>,
        run_id: &str,
        committed_round: i64,
        commit_op_key: &str,
        runner_verified_head_oid: &str,
        verified_commit_subject: &str,
        binding: &PlanApprovalBinding,
        approver: &str,
        now: i64,
    ) -> Result<GoalRunRecord, PlanStoreError> {
        if reader != PlanReader::Operator {
            return Err(PlanStoreError::new(
                "goal_checkpoint_adopt_operator_required",
                "Only the Chadex Operator can adopt a legacy checkpoint",
            ));
        }
        let checkpoint = runner_verified_head_oid.to_ascii_lowercase();
        let subject = verified_commit_subject.trim();
        if committed_round < 1
            || !is_git_oid(&checkpoint)
            || subject.is_empty()
            || subject.len() > 512
            || subject
                .bytes()
                .any(|byte| matches!(byte, b'\0' | b'\r' | b'\n'))
            || approver.trim().is_empty()
            || approver.len() > 200
        {
            return Err(PlanStoreError::new(
                "goal_checkpoint_adoption_invalid",
                "Adoption requires a Git object ID, one verified commit subject, and an Operator identity",
            ));
        }
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        let run = load_run(&tx, run_id)?;
        require_parked_unleased(&run)?;
        if binding != &run.binding {
            return Err(PlanStoreError::new(
                "goal_acceptance_binding_changed",
                "Checkpoint adoption must match the run's approved project, Runner, root, and base commit",
            ));
        }
        let expected_commit_key = format!("commit:{committed_round}@g");
        let generation = commit_op_key.strip_prefix(&expected_commit_key).unwrap_or("");
        if generation.is_empty() || !generation.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(PlanStoreError::new(
                "goal_checkpoint_commit_missing",
                "The operation key must identify this exact commit round and lease generation",
            ));
        }
        if committed_round <= run.iteration || committed_round != run.committed_rounds {
            return Err(PlanStoreError::new(
                "goal_checkpoint_legacy_not_adoptable",
                "Legacy adoption requires the committed round to be newer than iteration and equal committed_rounds",
            ));
        }
        let binding_json = serde_json::to_string(binding).expect("binding serializes");
        if let Some(existing) = run.checkpoint_sha.as_deref() {
            let adopted: Option<(String, String, String, String)> = tx
                .query_row(
                    "SELECT commit_op_key, checkpoint_sha, commit_subject, binding_json
                     FROM wc_goal_checkpoint_adoptions
                     WHERE goal_run_id = ?1 AND checkpoint_round = ?2",
                    params![run_id, committed_round],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()
                .map_err(plan_store_error)?;
            if existing == checkpoint
                && adopted.is_some_and(|record| {
                    record.0 == commit_op_key
                        && record.1 == checkpoint
                        && record.2 == subject
                        && record.3 == binding_json
                })
            {
                tx.commit().map_err(plan_store_error)?;
                return Ok(run);
            }
            return Err(PlanStoreError::new(
                "goal_checkpoint_conflict",
                "The legacy checkpoint is already recorded or conflicts with the adoption evidence",
            ));
        }
        let journal_commit: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM wc_goal_run_ops
                 WHERE goal_run_id = ?1 AND op_key = ?2 AND kind = 'commit'
                   AND iteration = ?3 AND state = 'succeeded'
                   AND op_key GLOB ('commit:' || ?3 || '@g[0-9]*')
                   AND substr(op_key, length('commit:' || ?3 || '@g') + 1)
                       NOT GLOB '*[^0-9]*')",
                params![run_id, commit_op_key, committed_round],
                |row| row.get(0),
            )
            .map_err(plan_store_error)?;
        if !journal_commit {
            return Err(PlanStoreError::new(
                "goal_checkpoint_commit_missing",
                "No successful commit journal entry matches the requested round and operation key",
            ));
        }
        if kill_switch_engaged(&tx)? {
            return Err(PlanStoreError::new(
                "goal_emergency_stop_engaged",
                "Goal emergency stop is engaged in the Chadex app",
            ));
        }
        tx.execute(
            "INSERT INTO wc_goal_checkpoint_adoptions (
                goal_run_id, checkpoint_round, commit_op_key, checkpoint_sha,
                commit_subject, binding_json, approver, adopted_at_unix_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                run_id,
                committed_round,
                commit_op_key,
                checkpoint,
                subject,
                binding_json,
                approver.trim(),
                now,
            ],
        )
        .map_err(plan_store_error)?;
        let changed = tx
            .execute(
                "UPDATE wc_goal_runs SET checkpoint_sha = ?2, current_attempt = 0,
                     updated_at_unix_ms = ?3
                 WHERE goal_run_id = ?1 AND checkpoint_sha IS NULL
                   AND committed_rounds = ?4 AND iteration < ?4
                   AND lease_owner IS NULL AND lease_expires_at_unix_ms IS NULL
                   AND control_request IS NULL AND state IN ('paused', 'needs_attention')",
                params![run_id, checkpoint, now, committed_round],
            )
            .map_err(plan_store_error)?;
        if changed != 1 {
            return Err(PlanStoreError::new(
                "goal_checkpoint_adoption_race",
                "The Goal changed while checkpoint adoption was being applied; refresh and inspect the journal",
            ));
        }
        append_event(
            &tx,
            run_id,
            now,
            "checkpoint_adopted",
            &serde_json::json!({
                "round": committed_round,
                "commit_op_key": commit_op_key,
                "checkpoint_sha": checkpoint,
                "commit_subject": subject,
                "approver": approver.trim(),
            }),
        )?;
        let run = load_run(&tx, run_id)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(run)
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

    pub fn goal_workspace_mutation_locked(&self,runner:&str,root:&str,now:i64)->Result<bool,PlanStoreError>{
        let conn=self.lock_connection(crate::StoreDomain::Goal);
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM wc_goal_runs r WHERE r.workspace_path=?1
                AND json_extract(r.binding_json,'$.runner_client_id')=?2
                AND (r.state='queued' OR (r.state='running' AND r.lease_expires_at_unix_ms > ?3)
                    OR EXISTS(SELECT 1 FROM wc_goal_run_events e WHERE e.goal_run_id=r.goal_run_id AND e.kind='container_owned'
                        AND NOT EXISTS(SELECT 1 FROM wc_goal_run_events c WHERE c.goal_run_id=e.goal_run_id AND c.kind='container_cleaned'
                            AND json_extract(c.detail_json,'$.id')=json_extract(e.detail_json,'$.id')
                            AND json_extract(c.detail_json,'$.created')=json_extract(e.detail_json,'$.created')))))",
            params![root,runner,now],|row|row.get(0)).map_err(plan_store_error)
    }

    /// Trusted container ownership survives event pagination and terminal runs.
    /// Never expose this control-plane material through a model-facing tool.
    pub fn goal_container_recovery_candidates(&self,now:i64)->Result<Vec<(GoalRunRecord,Value)>,PlanStoreError>{
        let conn=self.lock_connection(crate::StoreDomain::Goal);
        let mut statement=conn.prepare(
            "SELECT e.goal_run_id,e.detail_json FROM wc_goal_run_events e
             JOIN wc_goal_runs r ON r.goal_run_id=e.goal_run_id
             WHERE e.kind='container_owned' AND (r.lease_expires_at_unix_ms IS NULL OR r.lease_expires_at_unix_ms <= ?1)
             AND NOT EXISTS(SELECT 1 FROM wc_goal_run_events c WHERE c.goal_run_id=e.goal_run_id AND c.kind='container_cleaned'
                 AND json_extract(c.detail_json,'$.id')=json_extract(e.detail_json,'$.id')
                 AND json_extract(c.detail_json,'$.created')=json_extract(e.detail_json,'$.created'))
             ORDER BY e.seq LIMIT 32").map_err(plan_store_error)?;
        let entries=statement.query_map(params![now],|row|Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))
            .map_err(plan_store_error)?.collect::<Result<Vec<_>,_>>().map_err(plan_store_error)?;
        entries.into_iter().map(|(id,raw)|Ok((load_run(&conn,&id)?,serde_json::from_str(&raw)
            .map_err(|_|PlanStoreError::new("goal_container_journal_invalid","Container ownership journal is invalid"))?))).collect()
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

    /// Persist one observation made by the trusted Runner. Outcomes are
    /// derived here; receipt JSON supplied by an agent is never accepted.
    pub fn goal_acceptance_receipt_record(
        &self,
        run_id: &str,
        lease: &GoalLease,
        observation: GoalAcceptanceReceiptObservation,
    ) -> Result<GoalAcceptanceReceiptRecord, PlanStoreError> {
        if !is_git_oid(&observation.checkpoint_sha)
            || !is_sha256_hex(&observation.argv_sha256)
            || !is_sha256_hex(&observation.output_sha256)
        {
            return Err(PlanStoreError::new(
                "goal_receipt_invalid",
                "Receipt checkpoint must be a Git object ID and argv/output must be SHA-256 digests",
            ));
        }
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        fence(&tx, run_id, lease)?;
        let run = load_run(&tx, run_id)?;
        let lease_expires = run.lease_expires_at_unix_ms.unwrap_or_default();
        if observation.observed_at_unix_ms <= 0 || observation.observed_at_unix_ms >= lease_expires {
            return Err(lease_lost());
        }
        if run.acceptance_revision != observation.acceptance_revision
            || run.committed_rounds != observation.checkpoint_round
            || run.checkpoint_sha.as_deref() != Some(observation.checkpoint_sha.as_str())
        {
            return Err(PlanStoreError::new(
                "goal_receipt_scope_stale",
                "Receipt does not match the active acceptance revision, checkpoint, and attempt",
            ));
        }
        let attempt = run.current_attempt;
        if attempt < 1 {
            return Err(PlanStoreError::new(
                "goal_receipt_attempt_missing",
                "No acceptance attempt is active for this checkpoint",
            ));
        }
        let Some(check) = run.spec.acceptance.iter().find(|check| check.name == observation.check_name) else {
            return Err(PlanStoreError::new(
                "goal_receipt_check_unknown",
                "Receipt check name is not in the active acceptance revision",
            ));
        };
        if goal_acceptance_argv_sha256(&check.argv)? != observation.argv_sha256 {
            return Err(PlanStoreError::new(
                "goal_receipt_argv_mismatch",
                "Observed argv digest does not match the approved check",
            ));
        }
        let outcome = if check.kind == AcceptanceKind::ExternalGate
            && !observation.timed_out
            && (observation.exit_code == Some(2) || (observation.exit_code == Some(0)
                && !crate::goal_attestation::configured_authorization(&run,check,&observation)))
        {
            GoalAcceptanceOutcome::BlockedExternal
        } else if observation.timed_out {
            GoalAcceptanceOutcome::Failed
        } else {
            match observation.exit_code {
                Some(0) => GoalAcceptanceOutcome::Passed,
                Some(_) => GoalAcceptanceOutcome::Failed,
                None => GoalAcceptanceOutcome::Unknown,
            }
        };
        let receipt_id = allocate_identity(
            &tx,
            GOAL_ACCEPTANCE_RECEIPT_ID_PREFIX,
            "SELECT EXISTS(SELECT 1 FROM wc_goal_acceptance_receipts WHERE receipt_id = ?1)",
        )
        .map_err(|error| PlanStoreError::new("plan_store_unavailable", error.message().to_string()))?;
        let mut receipt = GoalAcceptanceReceiptRecord {
            receipt_id,
            goal_run_id: run_id.to_string(),
            acceptance_revision: observation.acceptance_revision,
            checkpoint_round: observation.checkpoint_round,
            checkpoint_sha: observation.checkpoint_sha,
            attempt,
            check_name: observation.check_name,
            argv_sha256: observation.argv_sha256,
            outcome,
            exit_code: observation.exit_code,
            timed_out: observation.timed_out,
            output_sha256: observation.output_sha256,
            observed_at_unix_ms: observation.observed_at_unix_ms,
            proof_hmac_sha256: String::new(),
        };
        let key: Vec<u8> = tx
            .query_row(
                "SELECT secret FROM wc_goal_acceptance_receipt_mac_key WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(plan_store_error)?
            .ok_or_else(|| PlanStoreError::new("goal_receipt_key_missing", "Acceptance receipt key is missing"))?;
        let payload = serde_json::to_vec(&receipt_mac_fields(&receipt)).expect("receipt MAC fields serialize");
        receipt.proof_hmac_sha256 = hex_encode(&hmac_sha256(&key, &payload));
        tx.execute(
            "INSERT INTO wc_goal_acceptance_receipts (
                receipt_id, goal_run_id, acceptance_revision, checkpoint_round,
                checkpoint_sha, attempt, check_name, argv_sha256, outcome,
                exit_code, timed_out, output_sha256, observed_at_unix_ms, proof_hmac_sha256
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                receipt.receipt_id,
                receipt.goal_run_id,
                receipt.acceptance_revision,
                receipt.checkpoint_round,
                receipt.checkpoint_sha,
                receipt.attempt,
                receipt.check_name,
                receipt.argv_sha256,
                receipt.outcome.as_str(),
                receipt.exit_code,
                receipt.timed_out as i64,
                receipt.output_sha256,
                receipt.observed_at_unix_ms,
                receipt.proof_hmac_sha256,
            ],
        )
        .map_err(plan_store_error)?;
        if acceptance_attempt_complete(&tx, &run, attempt, &key)? {
            let inserted = tx
                .execute(
                    "INSERT OR IGNORE INTO wc_goal_acceptance_rounds (
                        goal_run_id, acceptance_revision, checkpoint_round,
                        checkpoint_sha, attempt, completed_at_unix_ms
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![
                        run_id,
                        run.acceptance_revision,
                        run.committed_rounds,
                        run.checkpoint_sha,
                        attempt,
                        observation.observed_at_unix_ms,
                    ],
                )
                .map_err(plan_store_error)?;
            if inserted == 1 {
                tx.execute(
                    "UPDATE wc_goal_runs SET completed_acceptance_rounds = completed_acceptance_rounds + 1,
                         updated_at_unix_ms = MAX(updated_at_unix_ms, ?2)
                     WHERE goal_run_id = ?1 AND lease_owner = ?3 AND lease_generation = ?4",
                    params![run_id, observation.observed_at_unix_ms, lease.owner, lease.generation],
                )
                .map_err(plan_store_error)?;
            }
        }
        append_event(
            &tx,
            run_id,
            observation.observed_at_unix_ms,
            "acceptance_receipt",
            &serde_json::json!({
                "receipt_id": receipt.receipt_id,
                "revision": receipt.acceptance_revision,
                "round": receipt.checkpoint_round,
                "attempt": receipt.attempt,
                "check": receipt.check_name,
                "outcome": receipt.outcome.as_str(),
                "proof_hmac_sha256": receipt.proof_hmac_sha256,
            }),
        )?;
        tx.commit().map_err(plan_store_error)?;
        Ok(receipt)
    }

    /// Owner or Runtime-authorized Operator proposal. It is immutable and has
    /// no effect until an Operator approves the unchanged parked run/binding.
    #[allow(clippy::too_many_arguments)]
    pub fn goal_acceptance_revision_propose(
        &self,
        reader: PlanReader<'_>,
        run_id: &str,
        binding: &PlanApprovalBinding,
        expected_acceptance_revision: i64,
        expected_checkpoint_sha: &str,
        proposed_acceptance_sha256: &str,
        acceptance: Vec<AcceptanceCheck>,
        workspace_requirements: Vec<WorkspaceRequirement>,
        dependencies: Vec<WorkspaceDependency>,
        now: i64,
    ) -> Result<GoalAcceptanceRevisionProposal, PlanStoreError> {
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        let proposer = match reader {
            PlanReader::Owner(_) => GoalAcceptanceProposer::Owner,
            PlanReader::Operator => GoalAcceptanceProposer::Operator,
        };
        let run = load_visible_run(&tx, reader, run_id)?;
        require_parked_unleased(&run)?;
        if binding != &run.binding {
            return Err(PlanStoreError::new(
                "goal_acceptance_binding_changed",
                "Proposal must match the run's approved project, Runner, root, and base commit",
            ));
        }
        let Some(checkpoint) = run.checkpoint_sha.as_deref() else {
            return Err(PlanStoreError::new(
                "goal_checkpoint_unverified",
                "A Runner-verified checkpoint is required before proposing acceptance changes",
            ));
        };
        if expected_acceptance_revision != run.acceptance_revision
            || expected_checkpoint_sha != checkpoint
        {
            return Err(PlanStoreError::new(
                "goal_acceptance_revision_stale",
                "The acceptance revision or checkpoint changed; refresh the Goal before proposing",
            ));
        }
        let mut candidate = run.spec.clone();
        candidate.acceptance = acceptance;
        candidate.workspace_requirements = workspace_requirements;
        candidate.dependencies = dependencies;
        let candidate = candidate.normalized()?;
        let computed_hash = goal_acceptance_spec_sha256(
            &candidate.acceptance,
            &candidate.workspace_requirements,
            &candidate.dependencies,
        )?;
        if !constant_time_ascii_eq(&computed_hash, proposed_acceptance_sha256) {
            return Err(PlanStoreError::new(
                "goal_acceptance_hash_mismatch",
                "The proposed acceptance hash does not match the normalized acceptance checks",
            ));
        }
        ensure_no_required_gate_removed(&run.spec, &candidate)?;
        let source_hash = goal_acceptance_spec_sha256(
            &run.spec.acceptance,
            &run.spec.workspace_requirements,
            &run.spec.dependencies,
        )?;
        let proposal_id = allocate_identity(
            &tx,
            GOAL_ACCEPTANCE_PROPOSAL_ID_PREFIX,
            "SELECT EXISTS(SELECT 1 FROM wc_goal_acceptance_proposals WHERE proposal_id = ?1)",
        )
        .map_err(|error| PlanStoreError::new("plan_store_unavailable", error.message().to_string()))?;
        let proposer_role = match proposer {
            GoalAcceptanceProposer::Owner => "owner",
            GoalAcceptanceProposer::Operator => "operator",
        };
        let binding_json = serde_json::to_string(binding).expect("binding serializes");
        tx.execute(
            "INSERT INTO wc_goal_acceptance_proposals (
                proposal_id, goal_run_id, source_acceptance_revision, checkpoint_sha,
                source_acceptance_sha256, proposed_acceptance_sha256, acceptance_json,
                workspace_requirements_json, dependencies_json, proposer_owner_key,
                proposer_role, binding_json, proposed_at_unix_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                proposal_id,
                run_id,
                run.acceptance_revision,
                checkpoint,
                source_hash,
                computed_hash,
                serde_json::to_string(&candidate.acceptance).expect("acceptance serializes"),
                serde_json::to_string(&candidate.workspace_requirements)
                    .expect("workspace requirements serialize"),
                serde_json::to_string(&candidate.dependencies).expect("dependencies serialize"),
                run.owner_key,
                proposer_role,
                binding_json,
                now,
            ],
        )
        .map_err(plan_store_error)?;
        append_event(
            &tx,
            run_id,
            now,
            "acceptance_proposed",
            &serde_json::json!({
                "proposal_id": proposal_id,
                "source_revision": run.acceptance_revision,
                "checkpoint_sha": checkpoint,
                "acceptance_sha256": computed_hash,
                "proposer_role": proposer_role,
            }),
        )?;
        tx.commit().map_err(plan_store_error)?;
        Ok(GoalAcceptanceRevisionProposal {
            proposal_id,
            goal_run_id: run_id.to_string(),
            source_acceptance_revision: run.acceptance_revision,
            checkpoint_sha: checkpoint.to_string(),
            source_acceptance_sha256: source_hash,
            proposed_acceptance_sha256: computed_hash,
            acceptance: candidate.acceptance,
            workspace_requirements: candidate.workspace_requirements,
            dependencies: candidate.dependencies,
            binding: binding.clone(),
            proposer,
            proposed_at_unix_ms: now,
            proposer_owner_key: run.owner_key,
        })
    }

    /// Operator approval appends a new Goal acceptance/workspace snapshot.
    /// The immutable original Plan revision and content hash are untouched.
    #[allow(clippy::too_many_arguments)]
    pub fn goal_acceptance_revision_approve(
        &self,
        reader: PlanReader<'_>,
        run_id: &str,
        proposal_id: &str,
        expected_acceptance_revision: i64,
        expected_checkpoint_sha: &str,
        expected_proposed_acceptance_sha256: &str,
        binding: &PlanApprovalBinding,
        approver: &str,
        now: i64,
    ) -> Result<GoalAcceptanceRevisionRecord, PlanStoreError> {
        if reader != PlanReader::Operator {
            return Err(PlanStoreError::new(
                "goal_acceptance_approval_operator_required",
                "Only the Chadex Operator can approve Goal acceptance changes",
            ));
        }
        if approver.trim().is_empty() || approver.len() > 200 {
            return Err(PlanStoreError::new(
                "goal_acceptance_approver_invalid",
                "An operator identity is required to approve acceptance changes",
            ));
        }
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        let run = load_run(&tx, run_id)?;
        require_parked_unleased(&run)?;
        let Some(checkpoint) = run.checkpoint_sha.as_deref() else {
            return Err(PlanStoreError::new(
                "goal_checkpoint_unverified",
                "A Runner-verified checkpoint is required before approval",
            ));
        };
        if expected_acceptance_revision != run.acceptance_revision
            || expected_checkpoint_sha != checkpoint
        {
            return Err(PlanStoreError::new(
                "goal_acceptance_revision_stale",
                "The acceptance revision or checkpoint changed; review the proposal again",
            ));
        }
        if binding != &run.binding {
            return Err(PlanStoreError::new(
                "goal_acceptance_binding_changed",
                "Approval must match the run's approved project, Runner, root, and base commit",
            ));
        }
        if kill_switch_engaged(&tx)? {
            return Err(PlanStoreError::new(
                "goal_emergency_stop_engaged",
                "Goal emergency stop is engaged in the Chadex app",
            ));
        }
        let proposal: Option<(i64, String, String, String, String, String, String, String)> = tx
            .query_row(
                "SELECT source_acceptance_revision, checkpoint_sha,
                        source_acceptance_sha256, proposed_acceptance_sha256,
                        acceptance_json, workspace_requirements_json, dependencies_json, binding_json
                 FROM wc_goal_acceptance_proposals
                 WHERE proposal_id = ?1 AND goal_run_id = ?2 AND proposer_owner_key = ?3
                   AND checkpoint_sha IS NOT NULL",
                params![proposal_id, run_id, run.owner_key],
                |row| Ok((
                    row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?,
                    row.get(4)?, row.get(5)?, row.get(6)?, row.get(7)?,
                )),
            )
            .optional()
            .map_err(plan_store_error)?;
        let Some((source_revision, proposal_checkpoint, source_hash, proposed_hash, acceptance_json, requirements_json, dependencies_json, proposal_binding_json)) = proposal else {
            return Err(PlanStoreError::new("goal_acceptance_proposal_not_found", "Acceptance proposal not found"));
        };
        if source_revision != run.acceptance_revision || proposal_checkpoint != checkpoint {
            return Err(PlanStoreError::new(
                "goal_acceptance_proposal_stale",
                "The proposal was made against a different acceptance revision or checkpoint",
            ));
        }
        if !constant_time_ascii_eq(&proposed_hash, expected_proposed_acceptance_sha256) {
            return Err(PlanStoreError::new(
                "goal_acceptance_proposal_hash_mismatch",
                "The proposal hash shown for approval differs from the stored proposal",
            ));
        }
        if proposal_binding_json != serde_json::to_string(binding).expect("binding serializes") {
            return Err(PlanStoreError::new(
                "goal_acceptance_binding_changed",
                "The proposal was submitted for a different project, Runner, root, or base commit",
            ));
        }
        if source_hash
            != goal_acceptance_spec_sha256(
                &run.spec.acceptance,
                &run.spec.workspace_requirements,
                &run.spec.dependencies,
            )?
        {
            return Err(PlanStoreError::new(
                "goal_acceptance_proposal_corrupt",
                "The proposal source acceptance hash does not match the current revision",
            ));
        }
        let acceptance: Vec<AcceptanceCheck> = serde_json::from_str(&acceptance_json)
            .map_err(|_| PlanStoreError::new("goal_acceptance_proposal_corrupt", "Proposal acceptance JSON is invalid"))?;
        let workspace_requirements: Vec<WorkspaceRequirement> = serde_json::from_str(&requirements_json)
            .map_err(|_| PlanStoreError::new("goal_acceptance_proposal_corrupt", "Proposal workspace requirements are invalid"))?;
        let dependencies: Vec<WorkspaceDependency> = serde_json::from_str(&dependencies_json)
            .map_err(|_| PlanStoreError::new("goal_acceptance_proposal_corrupt", "Proposal dependencies are invalid"))?;
        let mut candidate = run.spec.clone();
        candidate.acceptance = acceptance;
        candidate.workspace_requirements = workspace_requirements;
        candidate.dependencies = dependencies;
        let candidate = candidate.normalized()?;
        let computed_proposal_hash = goal_acceptance_spec_sha256(
            &candidate.acceptance,
            &candidate.workspace_requirements,
            &candidate.dependencies,
        )?;
        if computed_proposal_hash != proposed_hash {
            return Err(PlanStoreError::new(
                "goal_acceptance_proposal_corrupt",
                "Stored proposal acceptance hash does not match its content",
            ));
        }
        ensure_no_required_gate_removed(&run.spec, &candidate)?;
        let next_revision = run.acceptance_revision + 1;
        let normalized_binding = serde_json::to_string(binding).expect("binding serializes");
        tx.execute(
            "INSERT INTO wc_goal_acceptance_revisions (
                goal_run_id, revision, plan_id, plan_revision, checkpoint_sha,
                source_revision, source_acceptance_sha256, acceptance_sha256,
                acceptance_json, workspace_requirements_json, dependencies_json,
                binding_json, approver, approved_at_unix_ms, proposal_id
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                run_id,
                next_revision,
                run.plan_id,
                run.revision,
                checkpoint,
                run.acceptance_revision,
                source_hash,
                proposed_hash,
                serde_json::to_string(&candidate.acceptance).expect("acceptance serializes"),
                serde_json::to_string(&candidate.workspace_requirements)
                    .expect("workspace requirements serialize"),
                serde_json::to_string(&candidate.dependencies).expect("dependencies serialize"),
                normalized_binding,
                approver.trim(),
                now,
                proposal_id,
            ],
        )
        .map_err(plan_store_error)?;
        let changed = tx
            .execute(
                "UPDATE wc_goal_runs SET acceptance_revision = ?2, current_attempt = 0,
                     updated_at_unix_ms = ?3
                 WHERE goal_run_id = ?1 AND acceptance_revision = ?4
                   AND checkpoint_sha = ?5 AND lease_owner IS NULL
                   AND control_request IS NULL AND state IN ('paused', 'needs_attention')",
                params![run_id, next_revision, now, run.acceptance_revision, checkpoint],
            )
            .map_err(plan_store_error)?;
        if changed != 1 {
            return Err(PlanStoreError::new(
                "goal_acceptance_approval_race",
                "The Goal changed while approval was being applied; refresh and try again",
            ));
        }
        append_event(
            &tx,
            run_id,
            now,
            "acceptance_approved",
            &serde_json::json!({
                "proposal_id": proposal_id,
                "revision": next_revision,
                "acceptance_sha256": proposed_hash,
                "workspace_requirements": candidate.workspace_requirements.len(),
                "dependencies": candidate.dependencies.len(),
                "approver": approver.trim(),
            }),
        )?;
        tx.commit().map_err(plan_store_error)?;
        Ok(GoalAcceptanceRevisionRecord {
            goal_run_id: run_id.to_string(),
            revision: next_revision,
            checkpoint_sha: Some(checkpoint.to_string()),
            source_revision: Some(run.acceptance_revision),
            source_acceptance_sha256: source_hash,
            acceptance_sha256: proposed_hash,
            acceptance: candidate.acceptance,
            workspace_requirements: candidate.workspace_requirements,
            dependencies: candidate.dependencies,
            binding: binding.clone(),
            approver: approver.trim().to_string(),
            approved_at_unix_ms: now,
            proposal_id: Some(proposal_id.to_string()),
        })
    }

    /// Read the persisted receipts for an owner or the local Operator. Every
    /// MAC is checked before a record is returned.
    pub fn goal_acceptance_receipts(
        &self,
        reader: PlanReader<'_>,
        run_id: &str,
    ) -> Result<Vec<GoalAcceptanceReceiptRecord>, PlanStoreError> {
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        load_visible_run(&conn, reader, run_id)?;
        let key: Vec<u8> = conn
            .query_row(
                "SELECT secret FROM wc_goal_acceptance_receipt_mac_key WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(plan_store_error)?
            .ok_or_else(|| PlanStoreError::new("goal_receipt_key_missing", "Acceptance receipt key is missing"))?;
        let mut statement = conn
            .prepare(
                "SELECT receipt_id, goal_run_id, acceptance_revision, checkpoint_round,
                        checkpoint_sha, attempt, check_name, argv_sha256, outcome,
                        exit_code, timed_out, output_sha256, observed_at_unix_ms, proof_hmac_sha256
                 FROM wc_goal_acceptance_receipts WHERE goal_run_id = ?1
                 ORDER BY observed_at_unix_ms, receipt_id",
            )
            .map_err(plan_store_error)?;
        let rows = statement
            .query_map(params![run_id], |row| {
                Ok(GoalAcceptanceReceiptRecord {
                    receipt_id: row.get(0)?,
                    goal_run_id: row.get(1)?,
                    acceptance_revision: row.get(2)?,
                    checkpoint_round: row.get(3)?,
                    checkpoint_sha: row.get(4)?,
                    attempt: row.get(5)?,
                    check_name: row.get(6)?,
                    argv_sha256: row.get(7)?,
                    outcome: GoalAcceptanceOutcome::from_db(&row.get::<_, String>(8)?)?,
                    exit_code: row.get(9)?,
                    timed_out: row.get::<_, i64>(10)? != 0,
                    output_sha256: row.get(11)?,
                    observed_at_unix_ms: row.get(12)?,
                    proof_hmac_sha256: row.get(13)?,
                })
            })
            .map_err(plan_store_error)?;
        let mut receipts = Vec::new();
        for row in rows {
            let receipt = row.map_err(plan_store_error)?;
            if !verify_receipt_mac(&key, &receipt)? {
                return Err(PlanStoreError::new(
                    "goal_receipt_integrity_failed",
                    "An acceptance receipt failed its Store integrity check",
                ));
            }
            receipts.push(receipt);
        }
        Ok(receipts)
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
                // A ChatGPT-mode run waiting for its next turn holds no lease
                // and has nothing in flight: apply directly, like a queued run.
                let idle_chatgpt = run.provider == CHATGPT_GOAL_PROVIDER && run.lease_owner.is_none();
                if run.state == GoalRunState::Running && !idle_chatgpt {
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
                request.to_string()
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
                         control_request = NULL, updated_at_unix_ms = ?2,
                         chatgpt_seen_at_unix_ms = ?2
                     WHERE goal_run_id = ?1",
                    params![run_id, now],
                )
                .map_err(plan_store_error)?;
                "resume".to_string()
            }
            GoalControlAction::RetryAcceptance => {
                let pending:i64=tx.query_row(
                    "SELECT COUNT(*) FROM wc_goal_run_events e WHERE e.goal_run_id=?1 AND e.kind='container_owned'
                     AND NOT EXISTS(SELECT 1 FROM wc_goal_run_events c WHERE c.goal_run_id=e.goal_run_id AND c.kind='container_cleaned'
                     AND json_extract(c.detail_json,'$.id')=json_extract(e.detail_json,'$.id')
                     AND json_extract(c.detail_json,'$.created')=json_extract(e.detail_json,'$.created'))",
                    params![run_id],|row|row.get(0)).map_err(plan_store_error)?;
                if pending>0{return Err(PlanStoreError::new("goal_container_cleanup_pending","Reconcile the exact owned container before retrying acceptance"))}

                if reader != PlanReader::Operator {
                    return Err(PlanStoreError::new(
                        "goal_acceptance_retry_operator_required",
                        "Only the Chadex Operator can retry acceptance at a checkpoint",
                    ));
                }
                require_parked_unleased(&run)?;
                if !matches!(run.state, GoalRunState::Paused | GoalRunState::NeedsAttention) {
                    return Err(PlanStoreError::new(
                        "goal_acceptance_retry_not_parked",
                        "Acceptance can be retried only after the Goal is parked",
                    ));
                }
                if run.checkpoint_sha.is_none() || run.committed_rounds < 1 {
                    return Err(PlanStoreError::new(
                        "goal_checkpoint_unverified",
                        "A Runner-verified commit checkpoint is required before retrying acceptance",
                    ));
                }
                if run.spec.acceptance.is_empty() {
                    return Err(PlanStoreError::new(
                        "goal_acceptance_empty",
                        "This Goal has no acceptance checks to retry",
                    ));
                }
                if kill_switch_engaged(&tx)? {
                    return Err(PlanStoreError::new(
                        "goal_emergency_stop_engaged",
                        "Goal emergency stop is engaged in the Chadex app",
                    ));
                }
                let latest_commit_succeeded: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM wc_goal_run_ops WHERE goal_run_id = ?1
                         AND kind = 'commit' AND iteration = ?2 AND state = 'succeeded'
                         AND op_key GLOB ('commit:' || ?2 || '@g[0-9]*')
                         AND substr(op_key, length('commit:' || ?2 || '@g') + 1)
                             NOT GLOB '*[^0-9]*')",
                        params![run_id, run.committed_rounds],
                        |row| row.get(0),
                    )
                    .map_err(plan_store_error)?;
                if !latest_commit_succeeded {
                    return Err(PlanStoreError::new(
                        "goal_checkpoint_commit_missing",
                        "The checkpoint has no successful commit journal entry",
                    ));
                }
                let unresolved_agent_operation: bool = tx
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM wc_goal_run_ops WHERE goal_run_id = ?1
                         AND kind IN ('agent', 'stage', 'commit')
                         AND state IN ('intent', 'dispatched', 'unknown'))",
                        params![run_id],
                        |row| row.get(0),
                    )
                    .map_err(plan_store_error)?;
                if unresolved_agent_operation {
                    return Err(PlanStoreError::new(
                        "goal_agent_outcome_unknown",
                        "An agent or commit operation has an unresolved outcome; reconcile it before retrying acceptance",
                    ));
                }
                let attempt = run.current_attempt.saturating_add(1).max(1);
                let changed = tx
                    .execute(
                        "UPDATE wc_goal_runs SET state = 'queued', state_reason = 'acceptance_retry',
                             current_attempt = ?2, control_request = NULL,
                             updated_at_unix_ms = ?3
                         WHERE goal_run_id = ?1 AND acceptance_revision = ?4
                           AND checkpoint_sha = ?5 AND lease_owner IS NULL
                           AND control_request IS NULL AND state IN ('paused', 'needs_attention')",
                        params![run_id, attempt, now, run.acceptance_revision, run.checkpoint_sha],
                    )
                    .map_err(plan_store_error)?;
                if changed != 1 {
                    return Err(PlanStoreError::new(
                        "goal_acceptance_retry_race",
                        "The Goal changed while retry was being queued; refresh and retry",
                    ));
                }
                format!("retry_acceptance:{attempt}")
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
                        "SELECT goal_run_id,
                                CASE WHEN state = 'running' AND provider = 'chatgpt' AND lease_owner IS NULL
                                     THEN 'idle' ELSE state END
                         FROM wc_goal_runs
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

    /// Test support: pretend ChatGPT was last seen at `at`.
    #[doc(hidden)]
    pub fn goal_run_override_chatgpt_seen(&self, run_id: &str, at: i64) -> Result<(), PlanStoreError> {
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        conn.execute(
            "UPDATE wc_goal_runs SET chatgpt_seen_at_unix_ms = ?2 WHERE goal_run_id = ?1",
            params![run_id, at],
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
        let acceptance_revisions = load_acceptance_revisions(&conn, run_id)?;
        let pending_proposals = load_pending_acceptance_proposals(&conn, &run, &acceptance_revisions)?;
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
        let acceptance_receipts=load_scoped_receipts(&conn,&run,run.current_attempt)?;
        if !acceptance_receipts.is_empty() {
            let key:Vec<u8>=conn.query_row("SELECT secret FROM wc_goal_acceptance_receipt_mac_key WHERE id=1",[],|row|row.get(0)).map_err(plan_store_error)?;
            for receipt in &acceptance_receipts {
                if !verify_receipt_mac(&key,receipt)? {return Err(PlanStoreError::new("goal_receipt_integrity_failed","Acceptance receipt failed its integrity check"))}
            }
        }
        Ok(GoalRunDetail {
            run,
            ops,
            events,
            acceptance_revisions,
            acceptance_receipts,
            pending_proposals,
            kill_switch_engaged: kill_switch_engaged(&conn)?,
        })
    }

    /// Operator clears the Plan & Goal board (one project, or all) when the
    /// ChatGPT connection ends or the app starts again: open plans and Goal
    /// requests are closed and finished runs leave the lists. Runs that have
    /// not finished keep going. Nothing is deleted.
    pub fn plan_goal_clear_board(
        &self,
        project_id: Option<&str>,
        now: i64,
    ) -> Result<(usize, usize), PlanStoreError> {
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        let plans = super::plan::close_open_plans(&tx, project_id, None, "cleared: connection ended", now)?;
        let runs = dismiss_finished_runs(&tx, project_id, now)?;
        tx.commit().map_err(plan_store_error)?;
        Ok((plans, runs))
    }

    /// Operator removes a finished run from the run lists. The run, its
    /// events and its result branch are kept; only listing hides it.
    pub fn goal_run_dismiss(&self, run_id: &str, now: i64) -> Result<GoalRunRecord, PlanStoreError> {
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        let run = load_run(&tx, run_id)?;
        if !run.state.terminal() {
            return Err(PlanStoreError::new(
                "goal_run_not_finished",
                "Only a finished Goal run can be removed; cancel it first",
            ));
        }
        tx.execute(
            "UPDATE wc_goal_runs SET dismissed_at_unix_ms = COALESCE(dismissed_at_unix_ms, ?2)
             WHERE goal_run_id = ?1",
            params![run_id, now],
        )
        .map_err(plan_store_error)?;
        append_event(&tx, run_id, now, "dismissed", &serde_json::json!({}))?;
        tx.commit().map_err(plan_store_error)?;
        Ok(run)
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
                       -- Dismissal only tidies the app's lists; the model can
                       -- still find a finished run to report its result.
                       AND (?1 IS NOT NULL OR dismissed_at_unix_ms IS NULL)
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

fn constant_time_ascii_eq(left: &str, right: &str) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.bytes()
        .zip(right.bytes())
        .fold(0_u8, |difference, (left, right)| difference | (left ^ right))
        == 0
}

fn require_parked_unleased(run: &GoalRunRecord) -> Result<(), PlanStoreError> {
    if !matches!(run.state, GoalRunState::Paused | GoalRunState::NeedsAttention)
        || run.lease_owner.is_some()
        || run.lease_expires_at_unix_ms.is_some()
        || run.control_request.is_some()
    {
        return Err(PlanStoreError::new(
            "goal_run_not_parked",
            "The Goal must be parked with no lease or pending control request",
        ));
    }
    Ok(())
}

fn ensure_no_required_gate_removed(old: &PlanSpec, new: &PlanSpec) -> Result<(), PlanStoreError> {
    for previous in &old.acceptance {
        let Some(replacement) = new.acceptance.iter().find(|check| check.name == previous.name) else {
            return Err(PlanStoreError::new(
                "goal_acceptance_gate_removed",
                format!("Required acceptance gate {} cannot be removed", previous.name),
            ));
        };
        if previous.kind == AcceptanceKind::ExternalGate
            && replacement.kind != AcceptanceKind::ExternalGate
        {
            return Err(PlanStoreError::new(
                "goal_acceptance_gate_weakened",
                format!("External gate {} cannot be changed into a runnable test", previous.name),
            ));
        }
        if previous.kind == AcceptanceKind::ExternalGate
            || replacement.kind == AcceptanceKind::ExternalGate
        {
            if previous.argv != replacement.argv || previous.cwd != replacement.cwd {
                return Err(PlanStoreError::new(
                    "goal_acceptance_gate_weakened",
                    format!(
                        "External gate {} must retain its approved argv and working directory",
                        previous.name
                    ),
                ));
            }
            if let Some(previous_manifest) = &previous.external_gate {
                let replacement_manifest = replacement.external_gate.as_ref().ok_or_else(|| {
                    PlanStoreError::new(
                        "goal_acceptance_gate_weakened",
                        format!("External gate {} cannot remove its approved manifest", previous.name),
                    )
                })?;
                if previous_manifest.owner != replacement_manifest.owner
                    || previous_manifest.action != replacement_manifest.action
                    || previous_manifest.evidence != replacement_manifest.evidence
                {
                    return Err(PlanStoreError::new(
                        "goal_acceptance_gate_weakened",
                        format!(
                            "External gate {} cannot change its approved owner, action, or evidence",
                            previous.name
                        ),
                    ));
                }
            }
        }
    }
    for previous in &old.workspace_requirements {
        if !new
            .workspace_requirements
            .iter()
            .any(|requirement| requirement.destination_path == previous.destination_path)
        {
            return Err(PlanStoreError::new(
                "goal_workspace_requirement_removed",
                format!("Required workspace input {} cannot be removed", previous.destination_path),
            ));
        }
    }
    for previous in &old.dependencies {
        if !new.dependencies.iter().any(|dependency| dependency.path == previous.path) {
            return Err(PlanStoreError::new(
                "goal_dependency_removed",
                format!("Required dependency {} cannot be removed", previous.path),
            ));
        }
    }
    Ok(())
}

fn verify_receipt_mac(key: &[u8], receipt: &GoalAcceptanceReceiptRecord) -> Result<bool, PlanStoreError> {
    let payload = serde_json::to_vec(&receipt_mac_fields(receipt))
        .map_err(|_| PlanStoreError::new("goal_receipt_integrity_failed", "Receipt could not be encoded"))?;
    Ok(constant_time_hex_eq(
        &hmac_sha256(key, &payload),
        &receipt.proof_hmac_sha256,
    ))
}

fn load_scoped_receipts(
    conn: &Connection,
    run: &GoalRunRecord,
    attempt: i64,
) -> Result<Vec<GoalAcceptanceReceiptRecord>, PlanStoreError> {
    let mut statement = conn
        .prepare(
            "SELECT receipt_id, goal_run_id, acceptance_revision, checkpoint_round,
                    checkpoint_sha, attempt, check_name, argv_sha256, outcome,
                    exit_code, timed_out, output_sha256, observed_at_unix_ms, proof_hmac_sha256
             FROM wc_goal_acceptance_receipts
             WHERE goal_run_id = ?1 AND acceptance_revision = ?2
               AND checkpoint_round = ?3 AND checkpoint_sha = ?4 AND attempt = ?5",
        )
        .map_err(plan_store_error)?;
    let rows = statement
        .query_map(
            params![
                run.goal_run_id,
                run.acceptance_revision,
                run.committed_rounds,
                run.checkpoint_sha,
                attempt,
            ],
            |row| {
                Ok(GoalAcceptanceReceiptRecord {
                    receipt_id: row.get(0)?,
                    goal_run_id: row.get(1)?,
                    acceptance_revision: row.get(2)?,
                    checkpoint_round: row.get(3)?,
                    checkpoint_sha: row.get(4)?,
                    attempt: row.get(5)?,
                    check_name: row.get(6)?,
                    argv_sha256: row.get(7)?,
                    outcome: GoalAcceptanceOutcome::from_db(&row.get::<_, String>(8)?)?,
                    exit_code: row.get(9)?,
                    timed_out: row.get::<_, i64>(10)? != 0,
                    output_sha256: row.get(11)?,
                    observed_at_unix_ms: row.get(12)?,
                    proof_hmac_sha256: row.get(13)?,
                })
            },
        )
        .map_err(plan_store_error)?;
    rows.collect::<Result<_, _>>().map_err(plan_store_error)
}

fn acceptance_attempt_complete(
    conn: &Connection,
    run: &GoalRunRecord,
    attempt: i64,
    key: &[u8],
) -> Result<bool, PlanStoreError> {
    if run.spec.acceptance.is_empty() || run.checkpoint_sha.is_none() || attempt < 1 {
        return Ok(false);
    }
    let receipts = load_scoped_receipts(conn, run, attempt)?;
    let mut outcomes = BTreeMap::new();
    for receipt in receipts {
        if !verify_receipt_mac(key, &receipt)? {
            return Err(PlanStoreError::new(
                "goal_receipt_integrity_failed",
                "An acceptance receipt failed its Store integrity check",
            ));
        }
        outcomes.insert(receipt.check_name, receipt.outcome);
    }
    Ok(run.spec.acceptance.iter().all(|check| {
        outcomes
            .get(&check.name)
            .is_some_and(|outcome| *outcome != GoalAcceptanceOutcome::Unknown)
    }))
}

fn ensure_goal_acceptance_passed(conn: &Connection, run: &GoalRunRecord) -> Result<(), PlanStoreError> {
    if run.checkpoint_sha.is_none() || run.committed_rounds < 1 {
        return Err(PlanStoreError::new(
            "goal_checkpoint_unverified",
            "A Runner-verified commit checkpoint is required before Goal success",
        ));
    }
    if run.spec.acceptance.is_empty() {
        return Ok(());
    }
    if run.current_attempt < 1 {
        return Err(PlanStoreError::new(
            "goal_acceptance_receipts_missing",
            "The active checkpoint has no completed acceptance attempt",
        ));
    }
    let key: Vec<u8> = conn
        .query_row(
            "SELECT secret FROM wc_goal_acceptance_receipt_mac_key WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(plan_store_error)?
        .ok_or_else(|| PlanStoreError::new("goal_receipt_key_missing", "Acceptance receipt key is missing"))?;
    let receipts = load_scoped_receipts(conn, run, run.current_attempt)?;
    let mut outcomes = BTreeMap::new();
    for receipt in receipts {
        if !verify_receipt_mac(&key, &receipt)? {
            return Err(PlanStoreError::new(
                "goal_receipt_integrity_failed",
                "An acceptance receipt failed its Store integrity check",
            ));
        }
        outcomes.insert(receipt.check_name, receipt.outcome);
    }
    if run.spec.acceptance.iter().all(|check| {
        outcomes.get(&check.name) == Some(&GoalAcceptanceOutcome::Passed)
    }) {
        Ok(())
    } else {
        Err(PlanStoreError::new(
            "goal_acceptance_not_passed",
            "Every required acceptance check needs a valid passed receipt at the current checkpoint and attempt",
        ))
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

fn load_acceptance_revisions(
    conn: &Connection,
    run_id: &str,
) -> Result<Vec<GoalAcceptanceRevisionRecord>, PlanStoreError> {
    let raw: Vec<(
        i64,
        Option<String>,
        Option<i64>,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
        Option<String>,
    )> = {
        let mut statement = conn
            .prepare(
                "SELECT revision, checkpoint_sha, source_revision, source_acceptance_sha256,
                        acceptance_sha256, acceptance_json, workspace_requirements_json,
                        dependencies_json, binding_json, approver, approved_at_unix_ms, proposal_id
                 FROM wc_goal_acceptance_revisions WHERE goal_run_id = ?1 ORDER BY revision",
            )
            .map_err(plan_store_error)?;
        let rows = statement
            .query_map(params![run_id], |row| {
                Ok((
                    row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?,
                    row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?, row.get(10)?, row.get(11)?,
                ))
            })
            .map_err(plan_store_error)?;
        rows.collect::<Result<_, _>>().map_err(plan_store_error)?
    };
    raw.into_iter()
        .map(|(
            revision,
            checkpoint_sha,
            source_revision,
            source_acceptance_sha256,
            acceptance_sha256,
            acceptance_json,
            requirements_json,
            dependencies_json,
            binding_json,
            approver,
            approved_at_unix_ms,
            proposal_id,
        )| {
            let acceptance: Vec<AcceptanceCheck> = serde_json::from_str(&acceptance_json)
                .map_err(|_| PlanStoreError::new("goal_acceptance_revision_corrupt", "Acceptance history JSON is invalid"))?;
            let workspace_requirements: Vec<WorkspaceRequirement> = serde_json::from_str(&requirements_json)
                .map_err(|_| PlanStoreError::new("goal_acceptance_revision_corrupt", "Workspace requirement history JSON is invalid"))?;
            let dependencies: Vec<WorkspaceDependency> = serde_json::from_str(&dependencies_json)
                .map_err(|_| PlanStoreError::new("goal_acceptance_revision_corrupt", "Dependency history JSON is invalid"))?;
            let full_hash = goal_acceptance_spec_sha256(&acceptance, &workspace_requirements, &dependencies)?;
            let legacy_empty_baseline = revision == 1
                && source_revision.is_none()
                && workspace_requirements.is_empty()
                && dependencies.is_empty()
                && legacy_acceptance_sha256(&acceptance)? == acceptance_sha256;
            if full_hash != acceptance_sha256 && !legacy_empty_baseline {
                return Err(PlanStoreError::new(
                    "goal_acceptance_revision_corrupt",
                    "Acceptance history hash does not match the complete acceptance policy",
                ));
            }
            let binding: PlanApprovalBinding = serde_json::from_str(&binding_json)
                .map_err(|_| PlanStoreError::new("goal_acceptance_revision_corrupt", "Acceptance binding history JSON is invalid"))?;
            Ok(GoalAcceptanceRevisionRecord {
                goal_run_id: run_id.to_string(),
                revision,
                checkpoint_sha,
                source_revision,
                source_acceptance_sha256,
                acceptance_sha256,
                acceptance,
                workspace_requirements,
                dependencies,
                binding,
                approver,
                approved_at_unix_ms,
                proposal_id,
            })
        })
        .collect()
}

fn load_pending_acceptance_proposals(
    conn: &Connection,
    run: &GoalRunRecord,
    revisions: &[GoalAcceptanceRevisionRecord],
) -> Result<Vec<GoalAcceptanceRevisionProposal>, PlanStoreError> {
    let raw: Vec<(
        String,
        String,
        i64,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        i64,
        String,
    )> = {
        let mut statement = conn
            .prepare(
                "SELECT p.proposal_id, p.goal_run_id, p.source_acceptance_revision,
                        p.checkpoint_sha, p.source_acceptance_sha256, p.proposed_acceptance_sha256,
                        p.acceptance_json, p.workspace_requirements_json, p.dependencies_json,
                        p.binding_json, p.proposer_role, p.proposed_at_unix_ms, p.proposer_owner_key
                 FROM wc_goal_acceptance_proposals p
                 LEFT JOIN wc_goal_acceptance_revisions r ON r.proposal_id = p.proposal_id
                 WHERE p.goal_run_id = ?1 AND r.proposal_id IS NULL
                   AND p.binding_json <> '{}'
                 ORDER BY p.proposed_at_unix_ms, p.proposal_id",
            )
            .map_err(plan_store_error)?;
        let rows = statement
            .query_map(params![run.goal_run_id], |row| {
                Ok((
                    row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?,
                    row.get(6)?, row.get(7)?, row.get(8)?, row.get(9)?, row.get(10)?, row.get(11)?,
                    row.get(12)?,
                ))
            })
            .map_err(plan_store_error)?;
        rows.collect::<Result<_, _>>().map_err(plan_store_error)?
    };
    raw.into_iter()
        .map(|(
            proposal_id,
            goal_run_id,
            source_acceptance_revision,
            checkpoint_sha,
            source_acceptance_sha256,
            proposed_acceptance_sha256,
            acceptance_json,
            requirements_json,
            dependencies_json,
            binding_json,
            proposer_role,
            proposed_at_unix_ms,
            proposer_owner_key,
        )| {
            let acceptance: Vec<AcceptanceCheck> = serde_json::from_str(&acceptance_json)
                .map_err(|_| PlanStoreError::new("goal_acceptance_proposal_corrupt", "Proposal acceptance JSON is invalid"))?;
            let workspace_requirements: Vec<WorkspaceRequirement> = serde_json::from_str(&requirements_json)
                .map_err(|_| PlanStoreError::new("goal_acceptance_proposal_corrupt", "Proposal workspace requirement JSON is invalid"))?;
            let dependencies: Vec<WorkspaceDependency> = serde_json::from_str(&dependencies_json)
                .map_err(|_| PlanStoreError::new("goal_acceptance_proposal_corrupt", "Proposal dependency JSON is invalid"))?;
            let binding: PlanApprovalBinding = serde_json::from_str(&binding_json)
                .map_err(|_| PlanStoreError::new("goal_acceptance_proposal_corrupt", "Proposal binding JSON is invalid"))?;
            if binding != run.binding {
                return Err(PlanStoreError::new(
                    "goal_acceptance_proposal_corrupt",
                    "Proposal binding differs from the approved Goal binding",
                ));
            }
            let source = revisions
                .iter()
                .find(|revision| revision.revision == source_acceptance_revision)
                .ok_or_else(|| PlanStoreError::new("goal_acceptance_proposal_corrupt", "Proposal source revision is missing"))?;
            let source_hash = goal_acceptance_spec_sha256(
                &source.acceptance,
                &source.workspace_requirements,
                &source.dependencies,
            )?;
            if source_hash != source_acceptance_sha256
                || goal_acceptance_spec_sha256(&acceptance, &workspace_requirements, &dependencies)?
                    != proposed_acceptance_sha256
            {
                return Err(PlanStoreError::new(
                    "goal_acceptance_proposal_corrupt",
                    "Proposal hashes do not match their complete acceptance policies",
                ));
            }
            let proposer = match proposer_role.as_str() {
                "owner" => GoalAcceptanceProposer::Owner,
                "operator" => GoalAcceptanceProposer::Operator,
                _ => return Err(PlanStoreError::new("goal_acceptance_proposal_corrupt", "Proposal actor is invalid")),
            };
            Ok(GoalAcceptanceRevisionProposal {
                proposal_id,
                goal_run_id,
                source_acceptance_revision,
                checkpoint_sha,
                source_acceptance_sha256,
                proposed_acceptance_sha256,
                acceptance,
                workspace_requirements,
                dependencies,
                binding,
                proposer,
                proposed_at_unix_ms,
                proposer_owner_key,
            })
        })
        .collect()
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

fn dismiss_finished_runs(
    conn: &Connection,
    project_id: Option<&str>,
    now: i64,
) -> Result<usize, PlanStoreError> {
    conn.execute(
        "UPDATE wc_goal_runs SET dismissed_at_unix_ms = ?2
         WHERE dismissed_at_unix_ms IS NULL
           AND state IN ('succeeded', 'failed', 'cancelled', 'stopped')
           AND (?1 IS NULL OR project_id = ?1)",
        params![project_id, now],
    )
    .map_err(plan_store_error)
}

pub(crate) fn load_run(conn: &Connection, run_id: &str) -> Result<GoalRunRecord, PlanStoreError> {
    let mut run = conn.query_row(
        "SELECT goal_run_id, plan_id, revision, approval_id, content_sha256, owner_key,
                project_id, binding_json, spec_json, dispatch_auth_json, provider, state,
                state_reason, control_request, iteration, checkpoint_sha,
                committed_rounds, completed_acceptance_rounds, current_attempt, acceptance_revision,
                same_failure_streak,
                last_failure_digest, workspace_project_id, workspace_path, workspace_branch,
                tokens_used, lease_owner, lease_generation, lease_expires_at_unix_ms,
                created_at_unix_ms, deadline_at_unix_ms, updated_at_unix_ms, finished_at_unix_ms,
                chatgpt_ready_iteration, chatgpt_seen_at_unix_ms, chatgpt_reported_iteration
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
                checkpoint_sha: row.get(15)?,
                committed_rounds: row.get(16)?,
                completed_acceptance_rounds: row.get(17)?,
                current_attempt: row.get(18)?,
                acceptance_revision: row.get(19)?,
                same_failure_streak: row.get(20)?,
                last_failure_digest: row.get(21)?,
                workspace_project_id: row.get(22)?,
                workspace_path: row.get(23)?,
                workspace_branch: row.get(24)?,
                tokens_used: row.get(25)?,
                lease_owner: row.get(26)?,
                lease_generation: row.get(27)?,
                lease_expires_at_unix_ms: row.get(28)?,
                created_at_unix_ms: row.get(29)?,
                deadline_at_unix_ms: row.get(30)?,
                updated_at_unix_ms: row.get(31)?,
                finished_at_unix_ms: row.get(32)?,
                chatgpt_ready_iteration: row.get(33)?,
                chatgpt_seen_at_unix_ms: row.get(34)?,
                chatgpt_reported_iteration: row.get(35)?,
            })
        },
    )
    .optional()
    .map_err(plan_store_error)?
    .ok_or_else(|| PlanStoreError::new("goal_run_not_found", "Goal run not found"))?;
    let revision: Option<(i64, Option<i64>, String, String, String, String)> = conn
        .query_row(
            "SELECT revision, source_revision, acceptance_sha256, acceptance_json,
                    workspace_requirements_json, dependencies_json
             FROM wc_goal_acceptance_revisions WHERE goal_run_id = ?1 AND revision = ?2",
            params![run_id, run.acceptance_revision],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )
        .optional()
        .map_err(plan_store_error)?;
    let Some((revision_number, source_revision, stored_hash, acceptance_json, requirements_json, dependencies_json)) = revision else {
        return Err(PlanStoreError::new(
            "goal_acceptance_revision_missing",
            "The active Goal acceptance revision is missing",
        ));
    };
    let acceptance: Vec<AcceptanceCheck> = serde_json::from_str(&acceptance_json)
        .map_err(|_| PlanStoreError::new("goal_acceptance_revision_corrupt", "Active acceptance JSON is invalid"))?;
    let workspace_requirements: Vec<WorkspaceRequirement> = serde_json::from_str(&requirements_json)
        .map_err(|_| PlanStoreError::new("goal_acceptance_revision_corrupt", "Active workspace requirements are invalid"))?;
    let dependencies: Vec<WorkspaceDependency> = serde_json::from_str(&dependencies_json)
        .map_err(|_| PlanStoreError::new("goal_acceptance_revision_corrupt", "Active dependencies are invalid"))?;
    let full_spec_hash = goal_acceptance_spec_sha256(&acceptance, &workspace_requirements, &dependencies)?;
    let is_legacy_empty_baseline = revision_number == 1
        && source_revision.is_none()
        && workspace_requirements.is_empty()
        && dependencies.is_empty()
        && legacy_acceptance_sha256(&acceptance)? == stored_hash;
    if full_spec_hash != stored_hash && !is_legacy_empty_baseline {
        return Err(PlanStoreError::new(
            "goal_acceptance_revision_corrupt",
            "Active acceptance hash does not match the complete acceptance policy",
        ));
    }
    run.spec.acceptance = acceptance;
    run.spec.workspace_requirements = workspace_requirements;
    run.spec.dependencies = dependencies;
    Ok(run)
}
