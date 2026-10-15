//! Durable Plan Mode control plane: plans, immutable revisions, review
//! decisions and the per-principal read-only lock.
//!
//! Plan text lives only in Chadex's own SQLite control plane. Revisions are
//! append-only (SQLite triggers reject UPDATE/DELETE), and an approval binds the
//! exact revision, its content hash, the project, the Runner/root it was
//! reviewed against, and an expiry. Saving a newer revision supersedes every
//! earlier approval in the same transaction.

use super::communication::{allocate_identity, now_unix_ms};
use super::Database;
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap};

pub const PLAN_ID_PREFIX: &str = "wc_plan_";
pub const PLAN_APPROVAL_ID_PREFIX: &str = "wc_papr_";
pub const MAX_PLAN_TITLE_CHARS: usize = 200;
pub const MAX_PLAN_CONTENT_BYTES: usize = 64 * 1024;
pub const MAX_PLAN_ACCEPTANCE_CHECKS: usize = 8;
pub const MAX_PLAN_CHECK_ARGS: usize = 32;
pub const MAX_PLAN_CHECK_ARG_BYTES: usize = 1024;
pub const MAX_PLAN_CHECK_TIMEOUT_SECS: u32 = 3600;
pub const MAX_GOAL_ITERATIONS: u32 = 20;
pub const MAX_GOAL_MINUTES: u32 = 24 * 60;
pub const MIN_GOAL_AGENT_TIMEOUT_SECS: u32 = 60;
pub const MAX_GOAL_AGENT_TIMEOUT_SECS: u32 = 3600;
pub const MAX_PLAN_LIST_LIMIT: usize = 50;
pub const DEFAULT_PLAN_LOCK_TTL_MS: i64 = 6 * 60 * 60 * 1000;
pub const DEFAULT_PLAN_APPROVAL_TTL_MS: i64 = 24 * 60 * 60 * 1000;
const MAX_PLAN_REASON_BYTES: usize = 2048;
const PLAN_HASH_DOMAIN: &str = "chadex.plan.revision.v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStoreError {
    code: &'static str,
    message: String,
}

impl PlanStoreError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn code(&self) -> &'static str {
        self.code
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

impl std::fmt::Display for PlanStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PlanStoreError {}

pub(crate) fn plan_store_error(error: rusqlite::Error) -> PlanStoreError {
    tracing::warn!(error = %error, "durable Plan store operation failed");
    PlanStoreError::new("plan_store_unavailable", "Durable Plan store is unavailable")
}

/// Sandbox capability required by one acceptance check.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceLane {
    #[default]
    OfflineSandbox,
    LoopbackIntegration,
    ExternalAttestation,
}

fn is_offline_lane(value: &AcceptanceLane) -> bool {
    *value == AcceptanceLane::OfflineSandbox
}

/// Whether a check is runnable code or an externally controlled gate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceKind {
    #[default]
    Test,
    ExternalGate,
}

fn is_test_kind(value: &AcceptanceKind) -> bool {
    *value == AcceptanceKind::Test
}

/// Human action and evidence required for an external gate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExternalGateManifest {
    pub owner: String,
    pub action: String,
    pub evidence: String,
    pub next_step: String,
}

/// A source artifact that may be prepared in a Goal workspace only with the
/// declared digest and read-only permission. The store never copies it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceRequirement {
    pub source_path: String,
    pub source_sha256: String,
    pub destination_path: String,
    pub permission: WorkspaceRequirementPermission,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceRequirementPermission {
    ReadOnly,
}

/// A declared dependency and the evidence describing where it came from.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceDependency {
    pub path: String,
    pub provenance: String,
}

/// One acceptance command, executed as literal argv (never a shell string).
/// Legacy specs deserialize with the offline sandbox and ordinary-test defaults.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AcceptanceCheck {
    pub name: String,
    pub argv: Vec<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default = "default_check_timeout_secs")]
    pub timeout_secs: u32,
    #[serde(default, skip_serializing_if = "is_offline_lane")]
    pub lane: AcceptanceLane,
    #[serde(default, skip_serializing_if = "is_test_kind")]
    pub kind: AcceptanceKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub loopback_ports: Vec<u16>,
    /// Immutable local Linux image approved with this acceptance revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_image: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_gate: Option<ExternalGateManifest>,
}

fn default_check_timeout_secs() -> u32 {
    600
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GoalBudget {
    #[serde(default = "default_max_iterations")]
    pub max_iterations: u32,
    #[serde(default = "default_max_minutes")]
    pub max_minutes: u32,
    #[serde(default = "default_agent_timeout_secs")]
    pub agent_timeout_secs: u32,
}

fn default_max_iterations() -> u32 {
    5
}

fn default_max_minutes() -> u32 {
    120
}

fn default_agent_timeout_secs() -> u32 {
    1800
}

impl Default for GoalBudget {
    fn default() -> Self {
        Self {
            max_iterations: default_max_iterations(),
            max_minutes: default_max_minutes(),
            agent_timeout_secs: default_agent_timeout_secs(),
        }
    }
}

/// Structured, hash-covered part of a plan revision.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlanSpec {
    #[serde(default)]
    pub acceptance: Vec<AcceptanceCheck>,
    #[serde(default)]
    pub budget: GoalBudget,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspace_requirements: Vec<WorkspaceRequirement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<WorkspaceDependency>,
}

