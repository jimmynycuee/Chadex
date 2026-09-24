//! Explicit execution roots and bounded concurrency for deterministic Chadex tasks.
//!
//! A task never relies on the process working directory to identify its mutation
//! boundary.  The runtime passes the registered Runner Project for the active
//! [`ExecutionWorkspace`] to every nested search/read/process/mutation call.

use fs2::FileExt;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use super::project_resolution::ProjectResolverErrorKind;
use super::{ResolvedProject, ToolRuntime};
use crate::auth::AuthContext;

pub(crate) const DEFAULT_MAX_CONCURRENT_TASKS: usize = 2;
pub(crate) const DEFAULT_GLOBAL_EXECUTOR_CONCURRENCY: usize = 3;
pub(crate) const MAX_CONFIGURED_CONCURRENCY: usize = 3;

/// The lifetime scope of an execution root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IsolationMode {
    /// Read-only work on the selected registered project.
    None,
    /// One detached worktree for the whole task.
    Task,
    /// One detached worktree per independent package.
    Package,
    /// A detached worktree used only to reconcile and validate package results.
    Integration,
}

impl IsolationMode {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Task => "task",
            Self::Package => "package",
            Self::Integration => "integration",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "task" => Some(Self::Task),
            "package" => Some(Self::Package),
            "integration" => Some(Self::Integration),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceState {
    Active,
    Applied,
    Preserved,
    Cleaned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum WorkspaceCleanupPhase {
    #[default]
    None,
    IntentPersisted,
    ProjectUnregistered,
    WorktreeRemoved,
    Complete,
}

impl WorkspaceCleanupPhase {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::IntentPersisted => "intent_persisted",
            Self::ProjectUnregistered => "project_unregistered",
            Self::WorktreeRemoved => "worktree_removed",
            Self::Complete => "complete",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "intent_persisted" => Some(Self::IntentPersisted),
            "project_unregistered" => Some(Self::ProjectUnregistered),
            "worktree_removed" => Some(Self::WorktreeRemoved),
            "complete" => Some(Self::Complete),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct WorkspacePrepareIntent {
    pub(crate) task_id: String,
    pub(crate) mode: IsolationMode,
    pub(crate) source_project: String,
    pub(crate) client_id: String,
    pub(crate) source_path: String,
    pub(crate) operation_id: String,
    pub(crate) base_ref: Option<String>,
    pub(crate) include_dirty_snapshot: bool,
    pub(crate) source_head_sha: Option<String>,
    pub(crate) source_tree_sha: Option<String>,
    pub(crate) source_index_tree_sha: Option<String>,
}

impl WorkspacePrepareIntent {
    fn manifest(&self) -> Value {
        json!({
            "intent_version": 1,
            "task_id": self.task_id,
            "mode": self.mode.as_str(),
            "source_project": self.source_project,
            "client_id": self.client_id,
            "source_path": self.source_path,
            "operation_id": self.operation_id,
            "base_ref": self.base_ref,
            "include_dirty_snapshot": self.include_dirty_snapshot,
            "source_head_sha": self.source_head_sha,
            "source_tree_sha": self.source_tree_sha,
            "source_index_tree_sha": self.source_index_tree_sha,
            "updated_at_ms": now_epoch_ms(),
        })
    }
}

impl WorkspaceState {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Applied => "applied",
            Self::Preserved => "preserved",
            Self::Cleaned => "cleaned",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "applied" => Some(Self::Applied),
            "preserved" => Some(Self::Preserved),
            "cleaned" => Some(Self::Cleaned),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(crate) struct WorkspaceTimings {
    pub(crate) snapshot_ms: u64,
    pub(crate) worktree_creation_ms: u64,
    pub(crate) scheduler_wait_ms: u64,
    pub(crate) execution_ms: u64,
    pub(crate) integration_ms: u64,
    pub(crate) validation_ms: u64,
    pub(crate) apply_back_ms: u64,
    pub(crate) cleanup_ms: u64,
}

impl WorkspaceTimings {
    pub(crate) fn as_value(&self) -> Value {
        json!({
            "snapshot_ms": self.snapshot_ms,
            "worktree_creation_ms": self.worktree_creation_ms,
            "scheduler_wait_ms": self.scheduler_wait_ms,
            "execution_ms": self.execution_ms,
            "integration_ms": self.integration_ms,
            "validation_ms": self.validation_ms,
            "apply_back_ms": self.apply_back_ms,
            "cleanup_ms": self.cleanup_ms,
        })
    }
}

/// Server-side identity of one explicit execution root. `root` is intentionally
/// kept internal; model projections expose the runtime Project identity and
/// lifecycle state instead of leaking local filesystem paths.
#[derive(Debug, Clone)]
pub(crate) struct ExecutionWorkspace {
    pub(crate) task_id: String,
    pub(crate) mode: IsolationMode,
    pub(crate) source_project: String,
    pub(crate) execution_project: String,
    pub(crate) root: String,
    pub(crate) base_sha: Option<String>,
    pub(crate) source_head_sha: Option<String>,
    pub(crate) source_tree_sha: Option<String>,
    pub(crate) source_index_tree_sha: Option<String>,
    pub(crate) registered_revision: Option<String>,
    pub(crate) prepare_operation_id: Option<String>,
    pub(crate) cleanup_operation_id: Option<String>,
    pub(crate) cleanup_phase: WorkspaceCleanupPhase,
    pub(crate) state: WorkspaceState,
    pub(crate) preserve_reason: Option<String>,
    pub(crate) timings: WorkspaceTimings,
}

impl ExecutionWorkspace {
    pub(crate) fn read_only(task_id: String, project: String, root: String) -> Self {
        Self {
            task_id,
            mode: IsolationMode::None,
            source_project: project.clone(),
            execution_project: project,
            root,
            base_sha: None,
            source_head_sha: None,
            source_tree_sha: None,
            source_index_tree_sha: None,
            registered_revision: None,
            prepare_operation_id: None,
            cleanup_operation_id: None,
            cleanup_phase: WorkspaceCleanupPhase::None,
            state: WorkspaceState::Active,
            preserve_reason: None,
            timings: WorkspaceTimings::default(),
        }
    }

    pub(crate) fn projection(&self) -> Value {
        let mut value = json!({
            "mode": self.mode.as_str(),
            "state": self.state.as_str(),
            "source_project": self.source_project,
            "execution_project": self.execution_project,
            "base_sha": self.base_sha,
            "source_head_sha": self.source_head_sha,
            "source_tree_sha": self.source_tree_sha,
            "source_index_tree_sha": self.source_index_tree_sha,
            "preserve_reason": self.preserve_reason,
            "cleanup_phase": self.cleanup_phase.as_str(),
            "cleanup_pending": self.cleanup_operation_id.is_some()
                && self.cleanup_phase != WorkspaceCleanupPhase::Complete,
            "timings": self.timings.as_value(),
        });
        if self.mode == IsolationMode::None {
            value["worktree_created"] = json!(false);
        } else {
            value["worktree_created"] = json!(true);
        }
        value
    }

    pub(crate) fn manifest(&self) -> Value {
        let mut value = self.projection();
        value["manifest_version"] = json!(2);
        value["task_id"] = json!(self.task_id);
        value["execution_root"] = json!(self.root);
        value["registered_revision"] = json!(self.registered_revision);
        value["prepare_operation_id"] = json!(self.prepare_operation_id);
        value["cleanup_operation_id"] = json!(self.cleanup_operation_id);
        value["cleanup_phase"] = json!(self.cleanup_phase.as_str());
        value["updated_at_ms"] = json!(now_epoch_ms());
        value
    }
}

/// Centralized process-local scheduler. A task permit limits independent
/// incoming tasks; executor permits limit nested package/step execution.
#[derive(Clone)]
pub(crate) struct ExecutionScheduler {
    task_slots: Arc<Semaphore>,
    executor_slots: Arc<Semaphore>,
    max_tasks: usize,
    max_executor: usize,
}

impl Default for ExecutionScheduler {
    fn default() -> Self {
        let max_tasks =
            configured_limit("CHADEX_MAX_CONCURRENT_TASKS", DEFAULT_MAX_CONCURRENT_TASKS);
        let max_executor = configured_limit(
            "CHADEX_GLOBAL_EXECUTOR_CONCURRENCY",
            DEFAULT_GLOBAL_EXECUTOR_CONCURRENCY,
        );
        Self {
            task_slots: Arc::new(Semaphore::new(max_tasks)),
            executor_slots: Arc::new(Semaphore::new(max_executor)),
            max_tasks,
            max_executor,
        }
    }
}

impl ExecutionScheduler {
    pub(crate) async fn acquire_task(&self) -> (OwnedSemaphorePermit, u64) {
        let started = Instant::now();
        let permit = self
            .task_slots
            .clone()
            .acquire_owned()
            .await
            .expect("task scheduler semaphore must remain open");
        (permit, started.elapsed().as_millis() as u64)
    }

    pub(crate) async fn acquire_executor(&self) -> OwnedSemaphorePermit {
        self.executor_slots
            .clone()
            .acquire_owned()
            .await
            .expect("executor scheduler semaphore must remain open")
    }

    pub(crate) const fn limits(&self) -> (usize, usize) {
        (self.max_tasks, self.max_executor)
    }
}

fn configured_limit(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1..=MAX_CONFIGURED_CONCURRENCY).contains(value))
        .unwrap_or(default)
}

/// Select isolation only after the pre-plan/package decision is available.
pub(crate) fn choose_isolation_mode(
    requires_mutation_boundary: bool,
    complexity_band: &str,
    package_count: usize,
    packages_independent: bool,
) -> IsolationMode {
    if !requires_mutation_boundary {
        return IsolationMode::None;
    }
    if package_count > 1
        && packages_independent
        && matches!(complexity_band, "large" | "very_large")
    {
        IsolationMode::Package
    } else {
        IsolationMode::Task
    }
}

pub(crate) fn package_paths_are_independent(packages: &[Vec<String>]) -> bool {
    // An empty dependency set means "unknown", not "disjoint".  Treating
    // unknown process/build effects as independent could merge generated
    // artifacts or other side effects during integration.
    packages.iter().all(|package| !package.is_empty())
        && packages.iter().enumerate().all(|(index, package)| {
            let paths = package.iter().collect::<std::collections::HashSet<_>>();
            packages[index + 1..]
                .iter()
                .all(|other| other.iter().all(|path| !paths.contains(path)))
        })
}

/// Lexically constrain all task-relative paths before they enter a nested
/// tool. Runner performs the authoritative canonical/symlink check against the
/// registered workspace root; this guard prevents obvious escape attempts from
/// crossing the server-side workspace boundary first.
pub(crate) fn workspace_relative_path(path: &str) -> Result<String, String> {
    if path.is_empty() || path == "." {
        return Ok(".".to_string());
    }
    if path.contains('\0') {
        return Err("workspace_path_contains_nul".to_string());
    }
    let path_ref = Path::new(path);
    if path_ref.is_absolute() {
        return Err("workspace_path_must_be_relative".to_string());
    }
    let mut output = Vec::new();
    for component in path_ref.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => output.push(part.to_string_lossy().into_owned()),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("workspace_path_escapes_root".to_string());
            }
        }
    }
    if output.is_empty() {
        Ok(".".to_string())
    } else {
        Ok(output.join("/"))
    }
}

const WORKING_TREE_SCRIPT: &str = r#"
set -eu
unset GIT_INDEX_FILE
head_sha="$(git rev-parse HEAD)"
head_tree="$(git rev-parse HEAD^{tree})"
index_tree="$(git write-tree)"
# Clean repositories are the latency-sensitive common case. Avoid rebuilding
# the complete effective tree through a temporary index unless tracked or
# non-ignored untracked content actually differs from HEAD.
if [ "$index_tree" = "$head_tree" ] && git diff --quiet -- && [ -z "$(git ls-files --others --exclude-standard)" ]; then
  working_tree="$head_tree"
else
  index_path="$(mktemp "${TMPDIR:-/tmp}/chadex-tree-index.XXXXXX")"
  rm -f "$index_path"
  trap 'rm -f "$index_path"' EXIT HUP INT TERM
  export GIT_INDEX_FILE="$index_path"
  git read-tree HEAD
  git add -A -- .
  working_tree="$(git write-tree)"
fi
printf 'head=%s\nhead_tree=%s\nworking_tree=%s\nindex_tree=%s\n' "$head_sha" "$head_tree" "$working_tree" "$index_tree"
"#;

fn source_requires_dirty_snapshot(
    head_tree_sha: &str,
    working_tree_sha: &str,
    index_tree_sha: &str,
) -> bool {
    working_tree_sha != head_tree_sha || index_tree_sha != head_tree_sha
}