impl PlanSpec {
    /// Validate and normalize. The normalized value is what gets hashed and
    /// stored, so equal intent always yields an equal hash.
    pub fn normalized(mut self) -> Result<Self, PlanStoreError> {
        if self.acceptance.len() > MAX_PLAN_ACCEPTANCE_CHECKS {
            return Err(PlanStoreError::new(
                "invalid_plan_acceptance",
                format!("at most {MAX_PLAN_ACCEPTANCE_CHECKS} acceptance checks are allowed"),
            ));
        }
        for check in &mut self.acceptance {
            check.name = check.name.trim().to_string();
            if check.name.is_empty() || check.name.chars().count() > 80 {
                return Err(PlanStoreError::new(
                    "invalid_plan_acceptance",
                    "acceptance check name must contain 1..=80 characters",
                ));
            }
            if check.argv.is_empty()
                || check.argv.len() > MAX_PLAN_CHECK_ARGS
                || check.argv[0].trim().is_empty()
                || check
                    .argv
                    .iter()
                    .any(|arg| arg.len() > MAX_PLAN_CHECK_ARG_BYTES || arg.contains('\0'))
            {
                return Err(PlanStoreError::new(
                    "invalid_plan_acceptance",
                    format!(
                        "acceptance argv must contain 1..={MAX_PLAN_CHECK_ARGS} non-NUL arguments of at most {MAX_PLAN_CHECK_ARG_BYTES} bytes"
                    ),
                ));
            }
            check.cwd = match check.cwd.take().map(|cwd| cwd.trim().to_string()) {
                None => None,
                Some(cwd) if cwd.is_empty() || cwd == "." => None,
                Some(cwd) => {
                    let path = std::path::Path::new(&cwd);
                    if path.is_absolute()
                        || path
                            .components()
                            .any(|part| matches!(part, std::path::Component::ParentDir))
                    {
                        return Err(PlanStoreError::new(
                            "invalid_plan_acceptance",
                            "acceptance cwd must be a project-relative path without '..'",
                        ));
                    }
                    Some(cwd)
                }
            };
            if check.timeout_secs == 0 || check.timeout_secs > MAX_PLAN_CHECK_TIMEOUT_SECS {
                return Err(PlanStoreError::new(
                    "invalid_plan_acceptance",
                    format!("acceptance timeout_secs must be within 1..={MAX_PLAN_CHECK_TIMEOUT_SECS}"),
                ));
            }
            if check.container_image.as_ref().is_some_and(|image|
                check.lane != AcceptanceLane::LoopbackIntegration ||
                !image.strip_prefix("sha256:").is_some_and(is_sha256_hex)) {
                return Err(PlanStoreError::new("invalid_plan_acceptance",
                    "container_image requires loopback_integration and an exact sha256 image ID"));
            }
            if check.loopback_ports.len() > 8
                || check.loopback_ports.iter().any(|port| *port < 49152)
                || check.loopback_ports.iter().copied().collect::<BTreeSet<_>>().len() != check.loopback_ports.len()
                || (check.lane != AcceptanceLane::LoopbackIntegration && !check.loopback_ports.is_empty())
                || (check.lane == AcceptanceLane::LoopbackIntegration && check.container_image.is_none() && check.loopback_ports.is_empty()) {
                return Err(PlanStoreError::new("invalid_plan_acceptance",
                    "loopback needs a pinned private container or distinct approved ports within 49152..=65535"));
            }
            match (&check.kind, &check.lane, &check.external_gate) {
                (AcceptanceKind::ExternalGate, AcceptanceLane::ExternalAttestation, Some(manifest)) => {
                    for (value, field) in [
                        (&manifest.owner, "owner"),
                        (&manifest.action, "action"),
                        (&manifest.evidence, "evidence"),
                        (&manifest.next_step, "next_step"),
                    ] {
                        if value.trim().is_empty() || value.len() > 2048 {
                            return Err(PlanStoreError::new(
                                "invalid_plan_acceptance",
                                format!("external gate {field} must contain 1..=2048 bytes"),
                            ));
                        }
                    }
                }
                (AcceptanceKind::ExternalGate, _, _) => {
                    return Err(PlanStoreError::new(
                        "invalid_plan_acceptance",
                        "external_gate checks require the external_attestation lane and a complete manifest",
                    ));
                }
                (_, AcceptanceLane::ExternalAttestation, _) | (_, _, Some(_)) => {
                    return Err(PlanStoreError::new(
                        "invalid_plan_acceptance",
                        "external_attestation lane and manifest are reserved for external_gate checks",
                    ));
                }
                _ => {}
            }
            let mut dependencies = BTreeSet::new();
            for dependency in &mut check.depends_on {
                *dependency = dependency.trim().to_string();
                if dependency.is_empty()
                    || dependency.as_str() == check.name.as_str()
                    || !dependencies.insert(dependency.clone())
                {
                    return Err(PlanStoreError::new(
                        "invalid_plan_acceptance",
                        "acceptance dependencies must be unique check names and cannot depend on themselves",
                    ));
                }
            }
        }
        let check_names: BTreeSet<_> = self.acceptance.iter().map(|check| check.name.as_str()).collect();
        let graph: HashMap<_, _> = self
            .acceptance
            .iter()
            .map(|check| (check.name.as_str(), check.depends_on.iter().map(String::as_str).collect::<Vec<_>>()))
            .collect();
        for check in &self.acceptance {
            if check.depends_on.iter().any(|name| !check_names.contains(name.as_str())) {
                return Err(PlanStoreError::new(
                    "invalid_plan_acceptance",
                    format!("acceptance check {} depends on an unknown check name", check.name),
                ));
            }
        }
        fn visit<'a>(
            name: &'a str,
            graph: &HashMap<&'a str, Vec<&'a str>>,
            visiting: &mut BTreeSet<&'a str>,
            visited: &mut BTreeSet<&'a str>,
        ) -> bool {
            if visited.contains(name) {
                return false;
            }
            if !visiting.insert(name) {
                return true;
            }
            if graph
                .get(name)
                .is_some_and(|dependencies| dependencies.iter().any(|dependency| visit(dependency, graph, visiting, visited)))
            {
                return true;
            }
            visiting.remove(name);
            visited.insert(name);
            false
        }
        let mut visiting = BTreeSet::new();
        let mut visited = BTreeSet::new();
        if graph.keys().any(|name| visit(name, &graph, &mut visiting, &mut visited)) {
            return Err(PlanStoreError::new(
                "invalid_plan_acceptance",
                "acceptance dependencies cannot contain a cycle",
            ));
        }
        let mut workspace_destinations = BTreeSet::new();
        for requirement in &mut self.workspace_requirements {
            requirement.source_path = normalize_workspace_path(&requirement.source_path, "source_path")?;
            requirement.destination_path = normalize_workspace_path(&requirement.destination_path, "destination_path")?;
            requirement.source_sha256 = requirement.source_sha256.to_ascii_lowercase();
            if !is_sha256_hex(&requirement.source_sha256) || !workspace_destinations.insert(requirement.destination_path.clone()) {
                return Err(PlanStoreError::new(
                    "invalid_workspace_requirement",
                    "workspace requirements need a SHA-256 digest and unique destinations",
                ));
            }
        }
        let mut dependency_paths = BTreeSet::new();
        for dependency in &mut self.dependencies {
            dependency.path = normalize_workspace_path(&dependency.path, "dependency path")?;
            dependency.provenance = dependency.provenance.trim().to_string();
            if dependency.provenance.is_empty()
                || dependency.provenance.len() > 2048
                || !dependency_paths.insert(dependency.path.clone())
            {
                return Err(PlanStoreError::new(
                    "invalid_workspace_dependency",
                    "workspace dependencies need a unique relative path and non-empty provenance",
                ));
            }
        }
        let budget = &self.budget;
        if budget.max_iterations == 0 || budget.max_iterations > MAX_GOAL_ITERATIONS {
            return Err(PlanStoreError::new(
                "invalid_plan_budget",
                format!("budget.max_iterations must be within 1..={MAX_GOAL_ITERATIONS}"),
            ));
        }
        if budget.max_minutes == 0 || budget.max_minutes > MAX_GOAL_MINUTES {
            return Err(PlanStoreError::new(
                "invalid_plan_budget",
                format!("budget.max_minutes must be within 1..={MAX_GOAL_MINUTES}"),
            ));
        }
        if !(MIN_GOAL_AGENT_TIMEOUT_SECS..=MAX_GOAL_AGENT_TIMEOUT_SECS)
            .contains(&budget.agent_timeout_secs)
        {
            return Err(PlanStoreError::new(
                "invalid_plan_budget",
                format!(
                    "budget.agent_timeout_secs must be within {MIN_GOAL_AGENT_TIMEOUT_SECS}..={MAX_GOAL_AGENT_TIMEOUT_SECS}"
                ),
            ));
        }
        Ok(self)
    }
}

fn normalize_workspace_path(value: &str, field: &str) -> Result<String, PlanStoreError> {
    let value = value.trim();
    if value.is_empty()
        || value.contains('\0')
        || value.contains('\\')
        || value.starts_with('/')
        || value.split('/').any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(PlanStoreError::new(
            "invalid_workspace_requirement",
            format!("{field} must be a normalized relative path without '.', '..', or platform separators"),
        ));
    }
    Ok(value.to_string())
}