fn valid_git_object(value: &str) -> bool {
    matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn prepare_error(result: &super::ToolResult) -> String {
    result
        .error
        .clone()
        .or_else(|| {
            result
                .output
                .get("error_code")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "execution_workspace_prepare_failed".to_string())
}

fn prepare_outcome_indeterminate(result: &super::ToolResult) -> bool {
    !result.success
        && ["error_code", "error_kind"].into_iter().any(|field| {
            result.output.get(field).and_then(Value::as_str) == Some("operation_indeterminate")
        })
}

impl ToolRuntime {
    async fn capture_execution_state(
        &self,
        project: &str,
    ) -> Result<(String, String, String, String), String> {
        let output = self
            .run_project_internal_posix_script_capture(
                project,
                WORKING_TREE_SCRIPT.to_string(),
                30,
                Some(".".to_string()),
            )
            .await?;
        let mut head = None;
        let mut head_tree = None;
        let mut working_tree = None;
        let mut index_tree = None;
        for line in output.stdout.lines() {
            if let Some(value) = line.strip_prefix("head=") {
                if valid_git_object(value.trim()) {
                    head = Some(value.trim().to_ascii_lowercase());
                }
            } else if let Some(value) = line.strip_prefix("head_tree=") {
                if valid_git_object(value.trim()) {
                    head_tree = Some(value.trim().to_ascii_lowercase());
                }
            } else if let Some(value) = line.strip_prefix("working_tree=") {
                if valid_git_object(value.trim()) {
                    working_tree = Some(value.trim().to_ascii_lowercase());
                }
            } else if let Some(value) = line.strip_prefix("index_tree=") {
                if valid_git_object(value.trim()) {
                    index_tree = Some(value.trim().to_ascii_lowercase());
                }
            }
        }
        match (head, head_tree, working_tree, index_tree) {
            (Some(head), Some(head_tree), Some(working_tree), Some(index_tree)) => {
                Ok((head, head_tree, working_tree, index_tree))
            }
            _ => Err(output
                .error
                .unwrap_or_else(|| "execution_tree_snapshot_failed".to_string())),
        }
    }

    pub(crate) async fn prepare_execution_workspace(
        &self,
        resolved: &ResolvedProject,
        task_id: &str,
        mode: IsolationMode,
        base_sha: Option<String>,
        auth: Option<&AuthContext>,
    ) -> Result<ExecutionWorkspace, String> {
        let source_project = resolved.resolved_id.clone();
        if mode == IsolationMode::None {
            return Ok(ExecutionWorkspace::read_only(
                task_id.to_string(),
                source_project,
                resolved.config.path.clone(),
            ));
        }
        let snapshot_started = Instant::now();
        let (source_head_sha, source_head_tree_sha, source_tree_sha, source_index_tree_sha) =
            self.capture_execution_state(&source_project).await?;
        // Snapshot and worktree creation are disjoint phases. Counting creation
        // twice inflates the reported package_workspace_component_sum_ms.
        let snapshot_ms = snapshot_started.elapsed().as_millis() as u64;
        let source_is_clean = !source_requires_dirty_snapshot(
            &source_head_tree_sha,
            &source_tree_sha,
            &source_index_tree_sha,
        );
        let requested_base = base_sha.or_else(|| source_is_clean.then(|| source_head_sha.clone()));
        let include_dirty_snapshot = !source_is_clean && requested_base.is_none();
        let operation_id = uuid::Uuid::new_v4().to_string();
        let prepare_intent = WorkspacePrepareIntent {
            task_id: task_id.to_string(),
            mode,
            source_project: source_project.clone(),
            client_id: resolved.config.client_id.clone(),
            source_path: resolved.config.path.clone(),
            operation_id: operation_id.clone(),
            base_ref: requested_base.clone(),
            include_dirty_snapshot,
            source_head_sha: Some(source_head_sha.clone()),
            source_tree_sha: Some(source_tree_sha.clone()),
            source_index_tree_sha: Some(source_index_tree_sha.clone()),
        };
        persist_workspace_prepare_intent(&prepare_intent).map_err(|error| {
            format!("execution_workspace_prepare_intent_persist_failed:{error}")
        })?;
        let operation_started = Instant::now();
        let output = self
            .prepare_managed_worktree_with_options(
                resolved.config.client_id.clone(),
                resolved.config.path.clone(),
                requested_base,
                operation_id.clone(),
                None,
                include_dirty_snapshot,
                auth,
            )
            .await;
        if !output.success {
            if !prepare_outcome_indeterminate(&output) {
                remove_workspace_prepare_intent(&prepare_intent).map_err(|error| {
                    format!("execution_workspace_prepare_intent_remove_failed:{error}")
                })?;
            }
            return Err(prepare_error(&output));
        }
        let execution_project = output
            .output
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "execution_workspace_missing_project_id".to_string())?
            .to_string();
        let root = output
            .output
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "execution_workspace_missing_root".to_string())?
            .to_string();
        let registered_revision = output
            .output
            .get("revision")
            .and_then(Value::as_str)
            .map(str::to_string);
        let base_sha = output
            .output
            .get("base_sha")
            .and_then(Value::as_str)
            .filter(|value| valid_git_object(value))
            .map(|value| value.to_ascii_lowercase());
        let mut timings = WorkspaceTimings::default();
        timings.snapshot_ms = snapshot_ms;
        timings.worktree_creation_ms = operation_started.elapsed().as_millis() as u64;
        let workspace = ExecutionWorkspace {
            task_id: task_id.to_string(),
            mode,
            source_project,
            execution_project,
            root,
            base_sha,
            source_head_sha: Some(source_head_sha),
            source_tree_sha: Some(source_tree_sha),
            source_index_tree_sha: Some(source_index_tree_sha),
            registered_revision,
            prepare_operation_id: Some(operation_id.clone()),
            cleanup_operation_id: None,
            cleanup_phase: WorkspaceCleanupPhase::None,
            state: WorkspaceState::Active,
            preserve_reason: None,
            timings,
        };
        persist_workspace_manifest(&workspace)
            .map_err(|error| format!("execution_workspace_manifest_persist_failed:{error}"))?;
        remove_workspace_prepare_intent(&prepare_intent)
            .map_err(|error| format!("execution_workspace_prepare_intent_remove_failed:{error}"))?;
        Ok(workspace)
    }

    /// Create another package/integration root from the same anonymous base
    /// commit. The source tree is not re-snapshotted and the original project
    /// remains untouched.
    pub(crate) async fn prepare_derived_execution_workspace(
        &self,
        resolved: &ResolvedProject,
        task_id: &str,
        mode: IsolationMode,
        base_sha: &str,
        source_head_sha: Option<String>,
        source_tree_sha: Option<String>,
        source_index_tree_sha: Option<String>,
        auth: Option<&AuthContext>,
    ) -> Result<ExecutionWorkspace, String> {
        if !valid_git_object(base_sha) {
            return Err("execution_workspace_invalid_base_sha".to_string());
        }
        let operation_id = uuid::Uuid::new_v4().to_string();
        let prepare_intent = WorkspacePrepareIntent {
            task_id: task_id.to_string(),
            mode,
            source_project: resolved.resolved_id.clone(),
            client_id: resolved.config.client_id.clone(),
            source_path: resolved.config.path.clone(),
            operation_id: operation_id.clone(),
            base_ref: Some(base_sha.to_string()),
            include_dirty_snapshot: false,
            source_head_sha: source_head_sha.clone(),
            source_tree_sha: source_tree_sha.clone(),
            source_index_tree_sha: source_index_tree_sha.clone(),
        };
        persist_workspace_prepare_intent(&prepare_intent).map_err(|error| {
            format!("execution_workspace_prepare_intent_persist_failed:{error}")
        })?;
        let started = Instant::now();
        let output = self
            .prepare_managed_worktree_with_options(
                resolved.config.client_id.clone(),
                resolved.config.path.clone(),
                Some(base_sha.to_string()),
                operation_id.clone(),
                None,
                false,
                auth,
            )
            .await;
        if !output.success {
            if !prepare_outcome_indeterminate(&output) {
                remove_workspace_prepare_intent(&prepare_intent).map_err(|error| {
                    format!("execution_workspace_prepare_intent_remove_failed:{error}")
                })?;
            }
            return Err(prepare_error(&output));
        }
        let execution_project = output
            .output
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "execution_workspace_missing_project_id".to_string())?
            .to_string();
        let root = output
            .output
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "execution_workspace_missing_root".to_string())?
            .to_string();
        let workspace = ExecutionWorkspace {
            task_id: task_id.to_string(),
            mode,
            source_project: resolved.resolved_id.clone(),
            execution_project,
            root,
            base_sha: Some(base_sha.to_ascii_lowercase()),
            source_head_sha,
            source_tree_sha,
            source_index_tree_sha,
            registered_revision: output
                .output
                .get("revision")
                .and_then(Value::as_str)
                .map(str::to_string),
            prepare_operation_id: Some(operation_id.clone()),
            cleanup_operation_id: None,
            cleanup_phase: WorkspaceCleanupPhase::None,
            state: WorkspaceState::Active,
            preserve_reason: None,
            timings: WorkspaceTimings {
                worktree_creation_ms: started.elapsed().as_millis() as u64,
                ..WorkspaceTimings::default()
            },
        };
        persist_workspace_manifest(&workspace)
            .map_err(|error| format!("execution_workspace_manifest_persist_failed:{error}"))?;
        remove_workspace_prepare_intent(&prepare_intent)
            .map_err(|error| format!("execution_workspace_prepare_intent_remove_failed:{error}"))?;
        Ok(workspace)
    }

    pub(crate) async fn reconcile_workspace_prepare_intent(
        &self,
        intent: &WorkspacePrepareIntent,
        auth: Option<&AuthContext>,
    ) -> Result<ExecutionWorkspace, String> {
        let output = self
            .prepare_managed_worktree_with_options(
                intent.client_id.clone(),
                intent.source_path.clone(),
                intent.base_ref.clone(),
                intent.operation_id.clone(),
                None,
                intent.include_dirty_snapshot,
                auth,
            )
            .await;
        if !output.success {
            return Err(prepare_error(&output));
        }
        let execution_project = output
            .output
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| "execution_workspace_missing_project_id".to_string())?
            .to_string();
        let root = output
            .output
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| "execution_workspace_missing_root".to_string())?
            .to_string();
        let base_sha = output
            .output
            .get("base_sha")
            .and_then(Value::as_str)
            .filter(|value| valid_git_object(value))
            .map(|value| value.to_ascii_lowercase())
            .or_else(|| intent.base_ref.clone());
        let workspace = ExecutionWorkspace {
            task_id: intent.task_id.clone(),
            mode: intent.mode,
            source_project: intent.source_project.clone(),
            execution_project,
            root,
            base_sha,
            source_head_sha: intent.source_head_sha.clone(),
            source_tree_sha: intent.source_tree_sha.clone(),
            source_index_tree_sha: intent.source_index_tree_sha.clone(),
            registered_revision: output
                .output
                .get("revision")
                .and_then(Value::as_str)
                .map(str::to_string),
            prepare_operation_id: Some(intent.operation_id.clone()),
            cleanup_operation_id: None,
            cleanup_phase: WorkspaceCleanupPhase::None,
            state: WorkspaceState::Preserved,
            preserve_reason: Some("runtime_interrupted_after_workspace_prepare".to_string()),
            timings: WorkspaceTimings::default(),
        };
        persist_workspace_manifest(&workspace)
            .map_err(|error| format!("execution_workspace_manifest_persist_failed:{error}"))?;
        remove_workspace_prepare_intent(intent)
            .map_err(|error| format!("execution_workspace_prepare_intent_remove_failed:{error}"))?;
        Ok(workspace)
    }

    pub(crate) async fn commit_execution_workspace(
        &self,
        workspace: &ExecutionWorkspace,
    ) -> Result<Option<String>, String> {
        if workspace.mode == IsolationMode::None {
            return Ok(None);
        }
        let output = self
            .run_project_internal_posix_script_capture(
                &workspace.execution_project,
                r#"
set -eu
git add -A -- .
if git diff --cached --quiet --exit-code; then
  printf 'NO_CHANGES\n'
else
  git -c user.name=Chadex -c user.email=chadex@localhost commit --no-gpg-sign -m 'Chadex isolated execution result' >/dev/null
  git rev-parse HEAD
fi
"#
                .to_string(),
                60,
                Some(".".to_string()),
            )
            .await?;
        if output.exit_code != Some(0) {
            return Err(output
                .error
                .unwrap_or_else(|| "execution_workspace_commit_failed".to_string()));
        }
        let result = output
            .stdout
            .lines()
            .rev()
            .find(|line| valid_git_object(line.trim()))
            .map(|line| line.trim().to_ascii_lowercase());
        Ok(result)
    }

    pub(crate) async fn workspace_head_sha(
        &self,
        workspace: &ExecutionWorkspace,
    ) -> Result<String, String> {
        let output = self
            .run_project_internal_posix_script_capture(
                &workspace.execution_project,
                "set -eu\ngit rev-parse HEAD\n".to_string(),
                30,
                Some(".".to_string()),
            )
            .await?;
        output
            .stdout
            .lines()
            .rev()
            .find(|line| valid_git_object(line.trim()))
            .map(|line| line.trim().to_ascii_lowercase())
            .ok_or_else(|| {
                output
                    .error
                    .unwrap_or_else(|| "execution_workspace_head_failed".to_string())
            })
    }

    pub(crate) async fn cherry_pick_execution_commit(
        &self,
        workspace: &ExecutionWorkspace,
        commit_sha: &str,
    ) -> Result<(), String> {
        if !valid_git_object(commit_sha) {
            return Err("execution_workspace_invalid_commit_sha".to_string());
        }
        let script = format!(
            "set -eu\ngit cherry-pick --no-edit --no-gpg-sign -- {}\n",
            shell_quote(commit_sha)
        );
        let output = self
            .run_project_internal_posix_script_capture(
                &workspace.execution_project,
                script,
                120,
                Some(".".to_string()),
            )
            .await?;
        if output.exit_code == Some(0) {
            Ok(())
        } else {
            Err(output
                .error
                .unwrap_or_else(|| "execution_workspace_integration_conflict".to_string()))
        }
    }

    async fn execution_result_write_paths(
        &self,
        project: &str,
        base_sha: &str,
        result_sha: &str,
    ) -> Result<Vec<String>, String> {
        if !valid_git_object(base_sha) || !valid_git_object(result_sha) {
            return Err("execution_workspace_invalid_git_object".to_string());
        }
        // Encode the NUL-delimited Git path list as hex so filenames containing
        // whitespace/newlines remain unambiguous across the Runner text protocol.
        let script = format!(
            "set -eu\ngit diff --name-only -z --no-renames {} {} | od -An -v -tx1\n",
            shell_quote(base_sha),
            shell_quote(result_sha),
        );
        let output = self
            .run_project_internal_posix_script_capture(project, script, 30, Some(".".to_string()))
            .await?;
        if output.exit_code != Some(0) {
            return Err(output
                .error
                .unwrap_or_else(|| "execution_workspace_write_set_failed".to_string()));
        }
        let mut bytes = Vec::new();
        for token in output.stdout.split_whitespace() {
            let byte = u8::from_str_radix(token, 16)
                .map_err(|_| "execution_workspace_write_set_decode_failed".to_string())?;
            bytes.push(byte);
        }
        let mut paths = Vec::new();
        for raw in bytes.split(|byte| *byte == 0).filter(|raw| !raw.is_empty()) {
            let path = std::str::from_utf8(raw)
                .map_err(|_| "execution_workspace_non_utf8_write_path".to_string())?;
            let normalized = workspace_relative_path(path)?;
            if !paths.contains(&normalized) {
                paths.push(normalized);
            }
        }
        Ok(paths)
    }

    /// Apply a result when no user/source change overlaps the task write-set.
    /// Unrelated working-tree/index/HEAD changes are intentionally allowed.
    /// This is the normal concurrent apply-back path; true write-set overlap
    /// falls back to isolated three-way reconciliation in the task executor.
    pub(crate) async fn apply_execution_result_if_write_set_unchanged(
        &self,
        workspace: &ExecutionWorkspace,
        result_sha: &str,
    ) -> Result<(), String> {
        if workspace.mode == IsolationMode::None {
            return Ok(());
        }
        let Some(base_sha) = workspace.base_sha.as_deref() else {
            return Err("execution_workspace_missing_base_sha".to_string());
        };
        let Some(source_tree_sha) = workspace.source_tree_sha.as_deref() else {
            return Err("execution_workspace_missing_source_tree_sha".to_string());
        };
        let Some(source_index_tree_sha) = workspace.source_index_tree_sha.as_deref() else {
            return Err("execution_workspace_missing_source_index_tree_sha".to_string());
        };
        if !valid_git_object(base_sha)
            || !valid_git_object(source_tree_sha)
            || !valid_git_object(source_index_tree_sha)
            || !valid_git_object(result_sha)
        {
            return Err("execution_workspace_invalid_git_object".to_string());
        }
        let write_paths = self
            .execution_result_write_paths(&workspace.execution_project, base_sha, result_sha)
            .await?;
        if write_paths.is_empty() {
            return Ok(());
        }
        let path_args = write_paths
            .iter()
            .map(|path| shell_quote(path))
            .collect::<Vec<_>>()
            .join(" ");
        let script = format!(
            r#"
set -eu
patch_path="$(mktemp "${{TMPDIR:-/tmp}}/chadex-apply.XXXXXX")"
index_path="$(mktemp "${{TMPDIR:-/tmp}}/chadex-apply-index.XXXXXX")"
rm -f "$index_path"
trap 'rm -f "$patch_path" "$index_path"' EXIT HUP INT TERM
current_index_tree="$(git write-tree)"
GIT_INDEX_FILE="$index_path" git read-tree HEAD
GIT_INDEX_FILE="$index_path" git add -A -- .
current_tree="$(GIT_INDEX_FILE="$index_path" git write-tree)"
if ! git diff --quiet {source_tree} "$current_tree" -- {paths}; then
  printf 'execution_workspace_source_overlap\n'
  exit 75
fi
if ! git diff --quiet {source_index_tree} "$current_index_tree" -- {paths}; then
  printf 'execution_workspace_source_overlap\n'
  exit 75
fi
git diff --binary {base} {result} > "$patch_path"
if [ -s "$patch_path" ]; then
  git apply --check --binary --whitespace=nowarn "$patch_path"
  git apply --binary --whitespace=nowarn "$patch_path"
fi
"#,
            source_tree = shell_quote(source_tree_sha),
            source_index_tree = shell_quote(source_index_tree_sha),
            paths = path_args,
            base = shell_quote(base_sha),
            result = shell_quote(result_sha),
        );
        let output = self
            .run_project_internal_posix_script_capture(
                &workspace.source_project,
                script,
                60,
                Some(".".to_string()),
            )
            .await?;
        if output.exit_code == Some(0) {
            Ok(())
        } else if output
            .stdout
            .lines()
            .any(|line| line.trim() == "execution_workspace_source_overlap")
        {
            Err("execution_workspace_source_overlap".to_string())
        } else {
            Err(output
                .error
                .unwrap_or_else(|| "execution_workspace_apply_failed".to_string()))
        }
    }

    pub(crate) async fn apply_execution_result_if_unchanged(
        &self,
        workspace: &ExecutionWorkspace,
        result_sha: &str,
    ) -> Result<(), String> {
        if workspace.mode == IsolationMode::None {
            return Ok(());
        }
        let Some(base_sha) = workspace.base_sha.as_deref() else {
            return Err("execution_workspace_missing_base_sha".to_string());
        };
        let Some(source_head_sha) = workspace.source_head_sha.as_deref() else {
            return Err("execution_workspace_missing_source_head_sha".to_string());
        };
        let Some(source_tree_sha) = workspace.source_tree_sha.as_deref() else {
            return Err("execution_workspace_missing_source_tree_sha".to_string());
        };
        let Some(source_index_tree_sha) = workspace.source_index_tree_sha.as_deref() else {
            return Err("execution_workspace_missing_source_index_tree_sha".to_string());
        };
        if !valid_git_object(base_sha)
            || !valid_git_object(source_head_sha)
            || !valid_git_object(source_tree_sha)
            || !valid_git_object(source_index_tree_sha)
            || !valid_git_object(result_sha)
        {
            return Err("execution_workspace_invalid_git_object".to_string());
        }
        let (current_head, _current_head_tree, current_tree, current_index_tree) = self
            .capture_execution_state(&workspace.source_project)
            .await?;
        if current_head != source_head_sha
            || current_tree != source_tree_sha
            || current_index_tree != source_index_tree_sha
        {
            return Err("execution_workspace_source_changed".to_string());
        }
        let script = format!(
            r#"
set -eu
patch_path="$(mktemp "${{TMPDIR:-/tmp}}/chadex-apply.XXXXXX")"
index_path="$(mktemp "${{TMPDIR:-/tmp}}/chadex-apply-index.XXXXXX")"
rm -f "$index_path"
trap 'rm -f "$patch_path" "$index_path"' EXIT HUP INT TERM
current_head="$(git rev-parse HEAD)"
current_index_tree="$(git write-tree)"
GIT_INDEX_FILE="$index_path" git read-tree HEAD
GIT_INDEX_FILE="$index_path" git add -A -- .
current_tree="$(GIT_INDEX_FILE="$index_path" git write-tree)"
if [ "$current_head" != {source_head} ] || [ "$current_tree" != {source_tree} ] || [ "$current_index_tree" != {source_index_tree} ]; then
  printf 'execution_workspace_source_changed\n'
  exit 75
fi
git diff --binary {base} {result} > "$patch_path"
if [ -s "$patch_path" ]; then
  git apply --check --binary --whitespace=nowarn "$patch_path"
  git apply --binary --whitespace=nowarn "$patch_path"
fi
"#,
            base = shell_quote(base_sha),
            result = shell_quote(result_sha),
            source_head = shell_quote(source_head_sha),
            source_tree = shell_quote(source_tree_sha),
            source_index_tree = shell_quote(source_index_tree_sha),
        );
        let output = self
            .run_project_internal_posix_script_capture(
                &workspace.source_project,
                script,
                60,
                Some(".".to_string()),
            )
            .await?;
        if output.exit_code == Some(0) {
            Ok(())
        } else if output
            .stdout
            .lines()
            .any(|line| line.trim() == "execution_workspace_source_changed")
        {
            Err("execution_workspace_source_changed".to_string())
        } else {
            Err(output
                .error
                .unwrap_or_else(|| "execution_workspace_apply_failed".to_string()))
        }
    }

    pub(crate) async fn apply_execution_result_three_way(
        &self,
        workspace: &ExecutionWorkspace,
        base_sha: &str,
        result_sha: &str,
    ) -> Result<(), String> {
        if workspace.mode == IsolationMode::None
            || !valid_git_object(base_sha)
            || !valid_git_object(result_sha)
        {
            return Err("execution_workspace_invalid_reconciliation_input".to_string());
        }
        let script = format!(
            r#"
set -eu
patch_path="$(mktemp "${{TMPDIR:-/tmp}}/chadex-reconcile.XXXXXX")"
trap 'rm -f "$patch_path"' EXIT HUP INT TERM
git diff --binary {base} {result} > "$patch_path"
if [ -s "$patch_path" ]; then
  git apply --3way --binary --whitespace=nowarn "$patch_path"
fi
"#,
            base = shell_quote(base_sha),
            result = shell_quote(result_sha),
        );
        let output = self
            .run_project_internal_posix_script_capture(
                &workspace.execution_project,
                script,
                120,
                Some(".".to_string()),
            )
            .await?;
        if output.exit_code == Some(0) {
            Ok(())
        } else {
            Err(output
                .error
                .unwrap_or_else(|| "execution_workspace_reconciliation_conflict".to_string()))
        }
    }

    pub(crate) async fn cleanup_execution_workspace(
        &self,
        workspace: &mut ExecutionWorkspace,
        auth: Option<&AuthContext>,
    ) -> Result<(), String> {
        if workspace.mode == IsolationMode::None {
            workspace.state = WorkspaceState::Cleaned;
            return Ok(());
        }
        // A path or manifest alone is not authority to delete. Require the
        // original Runner registration fence and a distinct execution identity.
        if workspace
            .registered_revision
            .as_deref()
            .is_none_or(str::is_empty)
            || workspace.execution_project == workspace.source_project
            || workspace.task_id.is_empty()
            || !std::path::Path::new(&workspace.root).is_absolute()
        {
            return Err("execution_workspace_ownership_unknown".into());
        }
        let _lease = acquire_workspace_cleanup_lease(workspace)?;
        if workspace.cleanup_phase == WorkspaceCleanupPhase::Complete {
            workspace.state = WorkspaceState::Cleaned;
            return Ok(());
        }

        if workspace.cleanup_phase == WorkspaceCleanupPhase::None {
            let registered = self
                .resolve_project_for_auth(&workspace.execution_project, auth)
                .await
                .map_err(|_| "execution_workspace_ownership_unknown".to_string())?;
            if registered.path != workspace.root {
                return Err("execution_workspace_ownership_mismatch".into());
            }
            workspace.cleanup_operation_id = Some(uuid::Uuid::new_v4().to_string());
            workspace.cleanup_phase = WorkspaceCleanupPhase::IntentPersisted;
            persist_workspace_manifest(workspace).map_err(|error| {
                format!("execution_workspace_cleanup_intent_persist_failed:{error}")
            })?;
        }

        if workspace.cleanup_phase == WorkspaceCleanupPhase::IntentPersisted {
            match self
                .resolve_project_for_auth(&workspace.execution_project, auth)
                .await
            {
                Ok(registered) => {
                    if registered.path != workspace.root {
                        return Err("execution_workspace_ownership_mismatch".into());
                    }
                    let revision = workspace
                        .registered_revision
                        .as_deref()
                        .ok_or_else(|| "execution_workspace_ownership_unknown".to_string())?;
                    let result = self
                        .unregister_project(
                            workspace.execution_project.clone(),
                            revision.to_string(),
                            auth,
                        )
                        .await;
                    if !result.success {
                        return Err(prepare_error(&result));
                    }
                }
                Err(error) if error.kind == ProjectResolverErrorKind::UnknownProject => {
                    // The durable cleanup intent proves that this exact registration
                    // was verified before unregister. Missing now may therefore be
                    // a crash immediately after that already-authorized effect.
                }
                Err(_) => return Err("execution_workspace_ownership_unknown".into()),
            }
            workspace.cleanup_phase = WorkspaceCleanupPhase::ProjectUnregistered;
            persist_workspace_manifest(workspace).map_err(|error| {
                format!("execution_workspace_cleanup_unregister_persist_failed:{error}")
            })?;
        }

        if workspace.cleanup_phase == WorkspaceCleanupPhase::ProjectUnregistered {
            let script = format!(
                "set -u\ntarget={}\nif git worktree remove --force -- \"$target\"; then\n  exit 0\nfi\n# Idempotent replay is safe only when the old worktree path is already gone.\n# If a path still exists but Git no longer recognizes it, fail closed rather\n# than deleting a possibly reused directory by path alone.\nif [ ! -e \"$target\" ]; then\n  git worktree prune >/dev/null 2>&1 || true\n  exit 0\nfi\nexit 1\n",
                shell_quote(&workspace.root)
            );
            let output = self
                .run_project_internal_posix_script_capture(
                    &workspace.source_project,
                    script,
                    60,
                    Some(".".to_string()),
                )
                .await?;
            if output.exit_code != Some(0) {
                return Err(output
                    .error
                    .unwrap_or_else(|| "execution_workspace_cleanup_failed".to_string()));
            }
            workspace.cleanup_phase = WorkspaceCleanupPhase::WorktreeRemoved;
            persist_workspace_manifest(workspace).map_err(|error| {
                format!("execution_workspace_cleanup_remove_persist_failed:{error}")
            })?;
        }

        if workspace.cleanup_phase == WorkspaceCleanupPhase::WorktreeRemoved {
            workspace.state = WorkspaceState::Cleaned;
            workspace.cleanup_phase = WorkspaceCleanupPhase::Complete;
            persist_workspace_manifest(workspace).map_err(|error| {
                format!("execution_workspace_cleanup_complete_persist_failed:{error}")
            })?;
        }
        Ok(())
    }
}

pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn now_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn task_state_root() -> Option<PathBuf> {
    #[cfg(test)]
    {
        None
    }
    #[cfg(not(test))]
    {
        std::env::var_os("CHADEX_TASK_STATE_DIR").map(PathBuf::from)
    }
}

fn private_state_directory(root: &Path, name: &str) -> Result<PathBuf, String> {
    if std::fs::symlink_metadata(root)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err("workspace_state_root_is_symlink".to_string());
    }
    std::fs::create_dir_all(root)
        .map_err(|error| format!("workspace_state_root_create_failed:{error}"))?;
    let directory = root.join(name);
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("workspace_state_directory_create_failed:{error}"))?;
    let metadata = std::fs::symlink_metadata(&directory)
        .map_err(|error| format!("workspace_state_directory_metadata_failed:{error}"))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("workspace_state_directory_unsafe".to_string());
    }
    #[cfg(unix)]
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("workspace_state_directory_permissions_failed:{error}"))?;
    Ok(directory)
}

fn atomic_private_json_write(directory: &Path, path: &Path, value: &Value) -> Result<(), String> {
    if std::fs::symlink_metadata(path)
        .ok()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err("workspace_state_target_is_symlink".to_string());
    }
    let bytes = serde_json::to_vec(value)
        .map_err(|error| format!("workspace_state_encode_failed:{error}"))?;
    let temporary_path = directory.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("workspace"),
        uuid::Uuid::new_v4().simple()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&temporary_path)
        .map_err(|error| format!("workspace_state_open_failed:{error}"))?;
    #[cfg(unix)]
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("workspace_state_permissions_failed:{error}"))?;
    if let Err(error) = file.write_all(&bytes).and_then(|_| file.sync_all()) {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(format!("workspace_state_write_failed:{error}"));
    }
    drop(file);
    if let Err(error) = std::fs::rename(&temporary_path, path) {
        let _ = std::fs::remove_file(&temporary_path);
        return Err(format!("workspace_state_rename_failed:{error}"));
    }
    std::fs::File::open(directory)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| format!("workspace_state_directory_sync_failed:{error}"))?;
    Ok(())
}