pub(super) fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Validate the textual object ID emitted by Git. The repository object
/// format determines whether this is 40 hex characters (SHA-1) or 64
/// (SHA-256); this is deliberately separate from content SHA-256 validators.
pub fn is_git_oid(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn plan_content_sha256(content: &str, spec: &PlanSpec) -> String {
    let spec_json = serde_json::to_string(spec).expect("PlanSpec serializes");
    let mut hasher = Sha256::new();
    hasher.update(PLAN_HASH_DOMAIN.as_bytes());
    hasher.update(b"\0");
    hasher.update(content.as_bytes());
    hasher.update(b"\0");
    hasher.update(spec_json.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanState {
    Drafting,
    Approved,
    Closed,
}

impl PlanState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Drafting => "drafting",
            Self::Approved => "approved",
            Self::Closed => "closed",
        }
    }

    fn from_db(value: &str) -> rusqlite::Result<Self> {
        match value {
            "drafting" => Ok(Self::Drafting),
            "approved" => Ok(Self::Approved),
            "closed" => Ok(Self::Closed),
            other => Err(rusqlite::Error::FromSqlConversionFailure(
                0,
                rusqlite::types::Type::Text,
                format!("unsupported Plan state: {other}").into(),
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlanSummary {
    pub plan_id: String,
    pub project_id: String,
    pub title: String,
    pub state: PlanState,
    pub latest_revision: i64,
    /// `plan` for a Plan Mode plan, `goal` for a direct Goal request that the
    /// operator starts from the Chadex app.
    pub origin: String,
    /// True while the owning principal is restricted to read-only tools.
    pub plan_mode_active: bool,
    pub plan_mode_expires_at_unix_ms: Option<i64>,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
    pub closed_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlanStartContext {
    pub owner_key: String,
    pub dispatch: Option<serde_json::Value>,
    pub dispatch_recorded_at_unix_ms: Option<i64>,
    pub owner_locked_plan: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlanRevisionSummary {
    pub revision: i64,
    pub content_sha256: String,
    pub created_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlanRevisionRecord {
    pub revision: i64,
    pub content: String,
    pub spec: PlanSpec,
    pub content_sha256: String,
    pub created_at_unix_ms: i64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanDecisionState {
    Active,
    Rejected,
    Consumed,
    Revoked,
    Superseded,
    Expired,
}

impl PlanDecisionState {
    fn from_db(value: &str) -> rusqlite::Result<Self> {
        Ok(match value {
            "active" => Self::Active,
            "rejected" => Self::Rejected,
            "consumed" => Self::Consumed,
            "revoked" => Self::Revoked,
            "superseded" => Self::Superseded,
            "expired" => Self::Expired,
            other => {
                return Err(rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Text,
                    format!("unsupported Plan decision state: {other}").into(),
                ))
            }
        })
    }
}

/// Environment an approval was granted against. A Goal may only start when
/// the current environment still matches.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanApprovalBinding {
    pub project_id: String,
    pub runner_client_id: String,
    pub project_root: String,
    pub base_commit: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlanDecisionRecord {
    pub approval_id: String,
    pub plan_id: String,
    pub revision: i64,
    pub content_sha256: String,
    pub state: PlanDecisionState,
    pub binding: Option<PlanApprovalBinding>,
    pub approver: String,
    pub channel: String,
    pub reason: Option<String>,
    pub created_at_unix_ms: i64,
    pub expires_at_unix_ms: Option<i64>,
    pub consumed_by_goal_run_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlanDetail {
    pub plan: PlanSummary,
    pub revision: Option<PlanRevisionRecord>,
    pub revisions: Vec<PlanRevisionSummary>,
    pub decisions: Vec<PlanDecisionRecord>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlanStartOutcome {
    pub plan: PlanSummary,
    pub created: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PlanRevisionOutcome {
    pub plan: PlanSummary,
    pub revision: PlanRevisionSummary,
    pub superseded_approvals: usize,
}

/// One active read-only lock, keyed by the opaque principal key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanLockRecord {
    pub owner_key: String,
    pub plan_id: String,
    pub project_id: String,
    pub expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanReader<'a> {
    /// The principal that owns the plan (ChatGPT/MCP side).
    Owner(&'a str),
    /// Trusted local operator (Chadex App over the operator REST path).
    Operator,
}

impl Database {
    pub(super) fn ensure_plan_schema(conn: &mut Connection) -> anyhow::Result<()> {
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS wc_plans (
                plan_id TEXT PRIMARY KEY,
                owner_key TEXT NOT NULL,
                project_id TEXT NOT NULL,
                title TEXT NOT NULL,
                state TEXT NOT NULL CHECK(state IN ('drafting', 'approved', 'closed')),
                latest_revision INTEGER NOT NULL CHECK(latest_revision >= 0),
                lock_active INTEGER NOT NULL CHECK(lock_active IN (0, 1)),
                lock_expires_at_unix_ms INTEGER,
                created_at_unix_ms INTEGER NOT NULL,
                updated_at_unix_ms INTEGER NOT NULL,
                closed_reason TEXT,
                CHECK(lock_active = 0 OR lock_expires_at_unix_ms IS NOT NULL),
                CHECK(state <> 'closed' OR lock_active = 0)
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_wc_plans_one_lock_per_owner
                ON wc_plans(owner_key) WHERE lock_active = 1;
            CREATE INDEX IF NOT EXISTS idx_wc_plans_owner_updated
                ON wc_plans(owner_key, updated_at_unix_ms DESC);

            CREATE TABLE IF NOT EXISTS wc_plan_revisions (
                plan_id TEXT NOT NULL,
                revision INTEGER NOT NULL CHECK(revision >= 1),
                content TEXT NOT NULL,
                spec_json TEXT NOT NULL,
                content_sha256 TEXT NOT NULL,
                author_key TEXT NOT NULL,
                created_at_unix_ms INTEGER NOT NULL,
                PRIMARY KEY(plan_id, revision),
                FOREIGN KEY(plan_id) REFERENCES wc_plans(plan_id)
            );
            CREATE TRIGGER IF NOT EXISTS trg_wc_plan_revisions_immutable_update
                BEFORE UPDATE ON wc_plan_revisions
                BEGIN SELECT RAISE(ABORT, 'plan revisions are immutable'); END;
            CREATE TRIGGER IF NOT EXISTS trg_wc_plan_revisions_immutable_delete
                BEFORE DELETE ON wc_plan_revisions
                BEGIN SELECT RAISE(ABORT, 'plan revisions are immutable'); END;

            CREATE TABLE IF NOT EXISTS wc_plan_decisions (
                approval_id TEXT PRIMARY KEY,
                plan_id TEXT NOT NULL,
                revision INTEGER NOT NULL,
                content_sha256 TEXT NOT NULL,
                state TEXT NOT NULL CHECK(state IN (
                    'active', 'rejected', 'consumed', 'revoked', 'superseded', 'expired'
                )),
                binding_json TEXT,
                approver TEXT NOT NULL,
                channel TEXT NOT NULL,
                reason TEXT,
                created_at_unix_ms INTEGER NOT NULL,
                expires_at_unix_ms INTEGER,
                decided_at_unix_ms INTEGER NOT NULL,
                consumed_by_goal_run_id TEXT,
                FOREIGN KEY(plan_id, revision) REFERENCES wc_plan_revisions(plan_id, revision),
                CHECK(state = 'rejected' OR (binding_json IS NOT NULL AND expires_at_unix_ms IS NOT NULL))
            );
            CREATE UNIQUE INDEX IF NOT EXISTS idx_wc_plan_decisions_one_active
                ON wc_plan_decisions(plan_id) WHERE state = 'active';
            CREATE INDEX IF NOT EXISTS idx_wc_plan_decisions_plan
                ON wc_plan_decisions(plan_id, created_at_unix_ms DESC);
            ",
        )?;
        // Added after the first Plan/Goal schema: plan origin and the owner's
        // Goal dispatch principal as recorded for one revision (so the
        // operator can start exactly that revision).
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (column, definition) in [
            ("origin", "TEXT NOT NULL DEFAULT 'plan' CHECK(origin IN ('plan', 'goal'))"),
            ("dispatch_json", "TEXT"),
            ("dispatch_revision", "INTEGER"),
            ("dispatch_recorded_at_unix_ms", "INTEGER"),
        ] {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('wc_plans') WHERE name = ?1)",
                params![column],
                |row| row.get(0),
            )?;
            if !exists {
                tx.execute_batch(&format!("ALTER TABLE wc_plans ADD COLUMN {column} {definition}"))?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Create a direct Goal request: a one-revision plan that never engages
    /// Plan Mode and waits for the operator to start it in the Chadex app.
    /// Earlier unstarted requests of the same owner and project are closed.
    #[allow(clippy::too_many_arguments)]
    pub fn goal_request_create(
        &self,
        owner_key: &str,
        project_id: &str,
        title: &str,
        content: &str,
        spec: PlanSpec,
        dispatch: &serde_json::Value,
        now: i64,
    ) -> Result<PlanRevisionOutcome, PlanStoreError> {
        validate_key(owner_key, "owner_key")?;
        validate_key(project_id, "project")?;
        let title = validate_plan_title(title)?;
        let content = content.trim();
        if content.is_empty() || content.len() > MAX_PLAN_CONTENT_BYTES {
            return Err(PlanStoreError::new(
                "invalid_plan_content",
                format!("objective must contain 1..={MAX_PLAN_CONTENT_BYTES} UTF-8 bytes"),
            ));
        }
        let spec = spec.normalized()?;
        if spec.acceptance.is_empty() {
            return Err(PlanStoreError::new(
                "plan_missing_acceptance",
                "A Goal needs at least one acceptance check (a command whose exit code 0 means done)",
            ));
        }
        let content_sha256 = plan_content_sha256(content, &spec);
        let spec_json = serde_json::to_string(&spec).expect("PlanSpec serializes");
        let dispatch_json = serde_json::to_string(dispatch).expect("dispatch serializes");
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        release_expired_locks(&tx, now)?;
        if let Some(locked) = active_lock_plan(&tx, owner_key)? {
            return Err(PlanStoreError::new(
                "plan_mode_active_for_other_plan",
                format!("Plan Mode is active for plan {locked}; finish or exit it first"),
            ));
        }
        close_open_plans(&tx, Some(project_id), Some(owner_key), "replaced by a newer Goal request", now)?;
        let plan_id = allocate_identity(
            &tx,
            PLAN_ID_PREFIX,
            "SELECT EXISTS(SELECT 1 FROM wc_plans WHERE plan_id = ?1)",
        )
        .map_err(|error| PlanStoreError::new("plan_store_unavailable", error.message().to_string()))?;
        tx.execute(
            "INSERT INTO wc_plans (
                plan_id, owner_key, project_id, title, state, latest_revision,
                lock_active, lock_expires_at_unix_ms, created_at_unix_ms,
                updated_at_unix_ms, closed_reason, origin, dispatch_json, dispatch_revision,
                dispatch_recorded_at_unix_ms
             ) VALUES (?1, ?2, ?3, ?4, 'drafting', 1, 0, NULL, ?5, ?5, NULL, 'goal', ?6, 1, ?5)",
            params![plan_id, owner_key, project_id, title, now, dispatch_json],
        )
        .map_err(plan_store_error)?;
        tx.execute(
            "INSERT INTO wc_plan_revisions (
                plan_id, revision, content, spec_json, content_sha256, author_key,
                created_at_unix_ms
             ) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6)",
            params![plan_id, content, spec_json, content_sha256, owner_key, now],
        )
        .map_err(plan_store_error)?;
        let plan = load_plan_summary(&tx, &plan_id, now)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(PlanRevisionOutcome {
            plan,
            revision: PlanRevisionSummary {
                revision: 1,
                content_sha256,
                created_at_unix_ms: now,
            },
            superseded_approvals: 0,
        })
    }

    /// Record (or, with `None`, clear) the owner's Goal dispatch principal
    /// for `revision` of a plan they own. It only counts while `revision` is
    /// the latest, so a later save without a fresh principal never inherits
    /// an older one.
    pub fn plan_record_dispatch(
        &self,
        owner_key: &str,
        plan_id: &str,
        revision: i64,
        dispatch: Option<&serde_json::Value>,
        now: i64,
    ) -> Result<(), PlanStoreError> {
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        load_owned_plan(&tx, PlanReader::Owner(owner_key), plan_id, now)?;
        tx.execute(
            "UPDATE wc_plans SET dispatch_json = ?3, dispatch_revision = ?2,
                 dispatch_recorded_at_unix_ms = ?4
             WHERE plan_id = ?1 AND latest_revision = ?2",
            params![
                plan_id,
                revision,
                dispatch.map(|value| serde_json::to_string(value).expect("dispatch serializes")),
                now
            ],
        )
        .map_err(plan_store_error)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(())
    }

    /// Operator view for "approve and start": the plan owner, the dispatch
    /// principal recorded for the latest revision (None when missing or
    /// recorded for an older revision), and any other plan the owner is
    /// currently planning in Plan Mode.
    pub fn plan_start_context(
        &self,
        plan_id: &str,
        now: i64,
    ) -> Result<PlanStartContext, PlanStoreError> {
        validate_key(plan_id, "plan_id")?;
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        release_expired_locks(&tx, now)?;
        let row: Option<(String, i64, Option<String>, Option<i64>, Option<i64>)> = tx
            .query_row(
                "SELECT owner_key, latest_revision, dispatch_json, dispatch_revision,
                        dispatch_recorded_at_unix_ms
                 FROM wc_plans WHERE plan_id = ?1",
                params![plan_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .optional()
            .map_err(plan_store_error)?;
        let (owner_key, latest, dispatch, dispatch_revision, recorded_at) =
            row.ok_or_else(|| PlanStoreError::new("plan_not_found", "Plan not found"))?;
        let dispatch = match dispatch.filter(|_| dispatch_revision == Some(latest)) {
            Some(text) => Some(serde_json::from_str(&text).map_err(|_| {
                PlanStoreError::new("plan_store_corrupt", "Stored dispatch principal is unreadable")
            })?),
            None => None,
        };
        let owner_locked_plan = active_lock_plan(&tx, &owner_key)?.filter(|other| other != plan_id);
        tx.commit().map_err(plan_store_error)?;
        Ok(PlanStartContext {
            owner_key,
            dispatch_recorded_at_unix_ms: if dispatch.is_some() { recorded_at } else { None },
            dispatch,
            owner_locked_plan,
        })
    }

    /// Enter Plan Mode for `owner_key` on `project_id`. Idempotent for the same
    /// project: an already-active lock is returned (and renewed) instead of a
    /// second plan being created.
    pub fn plan_start(
        &self,
        owner_key: &str,
        project_id: &str,
        title: &str,
        now: i64,
    ) -> Result<PlanStartOutcome, PlanStoreError> {
        validate_key(owner_key, "owner_key")?;
        validate_key(project_id, "project")?;
        let title = validate_plan_title(title)?;
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        release_expired_locks(&tx, now)?;
        if let Some(existing) = active_lock_plan(&tx, owner_key)? {
            let summary = load_plan_summary(&tx, &existing, now)?;
            if summary.project_id != project_id {
                return Err(PlanStoreError::new(
                    "plan_mode_active_for_other_project",
                    format!(
                        "Plan Mode is already active for project {} (plan {}); finish or exit it in the Chadex app first",
                        summary.project_id, summary.plan_id
                    ),
                ));
            }
            renew_lock(&tx, &existing, now)?;
            let plan = load_plan_summary(&tx, &existing, now)?;
            tx.commit().map_err(plan_store_error)?;
            return Ok(PlanStartOutcome {
                plan,
                created: false,
            });
        }
        close_open_plans(&tx, Some(project_id), Some(owner_key), "replaced by a newer plan", now)?;
        let plan_id = allocate_identity(
            &tx,
            PLAN_ID_PREFIX,
            "SELECT EXISTS(SELECT 1 FROM wc_plans WHERE plan_id = ?1)",
        )
        .map_err(|error| PlanStoreError::new("plan_store_unavailable", error.message().to_string()))?;
        tx.execute(
            "INSERT INTO wc_plans (
                plan_id, owner_key, project_id, title, state, latest_revision,
                lock_active, lock_expires_at_unix_ms, created_at_unix_ms,
                updated_at_unix_ms, closed_reason
             ) VALUES (?1, ?2, ?3, ?4, 'drafting', 0, 1, ?5, ?6, ?6, NULL)",
            params![
                plan_id,
                owner_key,
                project_id,
                title,
                now + DEFAULT_PLAN_LOCK_TTL_MS,
                now
            ],
        )
        .map_err(plan_store_error)?;
        let plan = load_plan_summary(&tx, &plan_id, now)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(PlanStartOutcome {
            plan,
            created: true,
        })
    }

    /// Append one immutable revision. CAS on `expected_revision` prevents a
    /// stale writer from silently replacing a newer plan. Saving a revision on
    /// an approved plan returns it to drafting, supersedes the approval, and
    /// re-engages the owner's read-only lock.
    pub fn plan_save_revision(
        &self,
        owner_key: &str,
        plan_id: &str,
        expected_revision: i64,
        content: &str,
        spec: PlanSpec,
        now: i64,
    ) -> Result<PlanRevisionOutcome, PlanStoreError> {
        validate_key(owner_key, "owner_key")?;
        validate_key(plan_id, "plan_id")?;
        let content = content.trim();
        if content.is_empty() || content.len() > MAX_PLAN_CONTENT_BYTES {
            return Err(PlanStoreError::new(
                "invalid_plan_content",
                format!("plan content must contain 1..={MAX_PLAN_CONTENT_BYTES} UTF-8 bytes"),
            ));
        }
        let spec = spec.normalized()?;
        let content_sha256 = plan_content_sha256(content, &spec);
        let spec_json = serde_json::to_string(&spec).expect("PlanSpec serializes");
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        release_expired_locks(&tx, now)?;
        let summary = load_owned_plan(&tx, PlanReader::Owner(owner_key), plan_id, now)?;
        if summary.state == PlanState::Closed {
            return Err(PlanStoreError::new(
                "plan_closed",
                "This plan is closed; start a new plan instead",
            ));
        }
        if summary.origin == "goal" {
            return Err(PlanStoreError::new(
                "goal_request_immutable",
                "A direct Goal request cannot be revised; call goal_run_start again with the new objective",
            ));
        }
        if summary.latest_revision != expected_revision {
            return Err(PlanStoreError::new(
                "plan_revision_changed",
                format!(
                    "Plan changed; latest revision is {} (expected {expected_revision})",
                    summary.latest_revision
                ),
            ));
        }
        if !summary.plan_mode_active {
            if let Some(other) = active_lock_plan(&tx, owner_key)? {
                if other != plan_id {
                    return Err(PlanStoreError::new(
                        "plan_mode_active_for_other_plan",
                        format!("Plan Mode is active for plan {other}; finish or exit it first"),
                    ));
                }
            }
        }
        let revision = summary.latest_revision + 1;
        tx.execute(
            "INSERT INTO wc_plan_revisions (
                plan_id, revision, content, spec_json, content_sha256, author_key,
                created_at_unix_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![plan_id, revision, content, spec_json, content_sha256, owner_key, now],
        )
        .map_err(plan_store_error)?;
        let superseded = tx
            .execute(
                "UPDATE wc_plan_decisions SET state = 'superseded', decided_at_unix_ms = ?2
                 WHERE plan_id = ?1 AND state = 'active'",
                params![plan_id, now],
            )
            .map_err(plan_store_error)?;
        tx.execute(
            "UPDATE wc_plans SET latest_revision = ?2, state = 'drafting', lock_active = 1,
                 lock_expires_at_unix_ms = ?3, updated_at_unix_ms = ?4
             WHERE plan_id = ?1",
            params![plan_id, revision, now + DEFAULT_PLAN_LOCK_TTL_MS, now],
        )
        .map_err(plan_store_error)?;
        let plan = load_plan_summary(&tx, plan_id, now)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(PlanRevisionOutcome {
            plan,
            revision: PlanRevisionSummary {
                revision,
                content_sha256,
                created_at_unix_ms: now,
            },
            superseded_approvals: superseded,
        })
    }

    pub fn plan_read(
        &self,
        reader: PlanReader<'_>,
        plan_id: &str,
        revision: Option<i64>,
        now: i64,
    ) -> Result<PlanDetail, PlanStoreError> {
        validate_key(plan_id, "plan_id")?;
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        release_expired_locks(&tx, now)?;
        expire_approvals(&tx, now)?;
        let plan = load_owned_plan(&tx, reader, plan_id, now)?;
        let wanted = revision.unwrap_or(plan.latest_revision);
        let revision = if wanted == 0 {
            None
        } else {
            Some(load_revision(&tx, plan_id, wanted)?.ok_or_else(|| {
                PlanStoreError::new(
                    "plan_revision_not_found",
                    format!("Plan revision {wanted} does not exist"),
                )
            })?)
        };
        let revisions = load_revision_summaries(&tx, plan_id)?;
        let decisions = load_decisions(&tx, plan_id)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(PlanDetail {
            plan,
            revision,
            revisions,
            decisions,
        })
    }

    pub fn plan_list(
        &self,
        reader: PlanReader<'_>,
        include_closed: bool,
        limit: usize,
        now: i64,
    ) -> Result<Vec<PlanSummary>, PlanStoreError> {
        let limit = limit.clamp(1, MAX_PLAN_LIST_LIMIT) as i64;
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        release_expired_locks(&tx, now)?;
        let ids: Vec<String> = {
            let (sql, owner) = match reader {
                PlanReader::Owner(owner) => (
                    "SELECT plan_id FROM wc_plans WHERE owner_key = ?1 AND (?2 OR state <> 'closed')
                     ORDER BY updated_at_unix_ms DESC, plan_id LIMIT ?3",
                    owner.to_string(),
                ),
                PlanReader::Operator => (
                    "SELECT plan_id FROM wc_plans WHERE (?1 = ?1) AND (?2 OR state <> 'closed')
                     ORDER BY updated_at_unix_ms DESC, plan_id LIMIT ?3",
                    String::new(),
                ),
            };
            let mut statement = tx.prepare(sql).map_err(plan_store_error)?;
            let rows = statement
                .query_map(params![owner, include_closed, limit], |row| {
                    row.get::<_, String>(0)
                })
                .map_err(plan_store_error)?;
            rows.collect::<Result<_, _>>().map_err(plan_store_error)?
        };
        let mut plans = Vec::with_capacity(ids.len());
        for id in ids {
            plans.push(load_plan_summary(&tx, &id, now)?);
        }
        tx.commit().map_err(plan_store_error)?;
        Ok(plans)
    }

    /// Trusted operator approval of one exact revision. The caller must echo
    /// the revision and content hash it displayed, so a revision saved while
    /// the reviewer was reading cannot be approved by accident.
    #[allow(clippy::too_many_arguments)]
    pub fn plan_approve(
        &self,
        plan_id: &str,
        revision: i64,
        content_sha256: &str,
        binding: PlanApprovalBinding,
        approver: &str,
        ttl_ms: i64,
        now: i64,
    ) -> Result<PlanDecisionRecord, PlanStoreError> {
        self.plan_approve_with(plan_id, revision, content_sha256, binding, approver, ttl_ms, true, now)
    }

    /// As `plan_approve`, but a plan that ChatGPT implements itself needs no
    /// acceptance check; only approval for Goal Mode does.
    #[allow(clippy::too_many_arguments)]
    pub fn plan_approve_with(
        &self,
        plan_id: &str,
        revision: i64,
        content_sha256: &str,
        binding: PlanApprovalBinding,
        approver: &str,
        ttl_ms: i64,
        require_acceptance: bool,
        now: i64,
    ) -> Result<PlanDecisionRecord, PlanStoreError> {
        validate_key(plan_id, "plan_id")?;
        validate_key(approver, "approver")?;
        for (value, field) in [
            (&binding.project_id, "project_id"),
            (&binding.runner_client_id, "runner_client_id"),
            (&binding.project_root, "project_root"),
            (&binding.base_commit, "base_commit"),
        ] {
            validate_key(value, field)?;
        }
        let ttl_ms = ttl_ms.clamp(60_000, 7 * DEFAULT_PLAN_APPROVAL_TTL_MS);
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        release_expired_locks(&tx, now)?;
        let plan = load_owned_plan(&tx, PlanReader::Operator, plan_id, now)?;
        if plan.state == PlanState::Closed {
            return Err(PlanStoreError::new("plan_closed", "This plan is closed"));
        }
        let record = check_reviewable_revision(&tx, &plan, revision, content_sha256)?;
        if require_acceptance && record.spec.acceptance.is_empty() {
            return Err(PlanStoreError::new(
                "plan_missing_acceptance",
                "A plan needs at least one acceptance check before it can be approved for Goal Mode",
            ));
        }
        if binding.project_id != plan.project_id {
            return Err(PlanStoreError::new(
                "plan_project_mismatch",
                "Approval binding does not match the plan's project",
            ));
        }
        if require_acceptance {
            // Goal approval is single-use per immutable revision, even when
            // approve-and-start is retried after the run consumed its approval.
            // Already-started revisions cannot receive a fresh approval.
            let consumed_approval: Option<String> = tx
                .query_row(
                    "SELECT approval_id FROM wc_plan_decisions
                     WHERE plan_id = ?1 AND revision = ?2 AND content_sha256 = ?3 AND state = 'consumed'
                     ORDER BY decided_at_unix_ms, approval_id LIMIT 1",
                    params![plan_id, revision, content_sha256],
                    |row| row.get(0),
                )
                .optional()
                .map_err(plan_store_error)?;
            if let Some(approval_id) = consumed_approval {
                let decision = load_decision(&tx, &approval_id)?;
                let run_id = decision.consumed_by_goal_run_id.as_deref().ok_or_else(|| {
                    PlanStoreError::new(
                        "plan_store_corrupt",
                        "Consumed Goal approval is missing its run binding",
                    )
                })?;
                let run_binding_json: Option<String> = tx
                    .query_row(
                        "SELECT binding_json FROM wc_goal_runs
                         WHERE goal_run_id = ?1 AND plan_id = ?2 AND revision = ?3 AND content_sha256 = ?4",
                        params![run_id, plan_id, revision, content_sha256],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(plan_store_error)?;
                let run_binding_json = run_binding_json.ok_or_else(|| {
                    PlanStoreError::new(
                        "plan_store_corrupt",
                        "Consumed Goal approval points to a missing or different run",
                    )
                })?;
                let run_binding: PlanApprovalBinding =
                    serde_json::from_str(&run_binding_json).map_err(|_| {
                        PlanStoreError::new(
                            "plan_store_corrupt",
                            "Consumed Goal run has an unreadable approval binding",
                        )
                    })?;
                if decision.binding.as_ref() != Some(&binding) || run_binding != binding {
                    return Err(PlanStoreError::new(
                        "plan_approval_environment_changed",
                        "This plan revision already started with a different Runner, root, or base commit",
                    ));
                }
                tx.commit().map_err(plan_store_error)?;
                return Ok(decision);
            }

            // Direct Goal requests share a still-valid active approval while
            // concurrent starts race to consume it. Ordinary Plan reapproval
            // retains its superseding semantics below.
            if plan.origin == "goal" {
                let active_approval: Option<String> = tx
                    .query_row(
                        "SELECT approval_id FROM wc_plan_decisions
                         WHERE plan_id = ?1 AND revision = ?2 AND content_sha256 = ?3
                           AND state = 'active' AND expires_at_unix_ms > ?4
                         ORDER BY created_at_unix_ms, approval_id LIMIT 1",
                        params![plan_id, revision, content_sha256, now],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(plan_store_error)?;
                if let Some(approval_id) = active_approval {
                    let decision = load_decision(&tx, &approval_id)?;
                    if decision.binding.as_ref() == Some(&binding) {
                        tx.commit().map_err(plan_store_error)?;
                        return Ok(decision);
                    }
                }
            }
        }
        tx.execute(
            "UPDATE wc_plan_decisions SET state = 'superseded', decided_at_unix_ms = ?2
             WHERE plan_id = ?1 AND state = 'active'",
            params![plan_id, now],
        )
        .map_err(plan_store_error)?;
        let approval_id = allocate_identity(
            &tx,
            PLAN_APPROVAL_ID_PREFIX,
            "SELECT EXISTS(SELECT 1 FROM wc_plan_decisions WHERE approval_id = ?1)",
        )
        .map_err(|error| PlanStoreError::new("plan_store_unavailable", error.message().to_string()))?;
        let binding_json = serde_json::to_string(&binding).expect("binding serializes");
        tx.execute(
            "INSERT INTO wc_plan_decisions (
                approval_id, plan_id, revision, content_sha256, state, binding_json,
                approver, channel, reason, created_at_unix_ms, expires_at_unix_ms,
                decided_at_unix_ms, consumed_by_goal_run_id
             ) VALUES (?1, ?2, ?3, ?4, 'active', ?5, ?6, 'chadex_app', NULL, ?7, ?8, ?7, NULL)",
            params![
                approval_id,
                plan_id,
                revision,
                content_sha256,
                binding_json,
                approver,
                now,
                now + ttl_ms
            ],
        )
        .map_err(plan_store_error)?;
        tx.execute(
            "UPDATE wc_plans SET state = 'approved', lock_active = 0,
                 lock_expires_at_unix_ms = NULL, updated_at_unix_ms = ?2
             WHERE plan_id = ?1",
            params![plan_id, now],
        )
        .map_err(plan_store_error)?;
        let decision = load_decision(&tx, &approval_id)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(decision)
    }

    /// Record a "request changes" decision. The plan stays in drafting and the
    /// read-only lock stays engaged.
    pub fn plan_reject(
        &self,
        plan_id: &str,
        revision: i64,
        content_sha256: &str,
        approver: &str,
        reason: &str,
        now: i64,
    ) -> Result<PlanDecisionRecord, PlanStoreError> {
        validate_key(plan_id, "plan_id")?;
        validate_key(approver, "approver")?;
        let reason = bounded_reason(reason)?;
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        release_expired_locks(&tx, now)?;
        let plan = load_owned_plan(&tx, PlanReader::Operator, plan_id, now)?;
        if plan.state == PlanState::Closed {
            return Err(PlanStoreError::new("plan_closed", "This plan is closed"));
        }
        if plan.origin == "goal" {
            // A direct Goal request cannot be revised, and must never put its
            // owner into Plan Mode: discard it instead.
            return Err(PlanStoreError::new(
                "goal_request_immutable",
                "A direct Goal request cannot be sent back for changes; discard it instead",
            ));
        }
        check_reviewable_revision(&tx, &plan, revision, content_sha256)?;
        let approval_id = allocate_identity(
            &tx,
            PLAN_APPROVAL_ID_PREFIX,
            "SELECT EXISTS(SELECT 1 FROM wc_plan_decisions WHERE approval_id = ?1)",
        )
        .map_err(|error| PlanStoreError::new("plan_store_unavailable", error.message().to_string()))?;
        tx.execute(
            "INSERT INTO wc_plan_decisions (
                approval_id, plan_id, revision, content_sha256, state, binding_json,
                approver, channel, reason, created_at_unix_ms, expires_at_unix_ms,
                decided_at_unix_ms, consumed_by_goal_run_id
             ) VALUES (?1, ?2, ?3, ?4, 'rejected', NULL, ?5, 'chadex_app', ?6, ?7, NULL, ?7, NULL)",
            params![approval_id, plan_id, revision, content_sha256, approver, reason, now],
        )
        .map_err(plan_store_error)?;
        // Requesting changes withdraws any approval and returns the plan to
        // read-only drafting for its owner.
        tx.execute(
            "UPDATE wc_plan_decisions SET state = 'revoked', decided_at_unix_ms = ?2
             WHERE plan_id = ?1 AND state = 'active'",
            params![plan_id, now],
        )
        .map_err(plan_store_error)?;
        let owner: String = tx
            .query_row(
                "SELECT owner_key FROM wc_plans WHERE plan_id = ?1",
                params![plan_id],
                |row| row.get(0),
            )
            .map_err(plan_store_error)?;
        let other_lock = active_lock_plan(&tx, &owner)?.filter(|other| other != plan_id);
        tx.execute(
            "UPDATE wc_plans SET state = 'drafting', lock_active = ?2,
                 lock_expires_at_unix_ms = CASE WHEN ?2 = 1 THEN ?3 ELSE lock_expires_at_unix_ms END,
                 updated_at_unix_ms = ?4
             WHERE plan_id = ?1",
            params![
                plan_id,
                if other_lock.is_none() { 1 } else { 0 },
                now + DEFAULT_PLAN_LOCK_TTL_MS,
                now
            ],
        )
        .map_err(plan_store_error)?;
        let decision = load_decision(&tx, &approval_id)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(decision)
    }

    pub fn plan_revoke_approval(
        &self,
        approval_id: &str,
        now: i64,
    ) -> Result<PlanDecisionRecord, PlanStoreError> {
        validate_key(approval_id, "approval_id")?;
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        let changed = tx
            .execute(
                "UPDATE wc_plan_decisions SET state = 'revoked', decided_at_unix_ms = ?2
                 WHERE approval_id = ?1 AND state = 'active'",
                params![approval_id, now],
            )
            .map_err(plan_store_error)?;
        if changed == 0 {
            return Err(PlanStoreError::new(
                "plan_approval_not_active",
                "Approval is not active (already consumed, revoked, superseded, or expired)",
            ));
        }
        let decision = load_decision(&tx, approval_id)?;
        tx.execute(
            "UPDATE wc_plans SET state = 'drafting', updated_at_unix_ms = ?2
             WHERE plan_id = ?1 AND state = 'approved'",
            params![decision.plan_id, now],
        )
        .map_err(plan_store_error)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(decision)
    }

    /// Operator exit from Plan Mode: closes the plan, releases the lock and
    /// revokes any active approval.
    pub fn plan_close(
        &self,
        plan_id: &str,
        reason: &str,
        now: i64,
    ) -> Result<PlanSummary, PlanStoreError> {
        validate_key(plan_id, "plan_id")?;
        let reason = bounded_reason(reason)?;
        let mut conn = self.lock_connection(crate::StoreDomain::Goal);
        let tx = conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(plan_store_error)?;
        load_owned_plan(&tx, PlanReader::Operator, plan_id, now)?;
        tx.execute(
            "UPDATE wc_plan_decisions SET state = 'revoked', decided_at_unix_ms = ?2
             WHERE plan_id = ?1 AND state = 'active'",
            params![plan_id, now],
        )
        .map_err(plan_store_error)?;
        tx.execute(
            "UPDATE wc_plans SET state = 'closed', lock_active = 0,
                 lock_expires_at_unix_ms = NULL, closed_reason = ?2, updated_at_unix_ms = ?3
             WHERE plan_id = ?1",
            params![plan_id, reason, now],
        )
        .map_err(plan_store_error)?;
        let plan = load_plan_summary(&tx, plan_id, now)?;
        tx.commit().map_err(plan_store_error)?;
        Ok(plan)
    }

    /// Every unexpired lock. Used to (re)build the in-memory guard cache.
    pub fn plan_active_locks(&self, now: i64) -> Result<Vec<PlanLockRecord>, PlanStoreError> {
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        let mut statement = conn
            .prepare(
                "SELECT owner_key, plan_id, project_id, lock_expires_at_unix_ms FROM wc_plans
                 WHERE lock_active = 1 AND lock_expires_at_unix_ms > ?1",
            )
            .map_err(plan_store_error)?;
        let rows = statement
            .query_map(params![now], |row| {
                Ok(PlanLockRecord {
                    owner_key: row.get(0)?,
                    plan_id: row.get(1)?,
                    project_id: row.get(2)?,
                    expires_at_unix_ms: row.get(3)?,
                })
            })
            .map_err(plan_store_error)?;
        rows.collect::<Result<_, _>>().map_err(plan_store_error)
    }

    pub fn plan_revision(
        &self,
        plan_id: &str,
        revision: i64,
    ) -> Result<Option<PlanRevisionRecord>, PlanStoreError> {
        let conn = self.lock_connection(crate::StoreDomain::Goal);
        load_revision(&conn, plan_id, revision)
    }

    pub fn plan_now_unix_ms() -> i64 {
        now_unix_ms()
    }
}

fn check_reviewable_revision(
    tx: &Transaction<'_>,
    plan: &PlanSummary,
    revision: i64,
    content_sha256: &str,
) -> Result<PlanRevisionRecord, PlanStoreError> {
    if revision != plan.latest_revision || revision == 0 {
        return Err(PlanStoreError::new(
            "plan_revision_stale",
            format!(
                "Only the latest revision ({}) can be reviewed; reload the plan",
                plan.latest_revision
            ),
        ));
    }
    let record = load_revision(tx, &plan.plan_id, revision)?.ok_or_else(|| {
        PlanStoreError::new("plan_revision_not_found", "Plan revision does not exist")
    })?;
    if record.content_sha256 != content_sha256 {
        return Err(PlanStoreError::new(
            "plan_content_hash_mismatch",
            "The reviewed content hash does not match the stored revision",
        ));
    }
    Ok(record)
}

/// Close every open plan (of one project and/or owner, or all), withdrawing
/// unused approvals and Plan Mode locks. A principal's new plan or Goal
/// request replaces its earlier ones on the project; only the operator clears
/// other principals' plans. Records stay for audit; Goal runs are unaffected.
pub(crate) fn close_open_plans(
    conn: &Connection,
    project_id: Option<&str>,
    owner_key: Option<&str>,
    reason: &str,
    now: i64,
) -> Result<usize, PlanStoreError> {
    let scope = "state != 'closed' AND (?1 IS NULL OR project_id = ?1) AND (?2 IS NULL OR owner_key = ?2)";
    conn.execute(
        &format!(
            "UPDATE wc_plan_decisions SET state = 'revoked', decided_at_unix_ms = ?3
             WHERE state = 'active' AND plan_id IN (SELECT plan_id FROM wc_plans WHERE {scope})"
        ),
        params![project_id, owner_key, now],
    )
    .map_err(plan_store_error)?;
    conn.execute(
        &format!(
            "UPDATE wc_plans SET state = 'closed', closed_reason = ?4, lock_active = 0,
                 lock_expires_at_unix_ms = NULL, updated_at_unix_ms = ?3
             WHERE {scope}"
        ),
        params![project_id, owner_key, now, reason],
    )
    .map_err(plan_store_error)
}

pub(crate) fn release_expired_locks(conn: &Connection, now: i64) -> Result<(), PlanStoreError> {
    conn.execute(
        "UPDATE wc_plans SET lock_active = 0, lock_expires_at_unix_ms = NULL,
             updated_at_unix_ms = ?1
         WHERE lock_active = 1 AND lock_expires_at_unix_ms <= ?1",
        params![now],
    )
    .map_err(plan_store_error)?;
    Ok(())
}

pub(crate) fn expire_approvals(conn: &Connection, now: i64) -> Result<(), PlanStoreError> {
    conn.execute(
        "UPDATE wc_plan_decisions SET state = 'expired', decided_at_unix_ms = ?1
         WHERE state = 'active' AND expires_at_unix_ms <= ?1",
        params![now],
    )
    .map_err(plan_store_error)?;
    Ok(())
}

fn active_lock_plan(conn: &Connection, owner_key: &str) -> Result<Option<String>, PlanStoreError> {
    conn.query_row(
        "SELECT plan_id FROM wc_plans WHERE owner_key = ?1 AND lock_active = 1",
        params![owner_key],
        |row| row.get(0),
    )
    .optional()
    .map_err(plan_store_error)
}

fn renew_lock(conn: &Connection, plan_id: &str, now: i64) -> Result<(), PlanStoreError> {
    conn.execute(
        "UPDATE wc_plans SET lock_expires_at_unix_ms = ?2, updated_at_unix_ms = ?3
         WHERE plan_id = ?1 AND lock_active = 1",
        params![plan_id, now + DEFAULT_PLAN_LOCK_TTL_MS, now],
    )
    .map_err(plan_store_error)?;
    Ok(())
}

pub(crate) fn load_owned_plan(
    conn: &Connection,
    reader: PlanReader<'_>,
    plan_id: &str,
    now: i64,
) -> Result<PlanSummary, PlanStoreError> {
    let owner: Option<String> = conn
        .query_row(
            "SELECT owner_key FROM wc_plans WHERE plan_id = ?1",
            params![plan_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(plan_store_error)?;
    match (owner, reader) {
        (None, _) => Err(PlanStoreError::new("plan_not_found", "Plan not found")),
        // Do not reveal another principal's plan.
        (Some(owner), PlanReader::Owner(reader)) if owner != reader => {
            Err(PlanStoreError::new("plan_not_found", "Plan not found"))
        }
        _ => load_plan_summary(conn, plan_id, now),
    }
}

pub(crate) fn load_plan_summary(
    conn: &Connection,
    plan_id: &str,
    now: i64,
) -> Result<PlanSummary, PlanStoreError> {
    conn.query_row(
        "SELECT plan_id, project_id, title, state, latest_revision, lock_active,
                lock_expires_at_unix_ms, created_at_unix_ms, updated_at_unix_ms, closed_reason,
                origin
         FROM wc_plans WHERE plan_id = ?1",
        params![plan_id],
        |row| {
            let lock_active: i64 = row.get(5)?;
            let expires: Option<i64> = row.get(6)?;
            let active = lock_active == 1 && expires.is_some_and(|value| value > now);
            Ok(PlanSummary {
                plan_id: row.get(0)?,
                project_id: row.get(1)?,
                title: row.get(2)?,
                state: PlanState::from_db(&row.get::<_, String>(3)?)?,
                latest_revision: row.get(4)?,
                origin: row.get(10)?,
                plan_mode_active: active,
                plan_mode_expires_at_unix_ms: if active { expires } else { None },
                created_at_unix_ms: row.get(7)?,
                updated_at_unix_ms: row.get(8)?,
                closed_reason: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(plan_store_error)?
    .ok_or_else(|| PlanStoreError::new("plan_not_found", "Plan not found"))
}

pub(crate) fn load_revision(
    conn: &Connection,
    plan_id: &str,
    revision: i64,
) -> Result<Option<PlanRevisionRecord>, PlanStoreError> {
    conn.query_row(
        "SELECT revision, content, spec_json, content_sha256, created_at_unix_ms
         FROM wc_plan_revisions WHERE plan_id = ?1 AND revision = ?2",
        params![plan_id, revision],
        |row| {
            let spec_json: String = row.get(2)?;
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                spec_json,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        },
    )
    .optional()
    .map_err(plan_store_error)?
    .map(|(revision, content, spec_json, content_sha256, created)| {
        let spec: PlanSpec = serde_json::from_str(&spec_json).map_err(|_| {
            PlanStoreError::new("plan_store_corrupt", "Stored plan spec is unreadable")
        })?;
        // Integrity check: the stored hash must still describe the stored body.
        if plan_content_sha256(&content, &spec) != content_sha256 {
            return Err(PlanStoreError::new(
                "plan_store_corrupt",
                "Stored plan revision does not match its content hash",
            ));
        }
        Ok(PlanRevisionRecord {
            revision,
            content,
            spec,
            content_sha256,
            created_at_unix_ms: created,
        })
    })
    .transpose()
}

fn load_revision_summaries(
    conn: &Connection,
    plan_id: &str,
) -> Result<Vec<PlanRevisionSummary>, PlanStoreError> {
    let mut statement = conn
        .prepare(
            "SELECT revision, content_sha256, created_at_unix_ms FROM wc_plan_revisions
             WHERE plan_id = ?1 ORDER BY revision",
        )
        .map_err(plan_store_error)?;
    let rows = statement
        .query_map(params![plan_id], |row| {
            Ok(PlanRevisionSummary {
                revision: row.get(0)?,
                content_sha256: row.get(1)?,
                created_at_unix_ms: row.get(2)?,
            })
        })
        .map_err(plan_store_error)?;
    rows.collect::<Result<_, _>>().map_err(plan_store_error)
}

const DECISION_COLUMNS: &str = "approval_id, plan_id, revision, content_sha256, state,
    binding_json, approver, channel, reason, created_at_unix_ms, expires_at_unix_ms,
    consumed_by_goal_run_id";

fn decision_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PlanDecisionRecord> {
    let binding_json: Option<String> = row.get(5)?;
    Ok(PlanDecisionRecord {
        approval_id: row.get(0)?,
        plan_id: row.get(1)?,
        revision: row.get(2)?,
        content_sha256: row.get(3)?,
        state: PlanDecisionState::from_db(&row.get::<_, String>(4)?)?,
        binding: binding_json.and_then(|value| serde_json::from_str(&value).ok()),
        approver: row.get(6)?,
        channel: row.get(7)?,
        reason: row.get(8)?,
        created_at_unix_ms: row.get(9)?,
        expires_at_unix_ms: row.get(10)?,
        consumed_by_goal_run_id: row.get(11)?,
    })
}

pub(crate) fn load_decision(
    conn: &Connection,
    approval_id: &str,
) -> Result<PlanDecisionRecord, PlanStoreError> {
    conn.query_row(
        &format!("SELECT {DECISION_COLUMNS} FROM wc_plan_decisions WHERE approval_id = ?1"),
        params![approval_id],
        decision_from_row,
    )
    .optional()
    .map_err(plan_store_error)?
    .ok_or_else(|| PlanStoreError::new("plan_approval_not_found", "Plan approval not found"))
}

fn load_decisions(
    conn: &Connection,
    plan_id: &str,
) -> Result<Vec<PlanDecisionRecord>, PlanStoreError> {
    let mut statement = conn
        .prepare(&format!(
            "SELECT {DECISION_COLUMNS} FROM wc_plan_decisions WHERE plan_id = ?1
             ORDER BY created_at_unix_ms DESC, approval_id LIMIT 20"
        ))
        .map_err(plan_store_error)?;
    let rows = statement
        .query_map(params![plan_id], decision_from_row)
        .map_err(plan_store_error)?;
    rows.collect::<Result<_, _>>().map_err(plan_store_error)
}

fn validate_plan_title(value: &str) -> Result<String, PlanStoreError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_PLAN_TITLE_CHARS {
        return Err(PlanStoreError::new(
            "invalid_plan_title",
            format!("plan title must contain 1..={MAX_PLAN_TITLE_CHARS} characters"),
        ));
    }
    Ok(value.to_string())
}

fn validate_key(value: &str, field: &'static str) -> Result<(), PlanStoreError> {
    if value.is_empty() || value.len() > 4096 || value.contains('\0') {
        return Err(PlanStoreError::new(
            "invalid_plan_argument",
            format!("{field} must contain 1..=4096 non-NUL bytes"),
        ));
    }
    Ok(())
}

fn bounded_reason(value: &str) -> Result<String, PlanStoreError> {
    let value = value.trim();
    if value.len() > MAX_PLAN_REASON_BYTES {
        return Err(PlanStoreError::new(
            "invalid_plan_reason",
            format!("reason must be at most {MAX_PLAN_REASON_BYTES} bytes"),
        ));
    }
    Ok(value.to_string())
}

/// Bounded line diff between two revisions (LCS). Returns unified-style lines
/// prefixed with ' ', '-', '+'. Inputs larger than the bound return None.
pub fn plan_line_diff(old: &str, new: &str) -> Option<Vec<String>> {
    const MAX_LINES: usize = 2000;
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    if old.len() > MAX_LINES || new.len() > MAX_LINES {
        return None;
    }
    let (n, m) = (old.len(), new.len());
    let mut lcs = vec![0u16; (n + 1) * (m + 1)];
    let idx = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[idx(i, j)] = if old[i] == new[j] {
                lcs[idx(i + 1, j + 1)] + 1
            } else {
                lcs[idx(i + 1, j)].max(lcs[idx(i, j + 1)])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n || j < m {
        if i < n && j < m && old[i] == new[j] {
            out.push(format!(" {}", old[i]));
            i += 1;
            j += 1;
        } else if i < n && (j == m || lcs[idx(i + 1, j)] >= lcs[idx(i, j + 1)]) {
            out.push(format!("-{}", old[i]));
            i += 1;
        } else {
            out.push(format!("+{}", new[j]));
            j += 1;
        }
    }
    Some(out)
}