fn remove_private_state_file(directory: &Path, path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => {
            std::fs::File::open(directory)
                .and_then(|directory| directory.sync_all())
                .map_err(|error| format!("workspace_state_directory_sync_failed:{error}"))?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("workspace_state_remove_failed:{error}")),
    }
}

fn workspace_prepare_intent_stem(intent: &WorkspacePrepareIntent) -> String {
    let digest = format!("{:x}", Sha256::digest(intent.operation_id.as_bytes()));
    format!(
        "{}-{}-{}",
        intent.task_id,
        intent.mode.as_str(),
        &digest[..16]
    )
}

pub(crate) fn persist_workspace_prepare_intent(
    intent: &WorkspacePrepareIntent,
) -> Result<(), String> {
    let Some(root) = task_state_root() else {
        return Ok(());
    };
    persist_workspace_prepare_intent_at(&root, intent)
}

fn persist_workspace_prepare_intent_at(
    root: &Path,
    intent: &WorkspacePrepareIntent,
) -> Result<(), String> {
    let directory = private_state_directory(root, "workspace-intents")?;
    let path = directory.join(format!("{}.json", workspace_prepare_intent_stem(intent)));
    atomic_private_json_write(&directory, &path, &intent.manifest())
}

pub(crate) fn remove_workspace_prepare_intent(
    intent: &WorkspacePrepareIntent,
) -> Result<(), String> {
    let Some(root) = task_state_root() else {
        return Ok(());
    };
    let directory = private_state_directory(&root, "workspace-intents")?;
    let path = directory.join(format!("{}.json", workspace_prepare_intent_stem(intent)));
    remove_private_state_file(&directory, &path)
}

pub(crate) fn load_workspace_prepare_intents() -> Vec<WorkspacePrepareIntent> {
    let Some(root) = task_state_root() else {
        return Vec::new();
    };
    load_workspace_prepare_intents_at(&root)
}

fn load_workspace_prepare_intents_at(root: &Path) -> Vec<WorkspacePrepareIntent> {
    let directory = root.join("workspace-intents");
    let Ok(metadata) = std::fs::symlink_metadata(&directory) else {
        return Vec::new();
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut intents = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                return None;
            }
            let metadata = std::fs::symlink_metadata(&path).ok()?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return None;
            }
            let value = serde_json::from_slice::<Value>(&std::fs::read(path).ok()?).ok()?;
            workspace_prepare_intent_from_manifest(&value).ok()
        })
        .collect::<Vec<_>>();
    intents.sort_by(|left, right| left.operation_id.cmp(&right.operation_id));
    intents
}

fn workspace_prepare_intent_from_manifest(value: &Value) -> Result<WorkspacePrepareIntent, String> {
    let required = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("workspace_prepare_intent_missing_{key}"))
    };
    let mode = value
        .get("mode")
        .and_then(Value::as_str)
        .and_then(IsolationMode::parse)
        .ok_or_else(|| "workspace_prepare_intent_invalid_mode".to_string())?;
    Ok(WorkspacePrepareIntent {
        task_id: required("task_id")?,
        mode,
        source_project: required("source_project")?,
        client_id: required("client_id")?,
        source_path: required("source_path")?,
        operation_id: required("operation_id")?,
        base_ref: value
            .get("base_ref")
            .and_then(Value::as_str)
            .map(str::to_string),
        include_dirty_snapshot: value
            .get("include_dirty_snapshot")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        source_head_sha: value
            .get("source_head_sha")
            .and_then(Value::as_str)
            .map(str::to_string),
        source_tree_sha: value
            .get("source_tree_sha")
            .and_then(Value::as_str)
            .map(str::to_string),
        source_index_tree_sha: value
            .get("source_index_tree_sha")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

pub(crate) fn load_workspace_manifests() -> Vec<(PathBuf, Value)> {
    let Some(root) = task_state_root() else {
        return Vec::new();
    };
    let directory = root.join("workspaces");
    let Ok(metadata) = std::fs::symlink_metadata(&directory) else {
        return Vec::new();
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(&directory) else {
        return Vec::new();
    };
    let mut manifests = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        manifests.push((path, value));
    }
    manifests.sort_by(|left, right| left.0.cmp(&right.0));
    manifests
}

pub(crate) fn workspace_from_manifest(value: &Value) -> Result<ExecutionWorkspace, String> {
    let string = |key: &str| {
        value
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("workspace_manifest_missing_{key}"))
    };
    let optional_string = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_string);
    let mode = IsolationMode::parse(
        value
            .get("mode")
            .and_then(Value::as_str)
            .ok_or_else(|| "workspace_manifest_missing_mode".to_string())?,
    )
    .ok_or_else(|| "workspace_manifest_invalid_mode".to_string())?;
    let state = WorkspaceState::parse(
        value
            .get("state")
            .and_then(Value::as_str)
            .ok_or_else(|| "workspace_manifest_missing_state".to_string())?,
    )
    .ok_or_else(|| "workspace_manifest_invalid_state".to_string())?;
    let timings = value.get("timings").unwrap_or(&Value::Null);
    let timing = |key: &str| timings.get(key).and_then(Value::as_u64).unwrap_or(0);
    Ok(ExecutionWorkspace {
        task_id: string("task_id")?,
        mode,
        source_project: string("source_project")?,
        execution_project: string("execution_project")?,
        root: string("execution_root")?,
        base_sha: optional_string("base_sha"),
        source_head_sha: optional_string("source_head_sha"),
        source_tree_sha: optional_string("source_tree_sha"),
        source_index_tree_sha: optional_string("source_index_tree_sha"),
        registered_revision: optional_string("registered_revision"),
        prepare_operation_id: optional_string("prepare_operation_id"),
        cleanup_operation_id: optional_string("cleanup_operation_id"),
        cleanup_phase: value
            .get("cleanup_phase")
            .and_then(Value::as_str)
            .and_then(WorkspaceCleanupPhase::parse)
            .unwrap_or_default(),
        state,
        preserve_reason: optional_string("preserve_reason"),
        timings: WorkspaceTimings {
            snapshot_ms: timing("snapshot_ms"),
            worktree_creation_ms: timing("worktree_creation_ms"),
            scheduler_wait_ms: timing("scheduler_wait_ms"),
            execution_ms: timing("execution_ms"),
            integration_ms: timing("integration_ms"),
            validation_ms: timing("validation_ms"),
            apply_back_ms: timing("apply_back_ms"),
            cleanup_ms: timing("cleanup_ms"),
        },
    })
}

pub(crate) fn remove_workspace_manifest(workspace: &ExecutionWorkspace) -> Result<(), String> {
    let Some(root) = task_state_root() else {
        return Ok(());
    };
    remove_workspace_manifest_at(&root, workspace)
}

fn remove_workspace_manifest_at(root: &Path, workspace: &ExecutionWorkspace) -> Result<(), String> {
    let directory = private_state_directory(root, "workspaces")?;
    let path = directory.join(format!("{}.json", workspace_manifest_stem(workspace)));
    remove_private_state_file(&directory, &path)
}

pub(crate) fn persist_workspace_manifest(workspace: &ExecutionWorkspace) -> Result<(), String> {
    let Some(root) = task_state_root() else {
        return Ok(());
    };
    persist_workspace_manifest_at(&root, workspace)
}

pub(crate) fn persist_workspace_projection_best_effort(workspace: &ExecutionWorkspace) {
    if let Err(error) = persist_workspace_manifest(workspace) {
        tracing::warn!(
            task_id = %workspace.task_id,
            execution_project = %workspace.execution_project,
            error = %error,
            "workspace projection persistence failed"
        );
    }
}

fn persist_workspace_manifest_at(
    root: &Path,
    workspace: &ExecutionWorkspace,
) -> Result<(), String> {
    let directory = private_state_directory(root, "workspaces")?;
    let path = directory.join(format!("{}.json", workspace_manifest_stem(workspace)));
    atomic_private_json_write(&directory, &path, &workspace.manifest())
}

struct WorkspaceCleanupLease(std::fs::File);

impl Drop for WorkspaceCleanupLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

fn acquire_workspace_cleanup_lease(
    workspace: &ExecutionWorkspace,
) -> Result<Option<WorkspaceCleanupLease>, String> {
    let Some(root) = task_state_root() else {
        return Ok(None);
    };
    let directory = private_state_directory(&root, "workspace-locks")?;
    let path = directory.join(format!("{}.lock", workspace_manifest_stem(workspace)));
    let mut options = std::fs::OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options
        .open(path)
        .map_err(|error| format!("execution_workspace_cleanup_lease_open_failed:{error}"))?;
    file.try_lock_exclusive().map_err(|error| {
        if error.kind() == std::io::ErrorKind::WouldBlock {
            "execution_workspace_cleanup_busy".to_string()
        } else {
            format!("execution_workspace_cleanup_lease_failed:{error}")
        }
    })?;
    Ok(Some(WorkspaceCleanupLease(file)))
}

fn workspace_manifest_stem(workspace: &ExecutionWorkspace) -> String {
    let digest = format!(
        "{:x}",
        Sha256::digest(workspace.execution_project.as_bytes())
    );
    format!(
        "{}-{}-{}",
        workspace.task_id,
        workspace.mode.as_str(),
        &digest[..16]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;
    use tokio::sync::Barrier;

    #[test]
    fn workspace_manifest_roundtrips_for_recovery() {
        let workspace = ExecutionWorkspace {
            task_id: "chadex_task_0123456789abcdef0123456789abcdef".to_string(),
            mode: IsolationMode::Integration,
            source_project: "source-project".to_string(),
            execution_project: "execution-project".to_string(),
            root: "/tmp/chadex-recovery-worktree".to_string(),
            base_sha: Some("a".repeat(40)),
            source_head_sha: Some("b".repeat(40)),
            source_tree_sha: Some("c".repeat(40)),
            source_index_tree_sha: Some("d".repeat(40)),
            registered_revision: Some("revision-1".to_string()),
            prepare_operation_id: Some("prepare-operation-1".to_string()),
            cleanup_operation_id: Some("cleanup-operation-1".to_string()),
            cleanup_phase: WorkspaceCleanupPhase::IntentPersisted,
            state: WorkspaceState::Preserved,
            preserve_reason: Some("runtime_interrupted".to_string()),
            timings: WorkspaceTimings {
                snapshot_ms: 1,
                worktree_creation_ms: 2,
                scheduler_wait_ms: 3,
                execution_ms: 4,
                integration_ms: 5,
                validation_ms: 6,
                apply_back_ms: 7,
                cleanup_ms: 8,
            },
        };
        let restored = workspace_from_manifest(&workspace.manifest()).unwrap();
        assert_eq!(restored.task_id, workspace.task_id);
        assert_eq!(restored.mode, IsolationMode::Integration);
        assert_eq!(restored.state, WorkspaceState::Preserved);
        assert_eq!(restored.execution_project, workspace.execution_project);
        assert_eq!(restored.registered_revision, workspace.registered_revision);
        assert_eq!(restored.timings.validation_ms, 6);
        assert_eq!(
            restored.preserve_reason.as_deref(),
            Some("runtime_interrupted")
        );
    }

    #[test]
    fn workspace_manifest_v2_roundtrips_transaction_identity_and_v1_defaults_safely() {
        let workspace = ExecutionWorkspace {
            task_id: "chadex_task_11111111111111111111111111111111".to_string(),
            mode: IsolationMode::Task,
            source_project: "source".to_string(),
            execution_project: "execution".to_string(),
            root: "/tmp/chadex-workspace-v2".to_string(),
            base_sha: Some("a".repeat(40)),
            source_head_sha: Some("b".repeat(40)),
            source_tree_sha: Some("c".repeat(40)),
            source_index_tree_sha: Some("d".repeat(40)),
            registered_revision: Some("revision-v2".to_string()),
            prepare_operation_id: Some("prepare-v2".to_string()),
            cleanup_operation_id: Some("cleanup-v2".to_string()),
            cleanup_phase: WorkspaceCleanupPhase::ProjectUnregistered,
            state: WorkspaceState::Preserved,
            preserve_reason: Some("cleanup_pending".to_string()),
            timings: WorkspaceTimings::default(),
        };
        let manifest = workspace.manifest();
        assert_eq!(manifest["manifest_version"], 2);
        let restored = workspace_from_manifest(&manifest).unwrap();
        assert_eq!(restored.prepare_operation_id.as_deref(), Some("prepare-v2"));
        assert_eq!(restored.cleanup_operation_id.as_deref(), Some("cleanup-v2"));
        assert_eq!(
            restored.cleanup_phase,
            WorkspaceCleanupPhase::ProjectUnregistered
        );

        let mut legacy = manifest;
        legacy["manifest_version"] = json!(1);
        legacy
            .as_object_mut()
            .unwrap()
            .remove("prepare_operation_id");
        legacy
            .as_object_mut()
            .unwrap()
            .remove("cleanup_operation_id");
        legacy.as_object_mut().unwrap().remove("cleanup_phase");
        let legacy = workspace_from_manifest(&legacy).unwrap();
        assert!(legacy.prepare_operation_id.is_none());
        assert!(legacy.cleanup_operation_id.is_none());
        assert_eq!(legacy.cleanup_phase, WorkspaceCleanupPhase::None);
    }

    #[test]
    fn prepare_intent_is_durable_and_recovers_exact_operation_identity() {
        let dir = tempfile::tempdir().unwrap();
        let intent = WorkspacePrepareIntent {
            task_id: "chadex_task_22222222222222222222222222222222".to_string(),
            mode: IsolationMode::Package,
            source_project: "source-project".to_string(),
            client_id: "runner-client".to_string(),
            source_path: "/tmp/source-project".to_string(),
            operation_id: "prepare-operation-exact".to_string(),
            base_ref: Some("a".repeat(40)),
            include_dirty_snapshot: true,
            source_head_sha: Some("b".repeat(40)),
            source_tree_sha: Some("c".repeat(40)),
            source_index_tree_sha: Some("d".repeat(40)),
        };
        persist_workspace_prepare_intent_at(dir.path(), &intent).unwrap();
        let restored = load_workspace_prepare_intents_at(dir.path());
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].operation_id, intent.operation_id);
        assert_eq!(restored[0].task_id, intent.task_id);
        assert_eq!(restored[0].mode, IsolationMode::Package);
        assert!(restored[0].include_dirty_snapshot);
    }

    #[cfg(unix)]
    #[test]
    fn durable_workspace_state_fails_closed_on_symlink_root() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let real_root = dir.path().join("real");
        std::fs::create_dir_all(&real_root).unwrap();
        let linked_root = dir.path().join("linked");
        symlink(&real_root, &linked_root).unwrap();
        let workspace = ExecutionWorkspace {
            task_id: "chadex_task_33333333333333333333333333333333".to_string(),
            mode: IsolationMode::Task,
            source_project: "source".to_string(),
            execution_project: "execution".to_string(),
            root: "/tmp/worktree".to_string(),
            base_sha: Some("a".repeat(40)),
            source_head_sha: None,
            source_tree_sha: None,
            source_index_tree_sha: None,
            registered_revision: Some("revision".to_string()),
            prepare_operation_id: Some("prepare".to_string()),
            cleanup_operation_id: None,
            cleanup_phase: WorkspaceCleanupPhase::None,
            state: WorkspaceState::Active,
            preserve_reason: None,
            timings: WorkspaceTimings::default(),
        };
        let error = persist_workspace_manifest_at(&linked_root, &workspace).unwrap_err();
        assert_eq!(error, "workspace_state_root_is_symlink");
    }

    #[test]
    fn clean_repo_fast_path_skips_dirty_snapshot_only_when_tree_and_index_match_head() {
        let head = "a".repeat(40);
        let working = "b".repeat(40);
        let index = "c".repeat(40);
        assert!(!source_requires_dirty_snapshot(&head, &head, &head));
        assert!(source_requires_dirty_snapshot(&head, &working, &head));
        assert!(source_requires_dirty_snapshot(&head, &head, &index));
    }

    #[test]
    fn relative_path_guard_rejects_escape_and_normalizes_dots() {
        assert_eq!(workspace_relative_path("./src/lib").unwrap(), "src/lib");
        assert_eq!(workspace_relative_path(".").unwrap(), ".");
        assert_eq!(
            workspace_relative_path("../outside"),
            Err("workspace_path_escapes_root".into())
        );
        assert_eq!(
            workspace_relative_path("/outside"),
            Err("workspace_path_must_be_relative".into())
        );
    }

    #[test]
    fn independent_packages_are_detected_without_graphify() {
        assert!(package_paths_are_independent(&[
            vec!["a.rs".into()],
            vec!["b.rs".into()],
        ]));
        assert!(!package_paths_are_independent(&[
            Vec::new(),
            vec!["b.rs".into()]
        ]));
        assert!(!package_paths_are_independent(&[
            vec!["a.rs".into()],
            vec!["a.rs".into(), "b.rs".into()],
        ]));
    }

    #[test]
    fn isolation_policy_keeps_small_and_dependent_work_sequential() {
        assert_eq!(
            choose_isolation_mode(true, "small", 2, true),
            IsolationMode::Task
        );
        assert_eq!(
            choose_isolation_mode(true, "large", 2, false),
            IsolationMode::Task
        );
        assert_eq!(
            choose_isolation_mode(true, "large", 2, true),
            IsolationMode::Package
        );
        assert_eq!(
            choose_isolation_mode(false, "very_large", 4, true),
            IsolationMode::None
        );
    }

    #[tokio::test]
    async fn scheduler_blocks_the_third_task_until_a_slot_is_released() {
        let scheduler = ExecutionScheduler {
            task_slots: Arc::new(Semaphore::new(2)),
            executor_slots: Arc::new(Semaphore::new(3)),
            max_tasks: 2,
            max_executor: 3,
        };
        let (first, _) = scheduler.acquire_task().await;
        let (second, _) = scheduler.acquire_task().await;
        let waiting = {
            let scheduler = scheduler.clone();
            tokio::spawn(async move { scheduler.acquire_task().await.1 })
        };
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(!waiting.is_finished());
        drop(first);
        assert!(waiting.await.unwrap() < 1000);
        drop(second);
    }

    #[tokio::test]
    async fn scheduler_allows_two_independent_tasks_to_overlap() {
        let scheduler = ExecutionScheduler {
            task_slots: Arc::new(Semaphore::new(2)),
            executor_slots: Arc::new(Semaphore::new(3)),
            max_tasks: 2,
            max_executor: 3,
        };
        let barrier = Arc::new(Barrier::new(3));
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..2 {
            let scheduler = scheduler.clone();
            let barrier = barrier.clone();
            let active = active.clone();
            let peak = peak.clone();
            tasks.push(tokio::spawn(async move {
                let (_permit, _) = scheduler.acquire_task().await;
                let now = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                barrier.wait().await;
                tokio::time::sleep(Duration::from_millis(5)).await;
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        barrier.wait().await;
        assert_eq!(peak.load(Ordering::SeqCst), 2);
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn recovery_manifest_identity_is_unique_per_execution_project() {
        let first = ExecutionWorkspace::read_only(
            "chadex_task_0123456789abcdef0123456789abcdef".to_string(),
            "agent:client:one".to_string(),
            "/tmp/one".to_string(),
        );
        let second = ExecutionWorkspace::read_only(
            first.task_id.clone(),
            "agent:client:two".to_string(),
            "/tmp/two".to_string(),
        );
        assert_ne!(
            workspace_manifest_stem(&first),
            workspace_manifest_stem(&second)
        );
        assert_eq!(
            workspace_manifest_stem(&first),
            workspace_manifest_stem(&first)
        );
    }
}
