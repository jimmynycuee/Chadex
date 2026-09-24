mod execution;
use execution::{uncertain_result, Execution, ExecutionGuard, ExecutionState};

use super::execution_workspace::{
    choose_isolation_mode, package_paths_are_independent, workspace_relative_path,
    ExecutionScheduler, ExecutionWorkspace, IsolationMode, WorkspaceState,
};
use super::kernel::{
    HostFileImportTrust, ToolCallContext, ToolCallErrorStatus, ToolCallRequest, ToolTransport,
};
use super::ResolvedProject;
use super::{
    sessions, ChadexTaskAcceptance, ChadexTaskBatchItem, ChadexTaskPolicy, ChadexTaskPreplan,
    ChadexTaskStep, ReadFilesItem, ToolResult, ToolRuntime,
};
use crate::auth::AuthContext;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::future::Future;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};
use tokio::task::JoinSet;
use uuid::Uuid;
use webcodex_tool_runtime_contracts::{ApplyFileChangeKind, ExecutionPurpose};

const DEFAULT_MAX_STEPS: usize = 12;
const ABSOLUTE_MAX_STEPS: usize = 20;
const DEFAULT_MAX_MUTATIONS: usize = 4;
const ABSOLUTE_MAX_MUTATIONS: usize = 8;
const DEFAULT_MAX_CHANGED_FILES: usize = 8;
const ABSOLUTE_MAX_CHANGED_FILES: usize = 32;
const DEFAULT_TIMEOUT_SECS: u64 = 120;
const ABSOLUTE_TIMEOUT_SECS: u64 = 600;
const DEFAULT_MAX_RESULT_BYTES: usize = 32 * 1024;
const MIN_MAX_RESULT_BYTES: usize = 8 * 1024;
const ABSOLUTE_MAX_RESULT_BYTES: usize = 256 * 1024;
const MAX_TASKS: usize = 64;
const MAX_FAILURE_MESSAGE_CHARS: usize = 800;
const MAX_FAILURE_STDIO_CHARS: usize = 1_600;
const MAX_COMPACT_GOAL_CHARS: usize = 1_024;
const MAX_READ_ONLY_RETRIES: usize = 1;
const LOCAL_TASK_LOG_MAX_BYTES: u64 = 8 * 1024 * 1024;
const TASK_STORAGE_LOG_TOTAL_MAX_BYTES: u64 = 64 * 1024 * 1024;
const TASK_TERMINAL_TTL_SECS: u64 = 7 * 24 * 60 * 60;
const CLEANED_WORKSPACE_TTL_SECS: u64 = 72 * 60 * 60;
const TASK_TEMP_FILE_TTL_SECS: u64 = 24 * 60 * 60;
static TASK_STATE_PERSIST_LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
const ALL_OPERATIONS: &[&str] = &[
    "search",
    "read",
    "edit",
    "run_process",
    "validate",
    "review",
];

const GRAPHIFY_DEPENDENCY_SCRIPT: &str = r#"
import collections, json, os, subprocess, sys
payload = json.loads(sys.argv[1])
graph_path = "graphify-out/graph.json"

def emit(
    status,
    fresh=False,
    analyzed=0,
    missing=0,
    boundaries=None,
    package_relations=None,
    parallelism_proven=False,
):
    print(json.dumps({
        "status": status,
        "fresh": fresh,
        "analyzed_file_count": analyzed,
        "missing_file_count": missing,
        "boundaries": boundaries or [],
        "package_relations": package_relations or [],
        "parallelism_proven": parallelism_proven,
    }, separators=(",", ":")))

if not os.path.isfile(graph_path):
    emit("graph_missing")
    raise SystemExit(0)

try:
    with open(graph_path, "r", encoding="utf-8") as handle:
        graph = json.load(handle)
except Exception:
    emit("graph_invalid")
    raise SystemExit(0)

packages = payload.get("packages") or []
candidates = sorted({path for package in packages for path in (package.get("files") or [])})
known_files = {node.get("source_file") for node in graph.get("nodes", []) if node.get("source_file")}
analyzed = sum(path in known_files for path in candidates)
missing = len(candidates) - analyzed

try:
    head = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        capture_output=True,
        text=True,
        timeout=1,
        check=False,
    ).stdout.strip()
except Exception:
    head = ""
built = graph.get("built_at_commit") or ""
graph_mtime = os.path.getmtime(graph_path)
newer_sources = [
    path for path in candidates
    if os.path.isfile(path) and os.path.getmtime(path) > graph_mtime + 0.001
]
newer_indexed_sources = []
try:
    status = subprocess.run(
        ["git", "status", "--porcelain", "-z"],
        capture_output=True,
        timeout=1,
        check=False,
    ).stdout.decode("utf-8", errors="replace")
    for entry in status.split("\0"):
        if len(entry) < 4:
            continue
        path = entry[3:]
        if " -> " in path:
            path = path.rsplit(" -> ", 1)[-1]
        if path in known_files and os.path.isfile(path) and os.path.getmtime(path) > graph_mtime + 0.001:
            newer_indexed_sources.append(path)
except Exception:
    newer_indexed_sources = []
fresh = bool(
    head and built and head == built and not newer_sources and not newer_indexed_sources
)
if not fresh:
    emit("graph_stale", False, analyzed, missing)
    raise SystemExit(0)

nodes = {node.get("id"): node for node in graph.get("nodes", [])}
allowed_relations = {"calls", "references", "imports_from", "imports", "depends_on", "re_exports"}
adjacency = collections.defaultdict(set)
direct = collections.Counter()
for link in graph.get("links", []):
    if link.get("confidence") != "EXTRACTED" or link.get("relation") not in allowed_relations:
        continue
    left = nodes.get(link.get("source"), {}).get("source_file")
    right = nodes.get(link.get("target"), {}).get("source_file")
    if not left or not right or left == right:
        continue
    adjacency[left].add(right)
    adjacency[right].add(left)
    direct[tuple(sorted((left, right)))] += 1

def package_relation(left_index, right_index):
    left_package = packages[left_index]
    right_package = packages[right_index]
    left_files = [path for path in (left_package.get("files") or []) if path in known_files]
    right_files = [path for path in (right_package.get("files") or []) if path in known_files]
    direct_edges = 0
    shared_neighbors = set()
    for left in left_files:
        for right in right_files:
            direct_edges += direct.get(tuple(sorted((left, right))), 0)
            shared_neighbors.update(
                neighbor
                for neighbor in adjacency.get(left, set()).intersection(adjacency.get(right, set()))
            )
    shared_count = len(shared_neighbors)
    return {
        "left_package": left_index,
        "right_package": right_index,
        "direct_edges": direct_edges,
        "shared_neighbor_count": shared_count,
        "strong": direct_edges > 0 or shared_count >= 2,
    }

package_relations = []
for left_index in range(len(packages)):
    for right_index in range(left_index + 1, len(packages)):
        package_relations.append(package_relation(left_index, right_index))

boundaries = []
for index in range(max(0, len(packages) - 1)):
    relation = next(
        relation
        for relation in package_relations
        if relation["left_package"] == index and relation["right_package"] == index + 1
    )
    boundaries.append({
        "boundary_after_step": packages[index].get("end_step", 0),
        "direct_edges": relation["direct_edges"],
        "shared_neighbor_count": relation["shared_neighbor_count"],
        "strong": relation["strong"],
    })

parallelism_proven = bool(
    missing == 0
    and all(package.get("files") for package in packages)
    and len(package_relations) == len(packages) * (len(packages) - 1) // 2
    and not any(relation["strong"] for relation in package_relations)
)

emit(
    "used",
    True,
    analyzed,
    missing,
    boundaries,
    package_relations,
    parallelism_proven,
)
"#;

#[derive(Debug, Clone, Serialize)]
struct NormalizedTaskPolicy {
    max_steps: usize,
    max_mutations: usize,
    max_changed_files: usize,
    timeout_secs: u64,
    max_result_bytes: usize,
    allowed_path_prefixes: Vec<String>,
    allowed_operations: Vec<String>,
}

#[derive(Debug, Clone)]
struct NormalizedAcceptance {
    require_validation_success: bool,
    require_review: bool,
}

#[derive(Debug, Clone, Serialize)]
struct TaskComplexityAssessment {
    score: u8,
    band: &'static str,
    recommended_task_count: usize,
    execution_task_count: usize,
    advisory_only: bool,
    signals: Value,
}

#[derive(Debug, Clone, Serialize)]
struct TaskPackageRange {
    index: usize,
    start_step: usize,
    end_step: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GraphifyBoundaryEvidence {
    boundary_after_step: usize,
    direct_edges: usize,
    shared_neighbor_count: usize,
    strong: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct GraphifyPackageRelation {
    left_package: usize,
    right_package: usize,
    direct_edges: usize,
    shared_neighbor_count: usize,
    strong: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct GraphifyDependencyEvidence {
    status: String,
    fresh: bool,
    #[serde(default)]
    analyzed_file_count: usize,
    #[serde(default)]
    missing_file_count: usize,
    #[serde(default)]
    boundaries: Vec<GraphifyBoundaryEvidence>,
    #[serde(default)]
    package_relations: Vec<GraphifyPackageRelation>,
    #[serde(default)]
    parallelism_proven: bool,
}

#[derive(Debug, Clone, Serialize)]
struct TaskPackagingPlan {
    requested_package_count: usize,
    recommended_package_count: usize,
    effective_package_count: usize,
    safe_boundary_count: usize,
    limited_by_recommendation: bool,
    limited_by_safe_boundaries: bool,
    outer_execute_task_count: usize,
    graphify_status: String,
    graphify_fresh: Option<bool>,
    graphify_adjusted: bool,
    dependency_merge_count: usize,
    graphify_planning_ms: u64,
    graphify_analyzed_file_count: usize,
    graphify_missing_file_count: usize,
    dependency_boundaries: Vec<GraphifyBoundaryEvidence>,
    dependency_proof: String,
    package_parallelism_proven: bool,
    parallel_cost_gate_passed: bool,
    parallel_cost_reason: String,
    estimated_sequential_ms: u64,
    estimated_parallel_ms: u64,
    estimated_parallel_gain_pct: f64,
    estimated_parallel_overhead_ms: u64,
    parallel_cost_source: String,
    parallel_overhead_samples: u64,
    packages: Vec<TaskPackageRange>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskStepSummary {
    index: usize,
    kind: String,
    status: String,
    duration_ms: u64,
    attempts: usize,
    retries: usize,
    result_bytes_before: usize,
    result_bytes_after: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<Value>,
}

#[derive(Debug, Clone, Serialize)]
struct TaskSnapshot {
    execution: Execution,
    task_id: String,
    project: String,
    source_path: String,
    goal: String,
    status: String,
    current_step: usize,
    total_steps: usize,
    completed_steps: usize,
    plan: Vec<String>,
    cancel_requested: bool,
    started_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    finished_at_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    limits: NormalizedTaskPolicy,
    counters: Value,
    validation: Value,
    review: Value,
    steps: Vec<TaskStepSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    failure: Option<Value>,
    complexity: TaskComplexityAssessment,
    packaging: TaskPackagingPlan,
    workspace: Value,
}

struct TaskControl {
    snapshot: StdMutex<TaskSnapshot>,
    raw_log: StdMutex<()>,
    cancel_requested: AtomicBool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskRecoveryPlan {
    version: u8,
    project: String,
    task_id: String,
    goal: String,
    preplan: Option<ChadexTaskPreplan>,
    package_count: Option<usize>,
    steps: Vec<ChadexTaskStep>,
    policy: Option<ChadexTaskPolicy>,
    acceptance: Option<ChadexTaskAcceptance>,
    created_at_ms: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AdaptiveExecutionTelemetry {
    version: u8,
    parallel_overhead_base_ewma_ms: f64,
    parallel_samples: u64,
    updated_at_ms: i64,
}

impl Default for AdaptiveExecutionTelemetry {
    fn default() -> Self {
        Self {
            version: 2,
            parallel_overhead_base_ewma_ms: PARALLEL_BASE_OVERHEAD_MS as f64,
            parallel_samples: 0,
            updated_at_ms: 0,
        }
    }
}

struct PackageExecutionOutcome {
    status: String,
    steps: Vec<TaskStepSummary>,
    completed_steps: usize,
    completed_mutations: usize,
    retries: usize,
    result_bytes_before: usize,
    result_bytes_after: usize,
    validation_seen: bool,
    validation_ok: bool,
    review_seen: bool,
    failure: Option<Value>,
}

impl Default for PackageExecutionOutcome {
    fn default() -> Self {
        Self {
            status: String::new(),
            steps: Vec::new(),
            completed_steps: 0,
            completed_mutations: 0,
            retries: 0,
            result_bytes_before: 0,
            result_bytes_after: 0,
            validation_seen: false,
            validation_ok: true,
            review_seen: false,
            failure: None,
        }
    }
}

pub(crate) struct ChadexTaskStore {
    tasks: StdMutex<HashMap<String, Arc<TaskControl>>>,
    recovered: StdMutex<HashMap<String, Value>>,
    scheduler: ExecutionScheduler,
    #[cfg(test)]
    execution_storage: tempfile::TempDir,
}

impl Default for ChadexTaskStore {
    fn default() -> Self {
        gc_task_storage();
        Self {
            tasks: StdMutex::new(HashMap::new()),
            recovered: StdMutex::new(load_recovered_task_projections()),
            scheduler: ExecutionScheduler::default(),
            #[cfg(test)]
            execution_storage: tempfile::tempdir().expect("isolated execution storage"),
        }
    }
}

impl ChadexTaskStore {
    fn execution_root(&self) -> Result<PathBuf, String> {
        #[cfg(test)]
        {
            Ok(self.execution_storage.path().to_path_buf())
        }
        #[cfg(not(test))]
        {
            task_storage_dir().ok_or_else(|| "execution_storage_unavailable".into())
        }
    }

    fn insert(&self, control: Arc<TaskControl>) -> Result<(), String> {
        let mut tasks = self.tasks.lock().map_err(|_| "task store lock poisoned")?;
        if tasks.len() >= MAX_TASKS {
            let oldest_terminal = tasks
                .iter()
                .filter_map(|(id, control)| {
                    let snapshot = control.snapshot.lock().ok()?;
                    is_terminal(&snapshot.status).then_some((id.clone(), snapshot.started_at_ms))
                })
                .min_by_key(|(_, started)| *started)
                .map(|(id, _)| id);
            if let Some(id) = oldest_terminal {
                tasks.remove(&id);
            } else {
                return Err("task_capacity_reached".to_string());
            }
        }
        let task_id = control
            .snapshot
            .lock()
            .map_err(|_| "task snapshot lock poisoned")?
            .task_id
            .clone();
        if tasks.contains_key(&task_id)
            || self
                .recovered
                .lock()
                .map_err(|_| "recovered task store lock poisoned")?
                .contains_key(&task_id)
        {
            return Err("task_id_conflict".to_string());
        }
        tasks.insert(task_id, control);
        Ok(())
    }

    fn get(&self, task_id: &str) -> Result<Arc<TaskControl>, String> {
        self.tasks
            .lock()
            .map_err(|_| "task store lock poisoned".to_string())?
            .get(task_id)
            .cloned()
            .ok_or_else(|| "task_not_found".to_string())
    }

    fn recovered(&self, task_id: &str) -> Option<Value> {
        self.recovered
            .lock()
            .ok()
            .and_then(|tasks| tasks.get(task_id).cloned())
    }

    fn recovered_for_project(&self, project: &str) -> Vec<Value> {
        let mut values = self
            .recovered
            .lock()
            .ok()
            .map(|tasks| {
                tasks
                    .values()
                    .filter(|value| value.get("project").and_then(Value::as_str) == Some(project))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        values.sort_by_key(|value| {
            std::cmp::Reverse(
                value
                    .get("started_at_ms")
                    .and_then(Value::as_i64)
                    .unwrap_or(0),
            )
        });
        values
    }

    fn update_recovered(&self, task_id: &str, value: Value) {
        if let Ok(mut recovered) = self.recovered.lock() {
            recovered.insert(task_id.to_string(), value);
        }
    }

    fn remove_recovered(&self, task_id: &str) {
        if let Ok(mut recovered) = self.recovered.lock() {
            recovered.remove(task_id);
        }
    }

    fn scheduler(&self) -> &ExecutionScheduler {
        &self.scheduler
    }
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().min(u64::MAX as u128) as u64
}

fn is_terminal(status: &str) -> bool {
    matches!(
        status,
        "completed"
            | "failed"
            | "failed_validation"
            | "blocked"
            | "cancelled"
            | "interrupted"
            | "unknown"
    )
}

fn valid_task_id(task_id: &str) -> bool {
    let Some(suffix) = task_id.strip_prefix("chadex_task_") else {
        return false;
    };
    suffix.len() == 32
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn task_step_kind(step: &ChadexTaskStep) -> &'static str {
    match step {
        ChadexTaskStep::Search { .. } => "search",
        ChadexTaskStep::Read { .. } => "read",
        ChadexTaskStep::Edit { .. } => "edit",
        ChadexTaskStep::RunProcess { .. } => "run_process",
        ChadexTaskStep::Validate { .. } => "validate",
        ChadexTaskStep::Review { .. } => "review",
    }
}

fn path_language(path: &str) -> Option<&'static str> {
    let extension = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "rs" => Some("rust"),
        "swift" => Some("swift"),
        "py" => Some("python"),
        "js" | "jsx" | "ts" | "tsx" => Some("javascript_typescript"),
        "kt" | "kts" => Some("kotlin"),
        "java" => Some("java"),
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" => Some("c_cpp"),
        "dart" => Some("dart"),
        "go" => Some("go"),
        _ => None,
    }
}

fn top_level_component(path: &str) -> Option<String> {
    Path::new(path)
        .components()
        .find_map(|component| match component {
            Component::Normal(value) => value.to_str().map(ToOwned::to_owned),
            _ => None,
        })
}

fn complexity_band(score: u8) -> (&'static str, usize) {
    match score {
        1..=3 => ("small", 1),
        4..=6 => ("medium", 2),
        7..=8 => ("large", 3),
        _ => ("very_large", 4),
    }
}

fn validate_task_preplan(preplan: &ChadexTaskPreplan) -> Result<(), String> {
    if !(1..=64).contains(&preplan.estimated_file_count) {
        return Err("preplan_estimated_file_count_out_of_range".to_string());
    }
    if !(1..=32).contains(&preplan.estimated_subsystem_count) {
        return Err("preplan_estimated_subsystem_count_out_of_range".to_string());
    }
    if !(1..=16).contains(&preplan.estimated_language_count) {
        return Err("preplan_estimated_language_count_out_of_range".to_string());
    }
    if preplan.validation_domain_count > 16 {
        return Err("preplan_validation_domain_count_out_of_range".to_string());
    }
    Ok(())
}

fn assess_plan_shape_complexity(
    steps: &[ChadexTaskStep],
    planned_mutations: usize,
    planned_changed_files: usize,
) -> TaskComplexityAssessment {
    let mut explicit_paths = HashSet::new();
    let mut subsystems = HashSet::new();
    let mut languages = HashSet::new();
    let mut validation_checks = 0usize;
    let mut process_steps = 0usize;

    let mut record_path = |path: &str| {
        if path.is_empty() {
            return;
        }
        explicit_paths.insert(path.to_string());
        if let Some(subsystem) = top_level_component(path) {
            subsystems.insert(subsystem);
        }
        if let Some(language) = path_language(path) {
            languages.insert(language.to_string());
        }
    };

    for step in steps {
        match step {
            ChadexTaskStep::Search { queries, .. } => {
                for query in queries {
                    if let Some(path) = query.path.as_deref() {
                        record_path(path);
                    }
                }
            }
            ChadexTaskStep::Read { items, .. } => {
                for item in items {
                    record_path(&item.path);
                }
            }
            ChadexTaskStep::Edit { changes, .. } => {
                for change in changes {
                    record_path(&change.path);
                    if let Some(path) = change.to_path.as_deref() {
                        record_path(path);
                    }
                }
            }
            ChadexTaskStep::RunProcess { cwd, .. } => {
                process_steps += 1;
                if let Some(path) = cwd.as_deref() {
                    record_path(path);
                }
            }
            ChadexTaskStep::Validate { checks } => {
                validation_checks += checks.len();
                for check in checks {
                    if let Some(path) = check.cwd.as_deref() {
                        record_path(path);
                    }
                }
            }
            ChadexTaskStep::Review { .. } => {}
        }
    }

    let step_count = steps.len();
    let explicit_file_count = explicit_paths.len().max(planned_changed_files);
    let subsystem_count = subsystems.len();
    let language_count = languages.len();

    let mut score = 1u8;
    score += u8::from(step_count >= 7);
    score += u8::from(explicit_file_count >= 3);
    score += u8::from(explicit_file_count >= 7);
    score += u8::from(planned_mutations >= 2);
    score += u8::from(planned_mutations >= 4);
    score += u8::from(subsystem_count >= 2);
    score += u8::from(language_count >= 2);
    score += u8::from(validation_checks >= 2 || process_steps >= 2);
    score = score.min(10);

    let (band, recommended_task_count) = complexity_band(score);

    TaskComplexityAssessment {
        score,
        band,
        recommended_task_count,
        execution_task_count: 1,
        advisory_only: false,
        signals: json!({
            "estimation_source": "legacy_plan_shape",
            "step_count": step_count,
            "explicit_file_count": explicit_file_count,
            "planned_mutations": planned_mutations,
            "planned_changed_files": planned_changed_files,
            "subsystem_count": subsystem_count,
            "language_count": language_count,
            "validation_check_count": validation_checks,
            "process_step_count": process_steps,
        }),
    }
}

fn assess_preplan_complexity(
    preplan: &ChadexTaskPreplan,
    plan_shape_shadow: &TaskComplexityAssessment,
) -> TaskComplexityAssessment {
    let mut score = 1u8;
    score += u8::from(preplan.estimated_file_count >= 3);
    score += u8::from(preplan.estimated_file_count >= 7);
    score += u8::from(preplan.estimated_subsystem_count >= 2);
    score += u8::from(preplan.estimated_subsystem_count >= 4);
    score += u8::from(preplan.estimated_language_count >= 2);
    score += u8::from(preplan.validation_domain_count >= 2);
    score += u8::from(preplan.cross_runtime_boundary);
    score += u8::from(preplan.stateful_or_schema_change);
    score += u8::from(preplan.concurrency_or_security_sensitive);
    score = score.min(10);

    let (band, recommended_task_count) = complexity_band(score);

    TaskComplexityAssessment {
        score,
        band,
        recommended_task_count,
        execution_task_count: 1,
        advisory_only: false,
        signals: json!({
            "estimation_source": "preplan",
            "estimated_file_count": preplan.estimated_file_count,
            "estimated_subsystem_count": preplan.estimated_subsystem_count,
            "estimated_language_count": preplan.estimated_language_count,
            "validation_domain_count": preplan.validation_domain_count,
            "cross_runtime_boundary": preplan.cross_runtime_boundary,
            "stateful_or_schema_change": preplan.stateful_or_schema_change,
            "concurrency_or_security_sensitive": preplan.concurrency_or_security_sensitive,
            "graphify_informed": preplan.graphify_informed,
            "plan_shape_shadow_score": plan_shape_shadow.score,
            "plan_shape_shadow_band": plan_shape_shadow.band,
        }),
    }
}

fn assess_task_complexity(
    preplan: Option<&ChadexTaskPreplan>,
    steps: &[ChadexTaskStep],
    planned_mutations: usize,
    planned_changed_files: usize,
) -> TaskComplexityAssessment {
    let plan_shape = assess_plan_shape_complexity(steps, planned_mutations, planned_changed_files);
    match preplan {
        Some(preplan) => assess_preplan_complexity(preplan, &plan_shape),
        None => plan_shape,
    }
}

fn initial_package_target(
    requested_package_count: Option<usize>,
    has_preplan: bool,
    recommended_package_count: usize,
) -> Option<usize> {
    if requested_package_count.is_none() && has_preplan {
        Some(recommended_package_count)
    } else {
        requested_package_count
    }
}

fn is_safe_package_boundary(step: &ChadexTaskStep) -> bool {
    match step {
        ChadexTaskStep::Validate { .. } => true,
        ChadexTaskStep::RunProcess {
            purpose: Some(purpose),
            ..
        } => matches!(
            purpose,
            ExecutionPurpose::Validation
                | ExecutionPurpose::Test
                | ExecutionPurpose::Build
                | ExecutionPurpose::Format
                | ExecutionPurpose::Release
        ),
        _ => false,
    }
}

fn is_integration_check(step: &ChadexTaskStep) -> bool {
    match step {
        ChadexTaskStep::Validate { .. } | ChadexTaskStep::Review { .. } => true,
        ChadexTaskStep::RunProcess {
            purpose: Some(purpose),
            ..
        } => matches!(
            purpose,
            ExecutionPurpose::Validation
                | ExecutionPurpose::Test
                | ExecutionPurpose::Build
                | ExecutionPurpose::Format
                | ExecutionPurpose::Release
        ),
        _ => false,
    }
}

const PARALLEL_MIN_GAIN_PCT: f64 = 15.0;
const PARALLEL_BASE_OVERHEAD_MS: u64 = 7_500;
const PARALLEL_EXTRA_PACKAGE_OVERHEAD_MS: u64 = 1_500;
const PARALLEL_OVERHEAD_EWMA_ALPHA: f64 = 0.25;
const PARALLEL_OVERHEAD_MIN_MS: u64 = 3_000;
const PARALLEL_OVERHEAD_MAX_MS: u64 = 30_000;

fn parse_sleep_cost_ms(executable: &str, args: &[String]) -> Option<u64> {
    let name = Path::new(executable).file_name()?.to_str()?;
    if name != "sleep" {
        return None;
    }
    let seconds = args.first()?.parse::<f64>().ok()?;
    if !seconds.is_finite() || seconds < 0.0 {
        return None;
    }
    Some((seconds * 1000.0).round().clamp(0.0, 120_000.0) as u64)
}

fn estimate_process_cost_ms(
    executable: &str,
    args: &[String],
    timeout_secs: Option<u64>,
    purpose: Option<&ExecutionPurpose>,
) -> u64 {
    if let Some(cost) = parse_sleep_cost_ms(executable, args) {
        return cost;
    }
    let (default_ms, timeout_fraction, cap_ms) = match purpose {
        Some(ExecutionPurpose::Build | ExecutionPurpose::Test | ExecutionPurpose::Release) => {
            (5_000_u64, 600_u64, 15_000_u64)
        }
        Some(ExecutionPurpose::Validation | ExecutionPurpose::Format) => {
            (1_500_u64, 350_u64, 8_000_u64)
        }
        _ => (2_000_u64, 300_u64, 8_000_u64),
    };
    timeout_secs
        .map(|seconds| {
            seconds
                .saturating_mul(timeout_fraction)
                .clamp(default_ms, cap_ms)
        })
        .unwrap_or(default_ms)
}

fn estimate_step_cost_ms(step: &ChadexTaskStep) -> u64 {
    match step {
        ChadexTaskStep::Search { .. } => 800,
        ChadexTaskStep::Read { items, .. } => 250_u64.saturating_mul(items.len().max(1) as u64),
        ChadexTaskStep::Edit { changes, .. } => {
            600_u64.saturating_add(150_u64.saturating_mul(changes.len().max(1) as u64))
        }
        ChadexTaskStep::RunProcess {
            executable,
            args,
            timeout_secs,
            purpose,
            ..
        } => estimate_process_cost_ms(executable, args, *timeout_secs, purpose.as_ref()),
        ChadexTaskStep::Validate { checks } => checks
            .iter()
            .map(|check| {
                estimate_process_cost_ms(
                    &check.executable,
                    &check.args,
                    check.timeout_secs,
                    Some(&ExecutionPurpose::Validation),
                )
            })
            .sum::<u64>()
            .max(750),
        ChadexTaskStep::Review { .. } => 700,
    }
}

fn estimated_parallel_overhead_ms(package_count: usize, base_overhead_ms: u64) -> u64 {
    base_overhead_ms.saturating_add(
        PARALLEL_EXTRA_PACKAGE_OVERHEAD_MS.saturating_mul(package_count.saturating_sub(1) as u64),
    )
}

fn estimated_parallel_critical_path_ms(package_costs: &[u64], workers: usize) -> u64 {
    if package_costs.is_empty() {
        return 0;
    }
    let workers = workers.clamp(1, package_costs.len());
    let mut lanes = vec![0_u64; workers];
    let mut costs = package_costs.to_vec();
    costs.sort_unstable_by(|left, right| right.cmp(left));
    for cost in costs {
        let (index, _) = lanes
            .iter()
            .enumerate()
            .min_by_key(|(_, value)| **value)
            .expect("at least one parallel cost lane");
        lanes[index] = lanes[index].saturating_add(cost);
    }
    lanes.into_iter().max().unwrap_or(0)
}

fn apply_parallel_cost_gate(
    packaging: &mut TaskPackagingPlan,
    steps: &[ChadexTaskStep],
    max_executor_concurrency: usize,
    base_overhead_ms: u64,
    overhead_samples: u64,
) {
    let package_costs = packaging
        .packages
        .iter()
        .map(|package| {
            steps[package.start_step..=package.end_step]
                .iter()
                .map(estimate_step_cost_ms)
                .sum::<u64>()
        })
        .collect::<Vec<_>>();
    let sequential_ms = package_costs.iter().copied().sum::<u64>();
    let overhead_ms = estimated_parallel_overhead_ms(package_costs.len(), base_overhead_ms);
    let parallel_ms = estimated_parallel_critical_path_ms(
        &package_costs,
        max_executor_concurrency.min(package_costs.len().max(1)),
    )
    .saturating_add(overhead_ms);
    let gain_pct = if sequential_ms == 0 || parallel_ms >= sequential_ms {
        0.0
    } else {
        ((sequential_ms - parallel_ms) as f64 / sequential_ms as f64) * 100.0
    };
    packaging.estimated_sequential_ms = sequential_ms;
    packaging.estimated_parallel_ms = parallel_ms;
    packaging.estimated_parallel_overhead_ms = overhead_ms;
    packaging.parallel_cost_source = if overhead_samples > 0 {
        "measured_ewma".to_string()
    } else {
        "cold_prior".to_string()
    };
    packaging.parallel_overhead_samples = overhead_samples;
    packaging.estimated_parallel_gain_pct = (gain_pct * 10.0).round() / 10.0;
    packaging.parallel_cost_gate_passed = packaging.package_parallelism_proven
        && package_costs.len() > 1
        && gain_pct >= PARALLEL_MIN_GAIN_PCT;
    packaging.parallel_cost_reason = if !packaging.package_parallelism_proven {
        "dependency_proof_required".to_string()
    } else if package_costs.len() <= 1 {
        "single_package".to_string()
    } else if packaging.parallel_cost_gate_passed {
        "estimated_gain_at_least_15_percent".to_string()
    } else {
        "estimated_gain_below_15_percent".to_string()
    };
}

fn text_mentions_package_path(value: &str, package_paths: &[String]) -> bool {
    package_paths.iter().any(|path| {
        value.contains(path)
            || Path::new(path)
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| !name.is_empty() && value.contains(name))
    })
}

fn integration_check_is_package_scoped(step: &ChadexTaskStep, package_paths: &[String]) -> bool {
    if package_paths.is_empty() {
        return false;
    }
    match step {
        ChadexTaskStep::Validate { checks } => {
            !checks.is_empty()
                && checks.iter().all(|check| {
                    check
                        .args
                        .iter()
                        .any(|arg| text_mentions_package_path(arg, package_paths))
                        || check
                            .cwd
                            .as_deref()
                            .is_some_and(|cwd| text_mentions_package_path(cwd, package_paths))
                })
        }
        ChadexTaskStep::RunProcess {
            args,
            cwd,
            purpose: Some(ExecutionPurpose::Validation | ExecutionPurpose::Format),
            ..
        } => {
            args.iter()
                .any(|arg| text_mentions_package_path(arg, package_paths))
                || cwd
                    .as_deref()
                    .is_some_and(|cwd| text_mentions_package_path(cwd, package_paths))
        }
        _ => false,
    }
}

fn integration_checks_for_parallel(
    steps: &[ChadexTaskStep],
    packaging: &TaskPackagingPlan,
    package_files: &[Vec<String>],
) -> (Vec<ChadexTaskStep>, usize) {
    let mut output = Vec::new();
    let mut deduplicated = 0usize;
    let mut seen = HashSet::new();
    for (index, step) in steps.iter().enumerate() {
        if !is_integration_check(step) {
            continue;
        }
        let package_index = package_index_for_step(packaging, index);
        let paths = package_files
            .get(package_index)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        if integration_check_is_package_scoped(step, paths) {
            deduplicated += 1;
            continue;
        }
        let signature = serde_json::to_string(step).unwrap_or_else(|_| format!("step-{index}"));
        if !seen.insert(signature) {
            deduplicated += 1;
            continue;
        }
        output.push(step.clone());
    }
    (output, deduplicated)
}

fn plan_task_packages(
    steps: &[ChadexTaskStep],
    requested_package_count: Option<usize>,
    recommended_package_count: usize,
) -> Result<TaskPackagingPlan, String> {
    let requested = requested_package_count.unwrap_or(1);
    if !(1..=4).contains(&requested) {
        return Err("package_count_out_of_range".to_string());
    }

    let recommended = recommended_package_count.clamp(1, 4);
    let target = requested.min(recommended);
    let safe_boundaries = steps
        .iter()
        .enumerate()
        .filter_map(|(index, step)| {
            let boundary = index + 1;
            (is_safe_package_boundary(step) && boundary + 1 < steps.len()).then_some(boundary)
        })
        .collect::<Vec<_>>();
    let effective = target.min(safe_boundaries.len() + 1).max(1);
    let selected_boundaries = safe_boundaries
        .iter()
        .copied()
        .take(effective.saturating_sub(1))
        .collect::<Vec<_>>();

    let mut packages = Vec::with_capacity(effective);
    let mut start_step = 0usize;
    for boundary in selected_boundaries {
        packages.push(TaskPackageRange {
            index: packages.len(),
            start_step,
            end_step: boundary - 1,
        });
        start_step = boundary;
    }
    packages.push(TaskPackageRange {
        index: packages.len(),
        start_step,
        end_step: steps.len().saturating_sub(1),
    });

    Ok(TaskPackagingPlan {
        requested_package_count: requested,
        recommended_package_count: recommended,
        effective_package_count: packages.len(),
        safe_boundary_count: safe_boundaries.len(),
        limited_by_recommendation: requested > recommended,
        limited_by_safe_boundaries: target > safe_boundaries.len() + 1,
        outer_execute_task_count: 1,
        graphify_status: "not_requested".to_string(),
        graphify_fresh: None,
        graphify_adjusted: false,
        dependency_merge_count: 0,
        graphify_planning_ms: 0,
        graphify_analyzed_file_count: 0,
        graphify_missing_file_count: 0,
        dependency_boundaries: Vec::new(),
        dependency_proof: "unknown".to_string(),
        package_parallelism_proven: false,
        parallel_cost_gate_passed: false,
        parallel_cost_reason: "not_evaluated".to_string(),
        estimated_sequential_ms: 0,
        estimated_parallel_ms: 0,
        estimated_parallel_gain_pct: 0.0,
        estimated_parallel_overhead_ms: 0,
        parallel_cost_source: "cold_prior".to_string(),
        parallel_overhead_samples: 0,
        packages,
    })
}

fn dependency_paths_for_step(step: &ChadexTaskStep) -> Vec<String> {
    let mut paths = Vec::new();
    match step {
        ChadexTaskStep::Read { items, .. } => {
            paths.extend(items.iter().map(|item| item.path.clone()));
        }
        ChadexTaskStep::Edit { changes, .. } => {
            for change in changes {
                paths.push(change.path.clone());
                if let Some(path) = change.to_path.as_ref() {
                    paths.push(path.clone());
                }
            }
        }
        _ => {}
    }
    paths = paths
        .into_iter()
        .filter_map(|path| workspace_relative_path(&path).ok())
        .filter(|path| path != ".")
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

fn dependency_payload(steps: &[ChadexTaskStep], packaging: &TaskPackagingPlan) -> Value {
    let packages = packaging
        .packages
        .iter()
        .map(|package| {
            let mut files = Vec::new();
            for step in &steps[package.start_step..=package.end_step] {
                files.extend(dependency_paths_for_step(step));
            }
            files.sort();
            files.dedup();
            json!({
                "index": package.index,
                "start_step": package.start_step,
                "end_step": package.end_step,
                "files": files,
            })
        })
        .collect::<Vec<_>>();
    json!({"packages": packages})
}

fn should_use_graphify(packaging: &TaskPackagingPlan, complexity_band: &str) -> bool {
    packaging.effective_package_count >= 2 && matches!(complexity_band, "large" | "very_large")
}

fn empty_graphify_evidence(status: &str) -> GraphifyDependencyEvidence {
    GraphifyDependencyEvidence {
        status: status.to_string(),
        fresh: false,
        analyzed_file_count: 0,
        missing_file_count: 0,
        boundaries: Vec::new(),
        package_relations: Vec::new(),
        parallelism_proven: false,
    }
}

fn graphify_proves_package_independence(
    packaging: &TaskPackagingPlan,
    evidence: &GraphifyDependencyEvidence,
) -> bool {
    if evidence.status != "used"
        || !evidence.fresh
        || evidence.missing_file_count != 0
        || !evidence.parallelism_proven
    {
        return false;
    }
    let expected_relations = packaging
        .packages
        .len()
        .saturating_mul(packaging.packages.len().saturating_sub(1))
        / 2;
    evidence.package_relations.len() == expected_relations
        && evidence.package_relations.iter().all(|relation| {
            relation.left_package < packaging.packages.len()
                && relation.right_package < packaging.packages.len()
                && relation.left_package < relation.right_package
                && !relation.strong
        })
}

fn apply_graphify_dependency_evidence(
    packaging: &mut TaskPackagingPlan,
    evidence: GraphifyDependencyEvidence,
    planning_ms: u64,
) {
    packaging.graphify_status = evidence.status.clone();
    packaging.graphify_fresh = Some(evidence.fresh);
    packaging.graphify_planning_ms = planning_ms;
    packaging.graphify_analyzed_file_count = evidence.analyzed_file_count;
    packaging.graphify_missing_file_count = evidence.missing_file_count;
    packaging.dependency_boundaries = evidence.boundaries.clone();
    packaging.package_parallelism_proven =
        graphify_proves_package_independence(packaging, &evidence);
    packaging.dependency_proof = if packaging.package_parallelism_proven {
        "graphify_no_cross_package_dependency".to_string()
    } else if evidence.status == "used" && evidence.fresh && evidence.missing_file_count == 0 {
        "graphify_dependency_or_incomplete_proof".to_string()
    } else {
        "dependency_unknown".to_string()
    };

    if evidence.status != "used" || !evidence.fresh || packaging.packages.len() <= 1 {
        return;
    }

    let strong_boundaries = evidence
        .boundaries
        .iter()
        .filter(|boundary| boundary.strong)
        .map(|boundary| boundary.boundary_after_step)
        .collect::<HashSet<_>>();
    if strong_boundaries.is_empty() {
        return;
    }

    let before = packaging.packages.len();
    let mut merged: Vec<TaskPackageRange> = Vec::with_capacity(before);
    for package in packaging.packages.clone() {
        if let Some(last) = merged.last_mut() {
            if strong_boundaries.contains(&last.end_step) {
                last.end_step = package.end_step;
                continue;
            }
        }
        let mut package = package;
        package.index = merged.len();
        merged.push(package);
    }
    packaging.dependency_merge_count = before.saturating_sub(merged.len());
    packaging.graphify_adjusted = packaging.dependency_merge_count > 0;
    packaging.effective_package_count = merged.len();
    packaging.packages = merged;
}

fn normalize_policy(policy: Option<ChadexTaskPolicy>) -> Result<NormalizedTaskPolicy, String> {
    let policy = policy.unwrap_or_default();
    let max_steps = policy.max_steps.unwrap_or(DEFAULT_MAX_STEPS);
    let max_mutations = policy.max_mutations.unwrap_or(DEFAULT_MAX_MUTATIONS);
    let max_changed_files = policy
        .max_changed_files
        .unwrap_or(DEFAULT_MAX_CHANGED_FILES);
    let timeout_secs = policy.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    let max_result_bytes = policy.max_result_bytes.unwrap_or(DEFAULT_MAX_RESULT_BYTES);
    if !(1..=ABSOLUTE_MAX_STEPS).contains(&max_steps) {
        return Err("max_steps_out_of_range".to_string());
    }
    if max_mutations > ABSOLUTE_MAX_MUTATIONS {
        return Err("max_mutations_out_of_range".to_string());
    }
    if !(1..=ABSOLUTE_MAX_CHANGED_FILES).contains(&max_changed_files) {
        return Err("max_changed_files_out_of_range".to_string());
    }
    if !(1..=ABSOLUTE_TIMEOUT_SECS).contains(&timeout_secs) {
        return Err("timeout_secs_out_of_range".to_string());
    }
    if !(MIN_MAX_RESULT_BYTES..=ABSOLUTE_MAX_RESULT_BYTES).contains(&max_result_bytes) {
        return Err("max_result_bytes_out_of_range".to_string());
    }

    let mut prefixes = Vec::new();
    for raw in policy.allowed_path_prefixes {
        let prefix = raw.trim().trim_end_matches('/');
        if prefix.is_empty() || !safe_relative_path(prefix) {
            return Err("invalid_allowed_path_prefix".to_string());
        }
        if !prefixes.iter().any(|existing| existing == prefix) {
            prefixes.push(prefix.to_string());
        }
    }

    let mut operations = if policy.allowed_operations.is_empty() {
        ALL_OPERATIONS
            .iter()
            .map(|value| (*value).to_string())
            .collect()
    } else {
        policy.allowed_operations
    };
    operations.sort();
    operations.dedup();
    if operations
        .iter()
        .any(|operation| !ALL_OPERATIONS.contains(&operation.as_str()))
    {
        return Err("unknown_allowed_operation".to_string());
    }

    Ok(NormalizedTaskPolicy {
        max_steps,
        max_mutations,
        max_changed_files,
        timeout_secs,
        max_result_bytes,
        allowed_path_prefixes: prefixes,
        allowed_operations: operations,
    })
}

fn normalize_acceptance(acceptance: Option<ChadexTaskAcceptance>) -> NormalizedAcceptance {
    let acceptance = acceptance.unwrap_or_default();
    NormalizedAcceptance {
        require_validation_success: acceptance.require_validation_success.unwrap_or(true),
        require_review: acceptance.require_review.unwrap_or(true),
    }
}

fn safe_relative_path(path: &str) -> bool {
    !path.is_empty() && workspace_relative_path(path).is_ok()
}

fn path_allowed(path: &str, prefixes: &[String]) -> bool {
    if !safe_relative_path(path) {
        return false;
    }
    if prefixes.is_empty() {
        return true;
    }
    prefixes.iter().any(|prefix| {
        path == prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

fn preflight_task(
    goal: &str,
    steps: &[ChadexTaskStep],
    policy: &NormalizedTaskPolicy,
) -> Result<(usize, usize), String> {
    if goal.trim().is_empty() || goal.chars().count() > 4000 {
        return Err("invalid_goal".to_string());
    }
    if steps.is_empty() || steps.len() > policy.max_steps {
        return Err("task_step_limit_exceeded".to_string());
    }
    let allowed: HashSet<&str> = policy
        .allowed_operations
        .iter()
        .map(String::as_str)
        .collect();
    let mut mutations = 0usize;
    let mut changed_paths = HashSet::new();

    for step in steps {
        let kind = task_step_kind(step);
        if !allowed.contains(kind) {
            return Err(format!("operation_not_allowed:{kind}"));
        }
        match step {
            ChadexTaskStep::Search { queries, .. } => {
                if queries.is_empty() || queries.len() > 8 {
                    return Err("invalid_search_batch".to_string());
                }
                if !policy.allowed_path_prefixes.is_empty()
                    && queries.iter().any(|query| {
                        query
                            .path
                            .as_deref()
                            .map(|path| path_allowed(path, &policy.allowed_path_prefixes))
                            != Some(true)
                    })
                {
                    return Err("scoped_task_search_requires_allowed_path".to_string());
                }
            }
            ChadexTaskStep::Read { items, .. } => {
                if items.is_empty() || items.len() > 8 {
                    return Err("invalid_read_batch".to_string());
                }
                if items
                    .iter()
                    .any(|item| !path_allowed(&item.path, &policy.allowed_path_prefixes))
                {
                    return Err("read_path_outside_task_scope".to_string());
                }
            }
            ChadexTaskStep::Edit { changes, dry_run } => {
                if changes.is_empty() || changes.len() > 16 {
                    return Err("invalid_edit_batch".to_string());
                }
                if !dry_run.unwrap_or(false) {
                    mutations += 1;
                }
                for change in changes {
                    if !path_allowed(&change.path, &policy.allowed_path_prefixes) {
                        return Err("edit_path_outside_task_scope".to_string());
                    }
                    changed_paths.insert(change.path.clone());
                    if let Some(to_path) = change.to_path.as_deref() {
                        if !path_allowed(to_path, &policy.allowed_path_prefixes) {
                            return Err("edit_destination_outside_task_scope".to_string());
                        }
                        changed_paths.insert(to_path.to_string());
                    }
                }
            }
            ChadexTaskStep::RunProcess { cwd, .. } => {
                if !policy.allowed_path_prefixes.is_empty() {
                    return Err("path_scoped_task_cannot_run_process".to_string());
                }
                if let Some(cwd) = cwd.as_deref() {
                    if !cwd.is_empty() && cwd != "." && !safe_relative_path(cwd) {
                        return Err("invalid_process_cwd".to_string());
                    }
                }
            }
            ChadexTaskStep::Validate { checks } => {
                if checks.is_empty() || checks.len() > 4 {
                    return Err("invalid_validation_pipeline".to_string());
                }
                if !policy.allowed_path_prefixes.is_empty() {
                    return Err("path_scoped_task_cannot_run_validation".to_string());
                }
                if checks
                    .iter()
                    .any(|check| check.executable.trim().is_empty())
                {
                    return Err("invalid_validation_executable".to_string());
                }
            }
            ChadexTaskStep::Review { .. } => {
                if !policy.allowed_path_prefixes.is_empty() {
                    return Err("path_scoped_task_cannot_review".to_string());
                }
            }
        }
    }
    if mutations > policy.max_mutations {
        return Err("task_mutation_limit_exceeded".to_string());
    }
    if changed_paths.len() > policy.max_changed_files {
        return Err("task_changed_file_limit_exceeded".to_string());
    }
    Ok((mutations, changed_paths.len()))
}

fn bounded_message(message: &str) -> String {
    message.chars().take(MAX_FAILURE_MESSAGE_CHARS).collect()
}

fn bounded_text(message: &str, max_chars: usize) -> String {
    message.chars().take(max_chars).collect()
}

fn bounded_tail_text(message: &str, max_chars: usize) -> String {
    let total = message.chars().count();
    if total <= max_chars {
        return message.to_string();
    }
    message
        .chars()
        .skip(total.saturating_sub(max_chars))
        .collect()
}

fn serialized_len(value: &Value) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

fn compact_step_value(step: &TaskStepSummary) -> Value {
    let mut value = json!({
        "index": step.index,
        "kind": step.kind,
        "status": step.status,
        "duration_ms": step.duration_ms,
        "attempts": step.attempts,
        "retries": step.retries,
        "result_bytes_before": step.result_bytes_before,
        "result_bytes_after": step.result_bytes_after,
    });
    if (step.status != "completed" || matches!(step.kind.as_str(), "validate" | "review"))
        && step.summary.is_some()
    {
        value["summary"] = step.summary.clone().unwrap_or(Value::Null);
    }
    value
}

fn package_index_for_step(packaging: &TaskPackagingPlan, step_index: usize) -> usize {
    packaging
        .packages
        .iter()
        .find(|package| step_index >= package.start_step && step_index <= package.end_step)
        .map(|package| package.index)
        .unwrap_or_else(|| packaging.packages.len().saturating_sub(1))
}

fn packaging_projection(snapshot: &TaskSnapshot) -> Value {
    let current_step = snapshot
        .current_step
        .min(snapshot.total_steps.saturating_sub(1));
    let current_package = snapshot
        .packaging
        .packages
        .iter()
        .find(|package| current_step >= package.start_step && current_step <= package.end_step)
        .map(|package| package.index)
        .unwrap_or_else(|| snapshot.packaging.packages.len().saturating_sub(1));
    let packages = snapshot
        .packaging
        .packages
        .iter()
        .map(|package| {
            let status = if snapshot.completed_steps > package.end_step {
                "completed"
            } else if current_step >= package.start_step && current_step <= package.end_step {
                match snapshot.status.as_str() {
                    "failed" | "failed_validation" => "failed",
                    "blocked" => "blocked",
                    "cancelled" => "cancelled",
                    "cancelling" => "cancelling",
                    "queued" => "queued",
                    _ => "running",
                }
            } else {
                "queued"
            };
            json!({
                "index": package.index,
                "start_step": package.start_step,
                "end_step": package.end_step,
                "status": status,
            })
        })
        .collect::<Vec<_>>();

    json!({
        "requested_package_count": snapshot.packaging.requested_package_count,
        "recommended_package_count": snapshot.packaging.recommended_package_count,
        "effective_package_count": snapshot.packaging.effective_package_count,
        "safe_boundary_count": snapshot.packaging.safe_boundary_count,
        "limited_by_recommendation": snapshot.packaging.limited_by_recommendation,
        "limited_by_safe_boundaries": snapshot.packaging.limited_by_safe_boundaries,
        "outer_execute_task_count": snapshot.packaging.outer_execute_task_count,
        "graphify_status": snapshot.packaging.graphify_status,
        "graphify_fresh": snapshot.packaging.graphify_fresh,
        "graphify_adjusted": snapshot.packaging.graphify_adjusted,
        "dependency_merge_count": snapshot.packaging.dependency_merge_count,
        "graphify_planning_ms": snapshot.packaging.graphify_planning_ms,
        "graphify_analyzed_file_count": snapshot.packaging.graphify_analyzed_file_count,
        "graphify_missing_file_count": snapshot.packaging.graphify_missing_file_count,
        "dependency_boundaries": snapshot.packaging.dependency_boundaries,
        "dependency_proof": snapshot.packaging.dependency_proof,
        "package_parallelism_proven": snapshot.packaging.package_parallelism_proven,
        "parallel_cost_gate_passed": snapshot.packaging.parallel_cost_gate_passed,
        "parallel_cost_reason": snapshot.packaging.parallel_cost_reason,
        "estimated_sequential_ms": snapshot.packaging.estimated_sequential_ms,
        "estimated_parallel_ms": snapshot.packaging.estimated_parallel_ms,
        "estimated_parallel_gain_pct": snapshot.packaging.estimated_parallel_gain_pct,
        "estimated_parallel_overhead_ms": snapshot.packaging.estimated_parallel_overhead_ms,
        "parallel_cost_source": snapshot.packaging.parallel_cost_source,
        "parallel_overhead_samples": snapshot.packaging.parallel_overhead_samples,
        "current_package": current_package,
        "packages": packages,
    })
}

fn compact_task_projection(snapshot: &TaskSnapshot) -> Value {
    let goal = bounded_text(&snapshot.goal, MAX_COMPACT_GOAL_CHARS);
    let goal_truncated = goal.chars().count() < snapshot.goal.chars().count();
    let mut review = snapshot.review.clone();
    if let Some(paths) = review
        .get_mut("changed_paths")
        .and_then(Value::as_array_mut)
    {
        if paths.len() > 16 {
            paths.truncate(16);
            review["changed_paths_truncated"] = json!(true);
        }
    }
    let mut value = json!({
        "task_id": snapshot.task_id,
        "execution_id": snapshot.execution.execution_id,
        "previous_execution_id": snapshot.execution.previous_execution_id,
        "execution_state": snapshot.execution.state,
        "project": snapshot.project,
        "goal": goal,
        "status": snapshot.status,
        "current_step": snapshot.current_step,
        "total_steps": snapshot.total_steps,
        "completed_steps": snapshot.completed_steps,
        "plan": snapshot.plan,
        "cancel_requested": snapshot.cancel_requested,
        "started_at_ms": snapshot.started_at_ms,
        "finished_at_ms": snapshot.finished_at_ms,
        "duration_ms": snapshot.duration_ms,
        "counters": snapshot.counters,
        "validation": snapshot.validation,
        "review": review,
        "complexity": snapshot.complexity,
        "packaging": packaging_projection(snapshot),
        "workspace": snapshot.workspace,
        "steps": snapshot.steps.iter().map(compact_step_value).collect::<Vec<_>>(),
        "failure": snapshot.failure,
    });
    if goal_truncated {
        value["goal_truncated"] = json!(true);
    }
    omit_null_object_fields(&mut value);

    let max_bytes = snapshot.limits.max_result_bytes;
    if serialized_len(&value) <= max_bytes {
        return value;
    }

    value["result_truncated"] = json!(true);
    if let Some(steps) = value.get_mut("steps").and_then(Value::as_array_mut) {
        for step in steps {
            if let Some(object) = step.as_object_mut() {
                object.remove("summary");
            }
        }
    }
    if let Some(review) = value.get_mut("review").and_then(Value::as_object_mut) {
        review.remove("changed_paths");
        review.remove("diff_stat");
        review.insert("details_truncated".to_string(), json!(true));
    }
    if let Some(failure) = value.get_mut("failure").and_then(Value::as_object_mut) {
        for key in ["stdout_tail", "stderr_tail"] {
            if let Some(text) = failure.get(key).and_then(Value::as_str) {
                failure.insert(key.to_string(), json!(bounded_tail_text(text, 400)));
            }
        }
        if let Some(message) = failure.get("message").and_then(Value::as_str) {
            failure.insert("message".to_string(), json!(bounded_text(message, 400)));
        }
        failure.insert("diagnostics_truncated".to_string(), json!(true));
    }
    value["goal"] = json!(bounded_text(
        value
            .get("goal")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        256
    ));
    value["goal_truncated"] = json!(true);

    if serialized_len(&value) > max_bytes {
        if let Some(steps) = value.get_mut("steps").and_then(Value::as_array_mut) {
            if steps.len() > 8 {
                let start = steps.len() - 8;
                steps.drain(0..start);
                value["steps_truncated"] = json!(true);
            }
        }
    }
    if serialized_len(&value) > max_bytes {
        let failure = value.get("failure").cloned().map(|mut failure| {
            if let Some(object) = failure.as_object_mut() {
                object.retain(|key, _| {
                    matches!(
                        key.as_str(),
                        "kind"
                            | "category"
                            | "message"
                            | "step"
                            | "package"
                            | "step_kind"
                            | "exit_code"
                            | "job_id"
                            | "execution_state"
                            | "failure_kind"
                            | "error_kind"
                            | "reason_code"
                            | "stdout_truncated"
                            | "stderr_truncated"
                            | "output_truncated"
                            | "state_changed"
                            | "failed_check"
                            | "attempts"
                            | "retries"
                    )
                });
                if let Some(message) = object.get("message").and_then(Value::as_str) {
                    object.insert("message".to_string(), json!(bounded_text(message, 256)));
                }
                object.insert("diagnostics_truncated".to_string(), json!(true));
            }
            failure
        });
        let mut minimal = json!({
            "task_id": snapshot.task_id,
        "execution_id": snapshot.execution.execution_id,
        "previous_execution_id": snapshot.execution.previous_execution_id,
        "execution_state": snapshot.execution.state,
            "project": snapshot.project,
            "status": snapshot.status,
            "current_step": snapshot.current_step,
            "total_steps": snapshot.total_steps,
            "completed_steps": snapshot.completed_steps,
            "cancel_requested": snapshot.cancel_requested,
            "started_at_ms": snapshot.started_at_ms,
            "finished_at_ms": snapshot.finished_at_ms,
            "duration_ms": snapshot.duration_ms,
            "counters": snapshot.counters,
            "validation": snapshot.validation,
            "review": {
                "status": snapshot.review.get("status"),
                "changed_file_count": snapshot.review.get("changed_file_count")
            },
            "complexity": {
                "score": snapshot.complexity.score,
                "band": snapshot.complexity.band,
                "recommended_task_count": snapshot.complexity.recommended_task_count,
                "execution_task_count": snapshot.complexity.execution_task_count,
                "advisory_only": snapshot.complexity.advisory_only
            },
            "packaging": packaging_projection(snapshot),
            "workspace": snapshot.workspace,
            "failure": failure,
            "result_truncated": true,
        });
        omit_null_object_fields(&mut minimal);
        value = minimal;
    }
    value
}

fn task_storage_dir() -> Option<PathBuf> {
    #[cfg(test)]
    {
        // Unit/integration tests must never inherit Chadex's production
        // Application Support task registry. Tests that exercise persistence
        // call the explicit-root helpers below with a TempDir.
        return None;
    }
    #[cfg(not(test))]
    {
        let root = PathBuf::from(std::env::var_os("CHADEX_TASK_STATE_DIR")?);
        if root.as_os_str().is_empty() {
            return None;
        }
        if let Ok(metadata) = fs::symlink_metadata(&root) {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return None;
            }
        } else if fs::create_dir_all(&root).is_err() {
            return None;
        }
        Some(root)
    }
}

fn adaptive_execution_telemetry_path(root: &Path) -> PathBuf {
    root.join("performance").join("adaptive-execution.json")
}

fn load_adaptive_execution_telemetry_at(root: &Path) -> AdaptiveExecutionTelemetry {
    let path = adaptive_execution_telemetry_path(root);
    let Ok(metadata) = fs::symlink_metadata(&path) else {
        return AdaptiveExecutionTelemetry::default();
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 16 * 1024 {
        return AdaptiveExecutionTelemetry::default();
    }
    let Ok(bytes) = fs::read(path) else {
        return AdaptiveExecutionTelemetry::default();
    };
    let Ok(value) = serde_json::from_slice::<AdaptiveExecutionTelemetry>(&bytes) else {
        return AdaptiveExecutionTelemetry::default();
    };
    if value.version != 2
        || !value.parallel_overhead_base_ewma_ms.is_finite()
        || value.parallel_overhead_base_ewma_ms < PARALLEL_OVERHEAD_MIN_MS as f64
        || value.parallel_overhead_base_ewma_ms > PARALLEL_OVERHEAD_MAX_MS as f64
    {
        return AdaptiveExecutionTelemetry::default();
    }
    value
}

fn load_adaptive_execution_telemetry() -> AdaptiveExecutionTelemetry {
    task_storage_dir()
        .as_deref()
        .map(load_adaptive_execution_telemetry_at)
        .unwrap_or_default()
}

fn persist_adaptive_execution_telemetry_at(
    root: &Path,
    telemetry: &AdaptiveExecutionTelemetry,
) -> bool {
    let directory = root.join("performance");
    if fs::create_dir_all(&directory).is_err() {
        return false;
    }
    let Ok(bytes) = serde_json::to_vec(telemetry) else {
        return false;
    };
    write_private_file(&adaptive_execution_telemetry_path(root), &bytes).is_ok()
}

fn update_parallel_overhead_telemetry(
    mut telemetry: AdaptiveExecutionTelemetry,
    measured_overhead_ms: u64,
    package_count: usize,
) -> AdaptiveExecutionTelemetry {
    let extra =
        PARALLEL_EXTRA_PACKAGE_OVERHEAD_MS.saturating_mul(package_count.saturating_sub(1) as u64);
    let normalized_base = measured_overhead_ms
        .saturating_sub(extra)
        .clamp(PARALLEL_OVERHEAD_MIN_MS, PARALLEL_OVERHEAD_MAX_MS) as f64;
    telemetry.parallel_overhead_base_ewma_ms = if telemetry.parallel_samples == 0 {
        normalized_base
    } else {
        telemetry.parallel_overhead_base_ewma_ms * (1.0 - PARALLEL_OVERHEAD_EWMA_ALPHA)
            + normalized_base * PARALLEL_OVERHEAD_EWMA_ALPHA
    };
    telemetry.parallel_samples = telemetry.parallel_samples.saturating_add(1);
    telemetry.updated_at_ms = now_ms();
    telemetry
}

fn record_parallel_overhead_sample(
    measured_overhead_ms: u64,
    package_count: usize,
) -> (AdaptiveExecutionTelemetry, bool) {
    let Some(root) = task_storage_dir() else {
        return (AdaptiveExecutionTelemetry::default(), false);
    };
    let telemetry = update_parallel_overhead_telemetry(
        load_adaptive_execution_telemetry_at(&root),
        measured_overhead_ms,
        package_count,
    );
    let persisted = persist_adaptive_execution_telemetry_at(&root, &telemetry);
    (telemetry, persisted)
}

fn write_private_file(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if path.exists() && fs::symlink_metadata(path)?.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing task state symlink",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "task state path has no parent",
        )
    })?;
    let parent_metadata = fs::symlink_metadata(parent)?;
    if !parent_metadata.is_dir() || parent_metadata.file_type().is_symlink() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "refusing unsafe task state directory",
        ));
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("state");
    let temporary_path = parent.join(format!(".{name}.{}.tmp", Uuid::new_v4().simple()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary_path)?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    drop(file);
    if let Err(error) = fs::rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error);
    }
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn file_age_secs(path: &Path) -> Option<u64> {
    let modified = path.metadata().ok()?.modified().ok()?;
    SystemTime::now()
        .duration_since(modified)
        .ok()
        .map(|age| age.as_secs())
}

fn task_recovery_plan_path(root: &Path, task_id: &str) -> PathBuf {
    root.join("recovery").join(format!("{task_id}.json"))
}

fn persist_task_recovery_plan_at(root: &Path, plan: &TaskRecoveryPlan) -> bool {
    let directory = root.join("recovery");
    if fs::create_dir_all(&directory).is_err() {
        return false;
    }
    let path = task_recovery_plan_path(root, &plan.task_id);
    let Ok(bytes) = serde_json::to_vec(plan) else {
        return false;
    };
    write_private_file(&path, &bytes).is_ok()
}

fn load_task_recovery_plan_at(root: &Path, task_id: &str) -> Option<TaskRecoveryPlan> {
    let path = task_recovery_plan_path(root, task_id);
    let metadata = fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    let plan = serde_json::from_slice::<TaskRecoveryPlan>(&bytes).ok()?;
    (plan.version == 1 && plan.task_id == task_id && valid_task_id(task_id)).then_some(plan)
}

fn load_recovered_task_projections() -> HashMap<String, Value> {
    let Some(root) = task_storage_dir() else {
        return HashMap::new();
    };
    load_recovered_task_projections_at(&root)
}

fn load_recovered_task_projections_at(root: &Path) -> HashMap<String, Value> {
    let active_executions = execution::restore_receipts(root);
    let state_dir = root.join("state");
    let Ok(entries) = fs::read_dir(&state_dir) else {
        return HashMap::new();
    };
    let mut recovered = HashMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().and_then(|value| value.to_str()) == Some("latest.json")
            || path.extension().and_then(|value| value.to_str()) != Some("json")
        {
            continue;
        }
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(mut value) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let Some(task_id) = value
            .get("task_id")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        if !valid_task_id(&task_id) || active_executions.contains(&task_id) {
            continue;
        }
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let workspace_state = value
            .pointer("/workspace/state")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let interrupted = !is_terminal(status) && status != "interrupted";
        let preserved = workspace_state == "preserved";
        let active_inflight = workspace_state == "active" && !is_terminal(status);
        if !interrupted
            && !preserved
            && !active_inflight
            && status != "interrupted"
            && status != "unknown"
        {
            continue;
        }
        let plan_available = load_task_recovery_plan_at(root, &task_id).is_some();
        if interrupted {
            value["status"] = json!("interrupted");
            value["failure"] = json!({
                "kind": "runtime_interrupted",
                "category": "recovery",
                "message": "Chadex restarted before this task reached an authoritative terminal state"
            });
            if let Some(workspace) = value.get_mut("workspace").and_then(Value::as_object_mut) {
                workspace.insert("state".to_string(), json!("preserved"));
                workspace.insert("preserve_reason".to_string(), json!("runtime_interrupted"));
            }
        }
        value["recovery"] = json!({
            "available": true,
            "plan_available": plan_available,
            "actions": if plan_available && value.get("execution_state").and_then(Value::as_str) != Some("unknown") && value.get("status").and_then(Value::as_str) != Some("unknown") { json!(["retry", "discard"]) } else { json!(["discard"]) },
            "restored_from_disk": true,
        });
        let _ = write_private_file(
            &path,
            serde_json::to_vec(&value).unwrap_or_default().as_slice(),
        );
        recovered.insert(task_id, value);
    }
    let latest_path = state_dir.join("latest.json");
    if let Some(latest) = recovered.values().max_by_key(|value| {
        value
            .get("started_at_ms")
            .and_then(Value::as_i64)
            .unwrap_or(0)
    }) {
        if let Ok(bytes) = serde_json::to_vec(latest) {
            let _ = write_private_file(&latest_path, &bytes);
        }
    } else if active_executions.is_empty() {
        // Never leave a stale terminal projection behind when startup found
        // no recoverable task. The desktop helper treats latest.json as the
        // source for its progress card.
        let _ = fs::remove_file(latest_path);
    }
    recovered
}

fn persist_recovered_projection(task_id: &str, value: &Value) {
    let Some(root) = task_storage_dir() else {
        return;
    };
    let path = root.join("state").join(format!("{task_id}.json"));
    if let Ok(bytes) = serde_json::to_vec(value) {
        let _ = write_private_file(&path, &bytes);
    }
}

fn remove_task_persistence(task_id: &str) {
    let Some(root) = task_storage_dir() else {
        return;
    };
    let latest_path = root.join("state").join("latest.json");
    let latest_matches = fs::read(&latest_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| {
            value
                .get("task_id")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .is_some_and(|latest_task_id| latest_task_id == task_id);
    for path in [
        root.join("state").join(format!("{task_id}.json")),
        root.join("recovery").join(format!("{task_id}.json")),
        root.join("logs").join(format!("{task_id}.jsonl")),
    ] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => {}
        }
    }
    if latest_matches {
        let _ = fs::remove_file(latest_path);
    }
}

fn gc_task_storage() {
    let Some(root) = task_storage_dir() else {
        return;
    };

    let state_dir = root.join("state");
    if let Ok(entries) = fs::read_dir(&state_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().and_then(|value| value.to_str()) == Some("latest.json")
                || path.extension().and_then(|value| value.to_str()) != Some("json")
                || file_age_secs(&path).unwrap_or(0) < TASK_TERMINAL_TTL_SECS
            {
                continue;
            }
            let Ok(value) = fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
                .ok_or(())
            else {
                continue;
            };
            let status = value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let workspace_state = value
                .pointer("/workspace/state")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            if is_terminal(status)
                && !matches!(status, "unknown" | "interrupted")
                && workspace_state != "preserved"
            {
                if let Some(task_id) = value.get("task_id").and_then(Value::as_str) {
                    remove_task_persistence(task_id);
                }
            }
        }
    }

    for (path, value) in super::execution_workspace::load_workspace_manifests() {
        let state = value
            .get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        if state == "cleaned" && file_age_secs(&path).unwrap_or(0) >= CLEANED_WORKSPACE_TTL_SECS {
            let _ = fs::remove_file(path);
        }
    }

    let protected_task_ids = fs::read_dir(&state_dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.file_name().and_then(|value| value.to_str()) == Some("latest.json") {
                return None;
            }
            let bytes = fs::read(path).ok()?;
            let value = serde_json::from_slice::<Value>(&bytes).ok()?;
            let task_id = value.get("task_id").and_then(Value::as_str)?;
            let status = value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let workspace_state = value
                .pointer("/workspace/state")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            (!is_terminal(status)
                || matches!(status, "interrupted" | "unknown")
                || workspace_state == "preserved")
                .then(|| task_id.to_string())
        })
        .collect::<HashSet<_>>();

    let log_dir = root.join("logs");
    if let Ok(entries) = fs::read_dir(&log_dir) {
        let mut logs = entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let metadata = fs::symlink_metadata(&path).ok()?;
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return None;
                }
                let task_id = path.file_stem().and_then(|value| value.to_str())?;
                if protected_task_ids.contains(task_id) {
                    return None;
                }
                let modified = metadata.modified().ok()?;
                Some((path, metadata.len(), modified))
            })
            .collect::<Vec<_>>();
        for (path, _, _) in &logs {
            if file_age_secs(path).unwrap_or(0) >= TASK_TERMINAL_TTL_SECS {
                let _ = fs::remove_file(path);
            }
        }
        logs.retain(|(path, _, _)| path.exists());
        logs.sort_by_key(|(_, _, modified)| *modified);
        let mut total = logs.iter().map(|(_, size, _)| *size).sum::<u64>();
        for (path, size, _) in logs {
            if total <= TASK_STORAGE_LOG_TOTAL_MAX_BYTES {
                break;
            }
            if fs::remove_file(&path).is_ok() {
                total = total.saturating_sub(size);
            }
        }
    }

    for directory in [
        root.join("state"),
        root.join("recovery"),
        root.join("workspaces"),
    ] {
        if let Ok(entries) = fs::read_dir(directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default();
                if name.starts_with('.')
                    && name.contains(".tmp")
                    && file_age_secs(&path).unwrap_or(0) >= TASK_TEMP_FILE_TTL_SECS
                {
                    let _ = fs::remove_file(path);
                }
            }
        }
    }
}

fn persist_task_state(snapshot: &TaskSnapshot) {
    let Some(_global_guard) = TASK_STATE_PERSIST_LOCK
        .get_or_init(|| StdMutex::new(()))
        .lock()
        .ok()
    else {
        return;
    };
    let Some(root) = task_storage_dir() else {
        return;
    };
    let state_dir = root.join("state");
    if fs::create_dir_all(&state_dir).is_err() {
        return;
    }
    let mut value = compact_task_projection(snapshot);
    // source_path is private local recovery metadata. Keep it out of normal
    // tool projections, but persist it so the desktop helper can avoid showing
    // a recovered task for an unrelated selected project.
    value["source_path"] = json!(snapshot.source_path);
    let Ok(bytes) = serde_json::to_vec(&value) else {
        return;
    };
    let _ = write_private_file(
        &state_dir.join(format!("{}.json", snapshot.task_id)),
        &bytes,
    );
    let _ = write_private_file(&state_dir.join("latest.json"), &bytes);
}

fn persist_raw_task_result(
    task_id: &str,
    step_index: usize,
    kind: &str,
    attempt: usize,
    result: &ToolResult,
) -> bool {
    let Some(root) = task_storage_dir() else {
        return false;
    };
    let log_dir = root.join("logs");
    if fs::create_dir_all(&log_dir).is_err() {
        return false;
    }
    let path = log_dir.join(format!("{task_id}.jsonl"));
    if path
        .metadata()
        .ok()
        .is_some_and(|metadata| metadata.len() >= LOCAL_TASK_LOG_MAX_BYTES)
    {
        return false;
    }
    if path.exists()
        && fs::symlink_metadata(&path)
            .ok()
            .is_some_and(|metadata| metadata.file_type().is_symlink())
    {
        return false;
    }
    let Ok(mut encoded) = serde_json::to_vec(&json!({
        "step": step_index,
        "kind": kind,
        "attempt": attempt,
        "result": result,
    })) else {
        return false;
    };
    encoded.push(b'\n');
    let current_len = path.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    if current_len.saturating_add(encoded.len() as u64) > LOCAL_TASK_LOG_MAX_BYTES {
        return false;
    }
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .and_then(|mut file| file.write_all(&encoded))
        .is_ok()
}

fn persist_task_step_result(
    control: &TaskControl,
    task_id: &str,
    step_index: usize,
    kind: &str,
    attempt: usize,
    result: &ToolResult,
) -> bool {
    let Ok(_guard) = control.raw_log.lock() else {
        return false;
    };
    persist_raw_task_result(task_id, step_index, kind, attempt, result)
}

fn first_failed_batch_item(result: &ToolResult) -> Option<(usize, &Value)> {
    result
        .output
        .get("items")
        .and_then(Value::as_array)?
        .iter()
        .enumerate()
        .find(|(_, item)| item.get("success").and_then(Value::as_bool) == Some(false))
}

fn normalize_read_only_batch_result(kind: &str, mut result: ToolResult) -> ToolResult {
    if !matches!(kind, "search" | "read") || !result.success {
        return result;
    }
    let failed_count = result
        .output
        .get("failed_count")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if failed_count == 0 {
        return result;
    }
    let message = first_failed_batch_item(&result)
        .and_then(|(_, item)| item.get("error").and_then(Value::as_str))
        .map(bounded_message)
        .unwrap_or_else(|| format!("{kind} step contained {failed_count} failed item(s)"));
    result.success = false;
    result.error = Some(message);
    result
}

fn result_markers(result: &ToolResult) -> Vec<&str> {
    let mut markers = [
        "error_kind",
        "failure_kind",
        "reason_code",
        "execution_state",
    ]
    .into_iter()
    .filter_map(|key| result.output.get(key).and_then(Value::as_str))
    .collect::<Vec<_>>();
    if let Some((_, item)) = first_failed_batch_item(result) {
        if let Some(output) = item.get("output") {
            markers.extend(
                [
                    "error_kind",
                    "failure_kind",
                    "reason_code",
                    "execution_state",
                ]
                .into_iter()
                .filter_map(|key| output.get(key).and_then(Value::as_str)),
            );
        }
    }
    markers
}

fn retryable_read_only_failure(kind: &str, result: &ToolResult) -> bool {
    if !matches!(kind, "search" | "read") || result.success {
        return false;
    }
    if result.output.get("state_changed").and_then(Value::as_bool) == Some(true) {
        return false;
    }
    result_markers(result).into_iter().any(|marker| {
        marker == "runner_unavailable"
            || marker == "runner_disconnected"
            || marker == "runner_timeout"
            || marker == "runner_transport_disconnected"
            || marker == "agent_unavailable"
            || marker == "request_timeout"
            || (marker.contains("runner")
                && (marker.contains("unavailable")
                    || marker.contains("disconnect")
                    || marker.contains("timeout")))
    })
}

fn should_retry_read_only_failure(kind: &str, result: &ToolResult, retries: usize) -> bool {
    !result_markers(result)
        .iter()
        .any(|marker| matches!(*marker, "unknown" | "outcome_unknown"))
        && retries < MAX_READ_ONLY_RETRIES
        && retryable_read_only_failure(kind, result)
}

fn failure_category(kind: &str, result: &ToolResult) -> &'static str {
    let markers = result_markers(result);
    let has = |needle: &str| markers.iter().any(|marker| marker.contains(needle));
    if kind == "validate" {
        "validation"
    } else if has("timeout") {
        "timeout"
    } else if markers.iter().any(|marker| *marker == "outcome_unknown")
        && (kind == "edit"
            || result.output.get("state_changed").and_then(Value::as_bool) != Some(false))
    {
        "ambiguous_mutation"
    } else if has("stale") || has("revision") {
        "stale_write"
    } else if has("agent_unavailable")
        || (has("runner") && (has("unavailable") || has("disconnect") || has("transport")))
    {
        "runner_disconnect"
    } else if has("permission") || has("scope") {
        "permission"
    } else if has("path") || has("limit") {
        "safety_block"
    } else {
        "step"
    }
}

fn compact_failure_details(
    result: &ToolResult,
    summary: Option<&Value>,
    step: usize,
    kind: &str,
    attempts: usize,
    retries: usize,
    raw_log_retained: bool,
) -> Value {
    let mut failure = json!({
        "kind": "step_failed",
        "category": failure_category(kind, result),
        "message": bounded_message(result.error.as_deref().unwrap_or("nested task step failed")),
        "step": step,
        "step_kind": kind,
        "attempts": attempts,
        "retries": retries,
        "raw_log_retained": raw_log_retained,
    });
    for key in [
        "exit_code",
        "job_id",
        "execution_state",
        "failure_kind",
        "error_kind",
        "reason_code",
        "stdout_truncated",
        "stderr_truncated",
        "output_truncated",
        "state_changed",
        "command_started",
        "command_completed",
    ] {
        if let Some(value) = result.output.get(key) {
            failure[key] = value.clone();
        }
    }
    if let Some(failed_check) = summary.and_then(|value| value.get("failed_check")) {
        failure["failed_check"] = failed_check.clone();
    }
    if let Some((position, item)) = first_failed_batch_item(result) {
        failure["failed_item"] = item
            .get("index")
            .cloned()
            .unwrap_or_else(|| json!(position));
        if let Some(error) = item.get("error").and_then(Value::as_str) {
            failure["item_error"] = json!(bounded_message(error));
        }
        if let Some(output) = item.get("output") {
            for key in [
                "exit_code",
                "job_id",
                "execution_state",
                "failure_kind",
                "error_kind",
                "reason_code",
                "stdout_truncated",
                "stderr_truncated",
                "output_truncated",
                "state_changed",
                "command_started",
                "command_completed",
            ] {
                if failure.get(key).is_none() {
                    if let Some(value) = output.get(key) {
                        failure[key] = value.clone();
                    }
                }
            }
            for key in ["stdout_tail", "stderr_tail"] {
                if failure.get(key).is_none() {
                    if let Some(text) = output.get(key).and_then(Value::as_str) {
                        failure[key] = json!(bounded_tail_text(text, MAX_FAILURE_STDIO_CHARS));
                    }
                }
            }
        }
    }
    for key in ["stdout_tail", "stderr_tail"] {
        if let Some(text) = result.output.get(key).and_then(Value::as_str) {
            failure[key] = json!(bounded_tail_text(text, MAX_FAILURE_STDIO_CHARS));
        }
    }
    omit_null_object_fields(&mut failure);
    failure
}

fn task_value(control: &TaskControl) -> Result<Value, String> {
    let snapshot = control
        .snapshot
        .lock()
        .map_err(|_| "task snapshot lock poisoned")?
        .clone();
    Ok(compact_task_projection(&snapshot))
}

fn execute_task_result(control: &TaskControl) -> ToolResult {
    let value = match task_value(control) {
        Ok(value) => value,
        Err(error) => return ToolResult::err(error),
    };
    let status = value
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("failed");
    if status == "completed" {
        return ToolResult::ok(value);
    }
    let message = value
        .pointer("/failure/message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            if status == "cancelled" {
                "task cancelled"
            } else {
                "task did not complete"
            }
        })
        .to_string();
    ToolResult::err_with_output(message, value)
}

fn update_snapshot(control: &TaskControl, f: impl FnOnce(&mut TaskSnapshot)) -> Result<(), String> {
    // One lock covers transition, durable receipt and publication. Observers
    // cannot see success before the exact projected result has been synced.
    let mut current = control
        .snapshot
        .lock()
        .map_err(|_| "task snapshot lock poisoned")?;
    if current.execution.state.terminal() {
        return Ok(());
    }
    let mut snapshot = current.clone();
    f(&mut snapshot);
    let terminal = is_terminal(&snapshot.status);
    if snapshot.execution.state == ExecutionState::Queued
        && (terminal || snapshot.status == "running")
    {
        snapshot
            .execution
            .state
            .transition(ExecutionState::Running)?;
    }
    if terminal {
        let workspace_uncertain = snapshot.execution.effects_possible
            && snapshot
                .failure
                .as_ref()
                .and_then(|f| f.get("category"))
                .and_then(Value::as_str)
                == Some("workspace");
        let state = if snapshot.execution.outcome_uncertain
            || workspace_uncertain
            || snapshot
                .failure
                .as_ref()
                .and_then(|f| f.get("kind"))
                .and_then(Value::as_str)
                == Some("package_join_failed")
            || (snapshot.status == "interrupted" && snapshot.execution.effects_possible)
        {
            ExecutionState::Unknown
        } else {
            match snapshot.status.as_str() {
                "completed" => ExecutionState::Succeeded,
                "cancelled" => ExecutionState::Cancelled,
                "interrupted" => ExecutionState::Interrupted,
                "unknown" => ExecutionState::Unknown,
                _ => ExecutionState::Failed,
            }
        };
        if state != ExecutionState::Succeeded
            && snapshot.workspace.get("mode").and_then(Value::as_str) != Some("none")
            && snapshot.workspace.get("state").and_then(Value::as_str) != Some("cleaned")
        {
            snapshot.workspace["state"] = json!("preserved");
        }
        snapshot.execution.state.transition(state)?;
        if state == ExecutionState::Unknown {
            snapshot.status = "unknown".into();
        }
    }
    if terminal
        || snapshot.execution.state != current.execution.state
        || snapshot.execution.effects_possible != current.execution.effects_possible
    {
        let mut result = compact_task_projection(&snapshot);
        result["source_path"] = json!(snapshot.source_path);
        if let Err(error) = snapshot.execution.persist(result) {
            // Disk still contains queued/running, never a false durable success.
            snapshot.execution.state = ExecutionState::Unknown;
            snapshot.status = "unknown".into();
            snapshot.finished_at_ms = Some(now_ms());
            snapshot.failure = Some(
                json!({"kind":"execution_receipt_write_failed", "category":"durability", "message":bounded_message(&error)}),
            );
            *current = snapshot;
            persist_task_state(&current);
            return Err(error);
        }
    }
    *current = snapshot;
    persist_task_state(&current);
    Ok(())
}

fn compact_result_summary(kind: &str, result: &ToolResult) -> Option<Value> {
    let output = &result.output;
    match kind {
        "search" | "read" => Some(json!({
            "requested_count": output.get("requested_count"),
            "returned_count": output.get("returned_count"),
            "succeeded_count": output.get("succeeded_count"),
            "failed_count": output.get("failed_count"),
            "output_truncated": output.get("output_truncated"),
        })),
        "edit" => Some(json!({
            "applied_count": output.get("applied_count"),
            "changed": output.get("changed"),
            "changed_paths": output.get("changed_paths"),
            "state_changed": output.get("state_changed"),
        })),
        "run_process" => Some(json!({
            "execution_state": output.get("execution_state"),
            "command_started": output.get("command_started"),
            "command_completed": output.get("command_completed"),
            "command_ok": output.get("command_ok"),
            "exit_code": output.get("exit_code"),
            "job_id": output.get("job_id"),
        })),
        _ => None,
    }
}

fn result_requires_observation(result: &ToolResult) -> bool {
    result
        .output
        .get("job_id")
        .and_then(Value::as_str)
        .is_some()
        && result
            .output
            .get("command_completed")
            .and_then(Value::as_bool)
            != Some(true)
}

fn review_summary(result: &ToolResult) -> Value {
    let changed_paths = result
        .output
        .get("files")
        .and_then(Value::as_array)
        .map(|files| {
            files
                .iter()
                .filter_map(|file| file.get("path").and_then(Value::as_str))
                .take(64)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let changed_file_count = result
        .output
        .get("files_total")
        .and_then(Value::as_u64)
        .unwrap_or(changed_paths.len() as u64);
    json!({
        "clean": result.output.get("clean"),
        "changed_file_count": changed_file_count,
        "changed_paths": changed_paths,
        "diff_stat": result.output.get("diff_stat"),
        "conflicted": result.output.pointer("/counts/conflicted"),
    })
}

fn omit_null_object_fields(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            fields.retain(|_, value| !value.is_null());
            for value in fields.values_mut() {
                omit_null_object_fields(value);
            }
        }
        Value::Array(items) => {
            for value in items {
                omit_null_object_fields(value);
            }
        }
        _ => {}
    }
}

impl ToolRuntime {
    fn invoke_task_tool<'a>(
        &'a self,
        project: &'a str,
        session_id: Option<&'a str>,
        tool_name: &'a str,
        arguments: Value,
        auth: Option<&'a AuthContext>,
        transport: sessions::SessionTransport,
    ) -> Pin<Box<dyn Future<Output = ToolResult> + Send + 'a>> {
        Box::pin(async move {
            let mut arguments = arguments;
            omit_null_object_fields(&mut arguments);
            let Some(mut arguments) = arguments.as_object().cloned() else {
                return ToolResult::err("task nested tool arguments must be an object");
            };
            arguments.insert("project".to_string(), Value::String(project.to_string()));
            if let Some(session_id) = session_id {
                arguments.insert(
                    "session_id".to_string(),
                    Value::String(session_id.to_string()),
                );
            }
            let transport = match transport {
                sessions::SessionTransport::Api => ToolTransport::Api,
                sessions::SessionTransport::Mcp => ToolTransport::Mcp,
            };
            // Re-enter the canonical kernel on a child task instead of recursively
            // polling the full ToolRuntime future on the current stack. JoinSet aborts
            // the child if this parent future is dropped, so no detached nested task
            // survives request cancellation.
            let runtime = self.clone();
            let auth = auth.cloned();
            let tool_name = tool_name.to_string();
            let arguments = Value::Object(arguments);
            let mut nested = JoinSet::new();
            nested.spawn(async move {
                runtime
                    .call_tool_with_context(
                        ToolCallRequest {
                            tool_name,
                            arguments,
                        },
                        ToolCallContext {
                            transport,
                            session_id: None,
                            auth: auth.as_ref(),
                            window: None,
                            record_oauth_scope_denials: true,
                            host_file_import_trust: HostFileImportTrust::Untrusted,
                        },
                    )
                    .await
            });
            let outcome = match nested.join_next().await {
                Some(Ok(outcome)) => outcome,
                Some(Err(error)) => {
                    return ToolResult::err(format!("task nested tool join failed: {error}"))
                }
                None => return ToolResult::err("task nested tool ended without a result"),
            };
            if let Some(result) = outcome.result {
                return result;
            }
            let message = match outcome.error_status {
                Some(ToolCallErrorStatus::InvalidArguments { message }) => message,
                Some(ToolCallErrorStatus::InsufficientScope { description, .. }) => description,
                None => return ToolResult::err("task nested tool returned no result"),
            };
            ToolResult::err_with_output(message, json!({"state_changed":false}))
        })
    }

    async fn collect_graphify_dependency_evidence(
        &self,
        project: &str,
        session_id: Option<&str>,
        steps: &[ChadexTaskStep],
        packaging: &TaskPackagingPlan,
        complexity_band: &str,
        requires_mutation_boundary: bool,
        policy: &NormalizedTaskPolicy,
        auth: Option<&AuthContext>,
        transport: sessions::SessionTransport,
    ) -> (GraphifyDependencyEvidence, u64) {
        if !requires_mutation_boundary {
            return (empty_graphify_evidence("skipped_read_only"), 0);
        }
        if packaging.effective_package_count <= 1 {
            return (empty_graphify_evidence("skipped_single_package"), 0);
        }
        if !should_use_graphify(packaging, complexity_band) {
            return (
                empty_graphify_evidence("skipped_below_graphify_threshold"),
                0,
            );
        }
        if !policy.allowed_path_prefixes.is_empty() {
            return (empty_graphify_evidence("skipped_path_scope"), 0);
        }
        if !policy
            .allowed_operations
            .iter()
            .any(|operation| operation == "run_process")
        {
            return (empty_graphify_evidence("skipped_process_policy"), 0);
        }

        let payload = dependency_payload(steps, packaging);
        let candidate_count = payload
            .get("packages")
            .and_then(Value::as_array)
            .map(|packages| {
                packages
                    .iter()
                    .filter_map(|package| package.get("files").and_then(Value::as_array))
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<HashSet<_>>()
                    .len()
            })
            .unwrap_or(0);
        if candidate_count < 2 {
            return (empty_graphify_evidence("skipped_insufficient_files"), 0);
        }

        let started = Instant::now();
        let graphify_timeout_secs = policy.timeout_secs.min(5).max(1);
        let _executor_permit = self.chadex_tasks.scheduler().acquire_executor().await;
        let result = self
            .invoke_task_tool(
                project,
                session_id,
                "run_process",
                json!({
                    "executable": "python3",
                    "args": ["-c", GRAPHIFY_DEPENDENCY_SCRIPT, payload.to_string()],
                    "timeout_secs": graphify_timeout_secs,
                    "sync_wait_secs": graphify_timeout_secs,
                    "cwd": ".",
                    "purpose": ExecutionPurpose::Diagnostic,
                }),
                auth,
                transport,
            )
            .await;
        let planning_ms = elapsed_ms(started);
        if !result.success || result_requires_observation(&result) {
            return (empty_graphify_evidence("analysis_unavailable"), planning_ms);
        }
        let stdout = result
            .output
            .get("stdout_tail")
            .or_else(|| result.output.get("stdout"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let Some(line) = stdout.lines().rev().find(|line| !line.trim().is_empty()) else {
            return (empty_graphify_evidence("analysis_empty"), planning_ms);
        };
        match serde_json::from_str::<GraphifyDependencyEvidence>(line.trim()) {
            Ok(evidence) => (evidence, planning_ms),
            Err(_) => (empty_graphify_evidence("analysis_invalid"), planning_ms),
        }
    }

    async fn dispatch_task_step(
        &self,
        control: &TaskControl,
        project: &str,
        session_id: Option<String>,
        step: ChadexTaskStep,
        policy: &NormalizedTaskPolicy,
        remaining: Duration,
        auth: Option<&AuthContext>,
        transport: sessions::SessionTransport,
    ) -> (ToolResult, Option<Value>, bool) {
        let kind = task_step_kind(&step);
        let outcome = self
            .dispatch_task_step_inner(
                project, session_id, step, policy, remaining, auth, transport,
            )
            .await;
        if (kind != "edit" || outcome.2) && uncertain_result(kind, &outcome.0) {
            let _ = update_snapshot(control, |s| s.execution.outcome_uncertain = true);
        }
        outcome
    }

    async fn dispatch_task_step_inner(
        &self,
        project: &str,
        session_id: Option<String>,
        step: ChadexTaskStep,
        policy: &NormalizedTaskPolicy,
        remaining: Duration,
        auth: Option<&AuthContext>,
        transport: sessions::SessionTransport,
    ) -> (ToolResult, Option<Value>, bool) {
        let kind = task_step_kind(&step);
        match step {
            ChadexTaskStep::Search {
                queries,
                max_result_bytes,
            } => {
                let result = self
                    .invoke_task_tool(
                        project,
                        session_id.as_deref(),
                        "search_project_texts",
                        json!({
                            "queries": queries,
                            "max_result_bytes": max_result_bytes
                                .unwrap_or(policy.max_result_bytes)
                                .min(policy.max_result_bytes),
                        }),
                        auth,
                        transport,
                    )
                    .await;
                let result = normalize_read_only_batch_result(kind, result);
                let summary = compact_result_summary(kind, &result);
                (result, summary, false)
            }
            ChadexTaskStep::Read {
                items,
                with_line_numbers,
                max_result_bytes,
            } => {
                let result = self
                    .invoke_task_tool(
                        project,
                        session_id.as_deref(),
                        "read_files",
                        json!({
                            "items": items,
                            "with_line_numbers": with_line_numbers,
                            "include_read_revision": false,
                            "max_result_bytes": max_result_bytes
                                .unwrap_or(policy.max_result_bytes)
                                .min(policy.max_result_bytes),
                        }),
                        auth,
                        transport,
                    )
                    .await;
                let result = normalize_read_only_batch_result(kind, result);
                let summary = compact_result_summary(kind, &result);
                (result, summary, false)
            }
            ChadexTaskStep::Edit {
                mut changes,
                dry_run,
            } => {
                let missing = changes
                    .iter()
                    .enumerate()
                    .filter(|(_, change)| {
                        change.kind != ApplyFileChangeKind::Create
                            && change.expected_read_revision.is_none()
                    })
                    .map(|(index, change)| (index, change.path.clone()))
                    .collect::<Vec<_>>();
                if !missing.is_empty() {
                    let items = missing
                        .iter()
                        .map(|(_, path)| ReadFilesItem {
                            path: path.clone(),
                            start_line: Some(1),
                            limit: Some(1),
                            expected_read_revision: None,
                        })
                        .collect::<Vec<_>>();
                    let guard_read = self
                        .invoke_task_tool(
                            project,
                            session_id.as_deref(),
                            "read_files",
                            json!({
                                "items": items,
                                "with_line_numbers": false,
                                "include_read_revision": true,
                                "max_result_bytes": policy.max_result_bytes,
                            }),
                            auth,
                            transport,
                        )
                        .await;
                    if !guard_read.success {
                        return (guard_read, Some(json!({"guard_read_failed":true})), false);
                    }
                    let Some(items) = guard_read.output.get("items").and_then(Value::as_array)
                    else {
                        return (
                            ToolResult::err("task guard read returned no items"),
                            Some(json!({"guard_read_failed":true})),
                            false,
                        );
                    };
                    for (position, (change_index, _)) in missing.iter().enumerate() {
                        let revision = items
                            .get(position)
                            .and_then(|item| item.get("output"))
                            .and_then(|output| output.get("read_revision"))
                            .and_then(Value::as_u64);
                        let Some(revision) = revision else {
                            return (
                                ToolResult::err("task guard read did not return a read_revision"),
                                Some(json!({"guard_read_failed":true,"change_index":change_index})),
                                false,
                            );
                        };
                        changes[*change_index].expected_read_revision = Some(revision);
                    }
                }
                let result = self
                    .invoke_task_tool(
                        project,
                        session_id.as_deref(),
                        "apply_text_edits",
                        json!({"changes": changes, "dry_run": dry_run}),
                        auth,
                        transport,
                    )
                    .await;
                let summary = compact_result_summary(kind, &result);
                (result, summary, !dry_run.unwrap_or(false))
            }
            ChadexTaskStep::RunProcess {
                executable,
                args,
                stdin,
                cwd,
                timeout_secs,
                purpose,
            } => {
                let remaining_secs = remaining.as_secs().max(1);
                let timeout_secs = timeout_secs.unwrap_or(remaining_secs).min(remaining_secs);
                let result = self
                    .invoke_task_tool(
                        project,
                        session_id.as_deref(),
                        "run_process",
                        json!({
                            "executable": executable,
                            "args": args,
                            "stdin": stdin,
                            "timeout_secs": timeout_secs,
                            "sync_wait_secs": timeout_secs.min(60).max(1),
                            "cwd": cwd,
                            "purpose": purpose,
                        }),
                        auth,
                        transport,
                    )
                    .await;
                let summary = compact_result_summary(kind, &result);
                (result, summary, false)
            }
            ChadexTaskStep::Validate { checks } => {
                let mut passed = 0usize;
                for (index, check) in checks.into_iter().enumerate() {
                    let remaining_secs = remaining.as_secs().max(1);
                    let timeout_secs = check
                        .timeout_secs
                        .unwrap_or(remaining_secs)
                        .min(remaining_secs);
                    let result = self
                        .invoke_task_tool(
                            project,
                            session_id.as_deref(),
                            "run_process",
                            json!({
                                "executable": check.executable,
                                "args": check.args,
                                "stdin": check.stdin,
                                "timeout_secs": timeout_secs,
                                "sync_wait_secs": timeout_secs.min(60).max(1),
                                "cwd": check.cwd,
                                "purpose": ExecutionPurpose::Validation,
                            }),
                            auth,
                            transport,
                        )
                        .await;
                    if result_requires_observation(&result) {
                        return (
                            ToolResult::err_with_output(
                                "validation requires external Job observation",
                                json!({"check_index":index,"job_id":result.output.get("job_id")}),
                            ),
                            Some(
                                json!({"checks_passed":passed,"checks_failed":0,"blocked_on_job":true}),
                            ),
                            false,
                        );
                    }
                    if !result.success {
                        return (
                            result,
                            Some(
                                json!({"checks_passed":passed,"checks_failed":1,"failed_check":index}),
                            ),
                            false,
                        );
                    }
                    passed += 1;
                }
                (
                    ToolResult::ok(json!({"checks_passed":passed})),
                    Some(json!({"checks_passed":passed,"checks_failed":0})),
                    false,
                )
            }
            ChadexTaskStep::Review {
                include_diff,
                max_hunks,
                max_hunk_lines,
            } => {
                let result = self
                    .invoke_task_tool(
                        project,
                        session_id.as_deref(),
                        "show_changes",
                        json!({
                            "include_diff": include_diff.unwrap_or(true),
                            "max_hunks": max_hunks,
                            "max_hunk_lines": max_hunk_lines,
                            "session_event_limit": 0,
                        }),
                        auth,
                        transport,
                    )
                    .await;
                let summary = result.success.then(|| review_summary(&result));
                (result, summary, false)
            }
        }
    }

    async fn execute_package_range(
        &self,
        task_id: String,
        package_index: usize,
        workspace: ExecutionWorkspace,
        steps: Arc<Vec<ChadexTaskStep>>,
        start_step: usize,
        end_step: usize,
        policy: NormalizedTaskPolicy,
        deadline: Instant,
        session_id: Option<String>,
        auth: Option<AuthContext>,
        transport: sessions::SessionTransport,
        control: Arc<TaskControl>,
    ) -> (ExecutionWorkspace, PackageExecutionOutcome) {
        let mut outcome = PackageExecutionOutcome::default();
        for index in start_step..=end_step {
            if control.cancel_requested.load(Ordering::SeqCst)
                || control
                    .snapshot
                    .lock()
                    .map(|s| s.execution.outcome_uncertain)
                    .unwrap_or(true)
            {
                outcome.status = "cancelled".to_string();
                outcome.failure = Some(json!({
                    "kind": "task_cancelled",
                    "category": "cancelled",
                    "message": "package execution was cancelled before the next step",
                    "step": index,
                    "package": package_index,
                }));
                return (workspace, outcome);
            }
            if Instant::now() >= deadline {
                outcome.status = "blocked".to_string();
                outcome.failure = Some(json!({
                    "kind": "task_timeout",
                    "category": "timeout",
                    "message": "task runtime budget exhausted before the next package step",
                    "step": index,
                    "package": package_index,
                }));
                return (workspace, outcome);
            }
            let step = steps[index].clone();
            let kind = task_step_kind(&step).to_string();
            let step_started = Instant::now();
            let mut attempts = 0usize;
            let mut retries = 0usize;
            let mut raw_log_retained = false;
            let mut result_bytes_before = 0usize;
            let mut result_bytes_after = 0usize;
            let (result, summary, mutated) = loop {
                attempts += 1;
                let _executor_permit = self.chadex_tasks.scheduler().acquire_executor().await;
                let nested = self
                    .dispatch_task_step(
                        &control,
                        &workspace.execution_project,
                        session_id.clone(),
                        step.clone(),
                        &policy,
                        deadline.saturating_duration_since(Instant::now()),
                        auth.as_ref(),
                        transport.clone(),
                    )
                    .await;
                result_bytes_before = result_bytes_before.saturating_add(
                    serde_json::to_vec(&nested.0)
                        .map(|bytes| bytes.len())
                        .unwrap_or(0),
                );
                result_bytes_after = result_bytes_after
                    .saturating_add(nested.1.as_ref().map(serialized_len).unwrap_or(0));
                raw_log_retained |=
                    persist_task_step_result(&control, &task_id, index, &kind, attempts, &nested.0);
                if should_retry_read_only_failure(&kind, &nested.0, retries)
                    && !control.cancel_requested.load(Ordering::SeqCst)
                    && Instant::now() < deadline
                {
                    retries += 1;
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    continue;
                }
                break nested;
            };
            outcome.retries += retries;
            outcome.result_bytes_before = outcome
                .result_bytes_before
                .saturating_add(result_bytes_before);
            outcome.result_bytes_after = outcome
                .result_bytes_after
                .saturating_add(result_bytes_after);
            let duration_ms = elapsed_ms(step_started);
            if kind == "validate" {
                outcome.validation_seen = true;
                outcome.validation_ok = result.success;
            }
            if kind == "review" {
                outcome.review_seen = result.success;
            }
            if mutated && result.success {
                outcome.completed_mutations += 1;
            }
            let summary_for_failure = summary.clone();
            outcome.steps.push(TaskStepSummary {
                index,
                kind: kind.clone(),
                status: if result.success {
                    "completed".to_string()
                } else if result_requires_observation(&result) {
                    "blocked".to_string()
                } else {
                    "failed".to_string()
                },
                duration_ms,
                attempts,
                retries,
                result_bytes_before,
                result_bytes_after,
                summary: summary.clone(),
            });
            if result_requires_observation(&result) {
                outcome.status = "blocked".to_string();
                let mut failure = json!({
                    "kind": "job_observation_required",
                    "category": "blocked_job",
                    "message": "a nested process outlived the synchronous package step",
                    "step": index,
                    "package": package_index,
                    "step_kind": kind,
                    "attempts": attempts,
                    "retries": retries,
                    "raw_log_retained": raw_log_retained,
                    "job_id": result.output.get("job_id"),
                    "execution_state": result.output.get("execution_state"),
                });
                omit_null_object_fields(&mut failure);
                outcome.failure = Some(failure);
                return (workspace, outcome);
            }
            if !result.success {
                let failure_status = if kind == "validate" {
                    "failed_validation"
                } else {
                    "failed"
                };
                let mut failure = compact_failure_details(
                    &result,
                    summary_for_failure.as_ref(),
                    index,
                    &kind,
                    attempts,
                    retries,
                    raw_log_retained,
                );
                failure["package"] = json!(package_index);
                outcome.status = failure_status.to_string();
                outcome.failure = Some(failure);
                return (workspace, outcome);
            }
            outcome
                .steps
                .last_mut()
                .expect("package step was just pushed")
                .status = "completed".to_string();
            outcome.completed_steps += 1;
            if kind == "review" {
                let changed = summary
                    .as_ref()
                    .and_then(|value| value.get("changed_file_count"))
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as usize;
                if changed > policy.max_changed_files {
                    outcome.status = "blocked".to_string();
                    outcome.failure = Some(json!({
                        "kind": "changed_file_limit_exceeded",
                        "category": "safety_block",
                        "message": "package review exceeds task max_changed_files",
                        "package": package_index,
                    }));
                    return (workspace, outcome);
                }
            }
        }
        outcome.status = "completed".to_string();
        (workspace, outcome)
    }

    async fn execute_parallel_package_task(
        &self,
        resolved: ResolvedProject,
        task_id: String,
        control: Arc<TaskControl>,
        steps: Vec<ChadexTaskStep>,
        packaging: TaskPackagingPlan,
        policy: NormalizedTaskPolicy,
        acceptance: NormalizedAcceptance,
        session_id: Option<String>,
        auth: Option<&AuthContext>,
        transport: sessions::SessionTransport,
        started: Instant,
        deadline: Instant,
        first_workspace: ExecutionWorkspace,
        initial_workspace_prepare_wall_ms: u64,
        planned_mutations: usize,
        planned_changed_files: usize,
    ) -> ToolResult {
        let steps = Arc::new(steps);
        // A caller session is bound to the selected source Project. Detached
        // package/integration Projects must not reuse that identity, or the
        // canonical session guard would reject otherwise safe nested work.
        let execution_session_id =
            if first_workspace.execution_project == first_workspace.source_project {
                session_id
            } else {
                None
            };
        let base_sha = match first_workspace.base_sha.clone() {
            Some(base_sha) => base_sha,
            None => {
                let _ = update_snapshot(&control, |snapshot| {
                    snapshot.status = "blocked".to_string();
                    snapshot.failure = Some(json!({
                        "kind": "package_workspace_missing_base",
                        "category": "workspace",
                        "message": "parallel package execution requires an isolated base commit",
                    }));
                });
                return execute_task_result(&control);
            }
        };
        let source_head_sha = first_workspace.source_head_sha.clone();
        let source_tree_sha = first_workspace.source_tree_sha.clone();
        let source_index_tree_sha = first_workspace.source_index_tree_sha.clone();
        let package_workspace_prepare_started = Instant::now();
        let package_count = packaging.packages.len();
        let mut prepared_workspaces = vec![None; package_count];
        prepared_workspaces[0] = Some(first_workspace);
        let mut prepare_jobs = JoinSet::new();
        for package_index in 1..package_count {
            let runtime = self.clone();
            let resolved = resolved.clone();
            let task_id = task_id.clone();
            let base_sha = base_sha.clone();
            let source_head_sha = source_head_sha.clone();
            let source_tree_sha = source_tree_sha.clone();
            let source_index_tree_sha = source_index_tree_sha.clone();
            let auth = auth.cloned();
            prepare_jobs.spawn(async move {
                let result = runtime
                    .prepare_derived_execution_workspace(
                        &resolved,
                        &task_id,
                        IsolationMode::Package,
                        &base_sha,
                        source_head_sha,
                        source_tree_sha,
                        source_index_tree_sha,
                        auth.as_ref(),
                    )
                    .await;
                (package_index, result)
            });
        }
        let mut prepare_failure: Option<(usize, String)> = None;
        while let Some(joined) = prepare_jobs.join_next().await {
            match joined {
                Ok((package_index, Ok(workspace))) => {
                    prepared_workspaces[package_index] = Some(workspace);
                }
                Ok((package_index, Err(error))) => {
                    if prepare_failure.is_none() {
                        prepare_failure = Some((package_index, error));
                    }
                }
                Err(error) => {
                    if prepare_failure.is_none() {
                        prepare_failure = Some((usize::MAX, error.to_string()));
                    }
                }
            }
        }
        let derived_workspace_prepare_wall_ms =
            package_workspace_prepare_started.elapsed().as_millis() as u64;
        if let Some((package_index, error)) = prepare_failure {
            let _ = update_snapshot(&control, |snapshot| {
                snapshot.status = "blocked".to_string();
                snapshot.failure = Some(json!({
                    "kind": "package_workspace_prepare_failed",
                    "category": "workspace",
                    "package": if package_index == usize::MAX { Value::Null } else { json!(package_index) },
                    "message": bounded_message(&error),
                }));
            });
            return execute_task_result(&control);
        }
        let workspaces = prepared_workspaces
            .into_iter()
            .map(|workspace| workspace.expect("all package workspaces prepared"))
            .collect::<Vec<_>>();
        let package_workspace_prepare_wall_ms =
            initial_workspace_prepare_wall_ms.saturating_add(derived_workspace_prepare_wall_ms);

        let package_execution_started = Instant::now();
        let mut jobs = JoinSet::new();
        for (package_index, package) in packaging.packages.iter().enumerate() {
            let runtime = self.clone();
            let workspace = workspaces[package_index].clone();
            let steps = steps.clone();
            let policy = policy.clone();
            let session_id = execution_session_id.clone();
            let auth = auth.cloned();
            let transport = transport.clone();
            let control = control.clone();
            let task_id = task_id.clone();
            let start_step = package.start_step;
            let end_step = package.end_step;
            jobs.spawn(async move {
                runtime
                    .execute_package_range(
                        task_id,
                        package_index,
                        workspace,
                        steps,
                        start_step,
                        end_step,
                        policy,
                        deadline,
                        session_id,
                        auth,
                        transport,
                        control,
                    )
                    .await
            });
        }
        let mut package_results = Vec::new();
        while let Some(joined) = jobs.join_next().await {
            match joined {
                Ok(result) => package_results.push(result),
                Err(error) => {
                    let _ = update_snapshot(&control, |snapshot| {
                        snapshot.status = "blocked".to_string();
                        snapshot.failure = Some(json!({
                            "kind": "package_join_failed",
                            "category": "scheduler",
                            "message": bounded_message(&error.to_string()),
                        }));
                    });
                    return execute_task_result(&control);
                }
            }
        }
        let package_execution_wall_ms = package_execution_started.elapsed().as_millis() as u64;
        package_results.sort_by_key(|(_, outcome)| {
            outcome
                .steps
                .first()
                .map(|step| step.index)
                .unwrap_or(usize::MAX)
        });
        let mut all_steps = package_results
            .iter()
            .flat_map(|(_, outcome)| outcome.steps.clone())
            .collect::<Vec<_>>();
        all_steps.sort_by_key(|step| step.index);
        let failed = package_results
            .iter()
            .find_map(|(_, outcome)| outcome.failure.clone());
        let mut validation_seen = false;
        let mut validation_ok = true;
        let mut review_seen = false;
        let mut completed_mutations = 0usize;
        let mut total_retries = 0usize;
        let mut result_bytes_before = 0usize;
        let mut result_bytes_after = 0usize;
        let mut completed_steps = 0usize;
        for (_, outcome) in &package_results {
            validation_seen |= outcome.validation_seen;
            validation_ok &= outcome.validation_ok;
            review_seen |= outcome.review_seen;
            completed_mutations += outcome.completed_mutations;
            total_retries += outcome.retries;
            result_bytes_before = result_bytes_before.saturating_add(outcome.result_bytes_before);
            result_bytes_after = result_bytes_after.saturating_add(outcome.result_bytes_after);
            completed_steps += outcome.completed_steps;
        }
        let _ = update_snapshot(&control, |snapshot| {
            snapshot.steps = all_steps;
            snapshot.completed_steps = completed_steps;
            snapshot.current_step = completed_steps.min(snapshot.total_steps);
            snapshot.counters = json!({
                "planned_mutations": planned_mutations,
                "planned_changed_files": planned_changed_files,
                "completed_mutations": completed_mutations,
                "retries": total_retries,
                "result_bytes_before": result_bytes_before,
                "result_bytes_after": result_bytes_after,
            });
            if validation_seen {
                snapshot.validation = json!({
                    "status": if validation_ok { "passed" } else { "failed" },
                });
            }
            if review_seen {
                snapshot.review = json!({"status":"completed"});
            }
        });
        if let Some(failure) = failed {
            let status = package_results
                .iter()
                .find_map(|(_, outcome)| {
                    (!outcome.status.is_empty() && outcome.failure.is_some())
                        .then_some(outcome.status.clone())
                })
                .unwrap_or_else(|| "failed".to_string());
            let _ = update_snapshot(&control, |snapshot| {
                snapshot.status = status;
                snapshot.finished_at_ms = Some(now_ms());
                snapshot.duration_ms = Some(elapsed_ms(started));
                snapshot.failure = Some(failure);
            });
            return execute_task_result(&control);
        }

        let package_commit_started = Instant::now();
        let mut package_commits = Vec::new();
        for (workspace, _) in &package_results {
            match self.commit_execution_workspace(workspace).await {
                Ok(Some(commit)) => package_commits.push(commit),
                Ok(None) => {}
                Err(error) => {
                    return self.finish_parallel_workspace_failure(
                        &control,
                        started,
                        "workspace_commit_failed",
                        error,
                    );
                }
            }
        }
        let package_commit_ms = package_commit_started.elapsed().as_millis() as u64;
        let integration_prepare_started = Instant::now();
        let mut integration = match self
            .prepare_derived_execution_workspace(
                &resolved,
                &task_id,
                IsolationMode::Integration,
                &base_sha,
                source_head_sha,
                source_tree_sha,
                source_index_tree_sha,
                auth,
            )
            .await
        {
            Ok(workspace) => workspace,
            Err(error) => {
                return self.finish_parallel_workspace_failure(
                    &control,
                    started,
                    "integration_workspace_prepare_failed",
                    error,
                );
            }
        };
        let integration_prepare_ms = integration_prepare_started.elapsed().as_millis() as u64;
        let integration_merge_started = Instant::now();
        for commit in &package_commits {
            if let Err(error) = self
                .cherry_pick_execution_commit(&integration, commit)
                .await
            {
                return self.finish_parallel_workspace_failure(
                    &control,
                    started,
                    "integration_conflict",
                    error,
                );
            }
        }
        let integration_merge_ms = integration_merge_started.elapsed().as_millis() as u64;
        integration.timings.integration_ms =
            integration_prepare_ms.saturating_add(integration_merge_ms);
        let result_sha = match self.workspace_head_sha(&integration).await {
            Ok(sha) => sha,
            Err(error) => {
                return self.finish_parallel_workspace_failure(
                    &control,
                    started,
                    "integration_head_failed",
                    error,
                );
            }
        };
        let package_files = packaging
            .packages
            .iter()
            .map(|package| {
                (package.start_step..=package.end_step)
                    .flat_map(|index| dependency_paths_for_step(&steps[index]))
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let (final_checks, integration_checks_deduplicated) =
            integration_checks_for_parallel(&steps, &packaging, &package_files);
        let reconciliation_checks = steps
            .iter()
            .filter(|step| is_integration_check(step))
            .cloned()
            .collect::<Vec<_>>();
        let validation_started = Instant::now();
        for step in final_checks.clone() {
            let kind = task_step_kind(&step).to_string();
            let integration_failure_kind = match &step {
                ChadexTaskStep::Validate { .. } => "integration_validation_failed",
                ChadexTaskStep::Review { .. } => "integration_review_failed",
                _ => "integration_process_check_failed",
            };
            let _executor_permit = self.chadex_tasks.scheduler().acquire_executor().await;
            let (result, summary, _) = self
                .dispatch_task_step(
                    &control,
                    &integration.execution_project,
                    execution_session_id.clone(),
                    step,
                    &policy,
                    deadline.saturating_duration_since(Instant::now()),
                    auth,
                    transport.clone(),
                )
                .await;
            if !result.success || result_requires_observation(&result) {
                return self.finish_parallel_workspace_failure(
                    &control,
                    started,
                    integration_failure_kind,
                    result
                        .error
                        .unwrap_or_else(|| "integration_check_failed".to_string()),
                );
            }
            if kind == "validate" {
                validation_seen = true;
                validation_ok = true;
                let summary = summary.unwrap_or_else(|| json!({"status":"passed"}));
                let _ = update_snapshot(&control, |snapshot| {
                    snapshot.validation = summary;
                    snapshot.validation["status"] = json!("passed");
                });
            } else if kind == "review" {
                review_seen = true;
                let summary = summary.unwrap_or_else(|| json!({"status":"completed"}));
                let _ = update_snapshot(&control, |snapshot| {
                    snapshot.review = summary;
                    snapshot.review["status"] = json!("completed");
                });
            }
        }
        integration.timings.validation_ms = validation_started.elapsed().as_millis() as u64;
        let terminal_failure =
            if acceptance.require_validation_success && (!validation_seen || !validation_ok) {
                Some((
                    "blocked",
                    "validation_required",
                    "task acceptance requires a successful validate step",
                ))
            } else if acceptance.require_review && !review_seen {
                Some((
                    "blocked",
                    "review_required",
                    "task acceptance requires a successful review step",
                ))
            } else {
                None
            };
        integration.timings.execution_ms = package_execution_wall_ms;
        if let Some((status, kind, message)) = terminal_failure {
            integration.state = WorkspaceState::Preserved;
            integration.preserve_reason = Some(message.to_string());
            super::execution_workspace::persist_workspace_manifest(&integration);
            let projection = integration.projection();
            let _ = update_snapshot(&control, |snapshot| {
                snapshot.status = status.to_string();
                snapshot.finished_at_ms = Some(now_ms());
                snapshot.duration_ms = Some(elapsed_ms(started));
                snapshot.workspace = projection;
                snapshot.failure =
                    Some(json!({"kind":kind,"category":"acceptance","message":message}));
            });
            return execute_task_result(&control);
        }
        let apply_back_started = Instant::now();
        if let Err(error) = self
            .apply_execution_result_if_write_set_unchanged(&integration, &result_sha)
            .await
        {
            if error == "execution_workspace_source_overlap" {
                if let Ok(mut reconciliation) = self
                    .prepare_execution_workspace(
                        &resolved,
                        &format!("{}-reconcile", task_id),
                        IsolationMode::Integration,
                        None,
                        auth,
                    )
                    .await
                {
                    let reconciliation_result = self
                        .apply_execution_result_three_way(&reconciliation, &base_sha, &result_sha)
                        .await;
                    if reconciliation_result
                        .as_ref()
                        .is_err_and(|error| execution::uncertain_workspace_error(error))
                    {
                        let _ = update_snapshot(&control, |s| s.execution.outcome_uncertain = true);
                    }
                    let mut reconciled = reconciliation_result.is_ok();
                    if reconciled {
                        for check in reconciliation_checks.clone() {
                            let _executor_permit =
                                self.chadex_tasks.scheduler().acquire_executor().await;
                            let (check_result, _, _) = self
                                .dispatch_task_step(
                                    &control,
                                    &reconciliation.execution_project,
                                    execution_session_id.clone(),
                                    check,
                                    &policy,
                                    deadline.saturating_duration_since(Instant::now()),
                                    auth,
                                    transport.clone(),
                                )
                                .await;
                            if !check_result.success || result_requires_observation(&check_result) {
                                reconciled = false;
                                break;
                            }
                        }
                    }
                    let mut reconciliation_error = if reconciled {
                        None
                    } else {
                        Some("source_changed_reconciliation_failed".to_string())
                    };
                    if reconciled {
                        match self.commit_execution_workspace(&reconciliation).await {
                            Ok(Some(reconciled_sha)) => {
                                if let Err(apply_error) = self
                                    .apply_execution_result_if_unchanged(
                                        &reconciliation,
                                        &reconciled_sha,
                                    )
                                    .await
                                {
                                    reconciled = false;
                                    if execution::uncertain_workspace_error(&apply_error) {
                                        let _ = update_snapshot(&control, |s| {
                                            s.execution.outcome_uncertain = true
                                        });
                                    }
                                    reconciliation_error = Some(apply_error);
                                }
                            }
                            Ok(None) => {}
                            Err(commit_error) => {
                                reconciled = false;
                                let _ = update_snapshot(&control, |s| {
                                    s.execution.outcome_uncertain = true
                                });
                                reconciliation_error = Some(commit_error);
                            }
                        }
                    }
                    if reconciled {
                        reconciliation.state = WorkspaceState::Applied;
                        super::execution_workspace::persist_workspace_manifest(&reconciliation);
                        let cleanup_started = Instant::now();
                        if let Err(cleanup_error) = self
                            .cleanup_execution_workspace(&reconciliation, auth)
                            .await
                        {
                            reconciliation.state = WorkspaceState::Preserved;
                            reconciliation.preserve_reason = Some(cleanup_error.clone());
                            reconciliation.timings.cleanup_ms =
                                cleanup_started.elapsed().as_millis() as u64;
                            super::execution_workspace::persist_workspace_manifest(&reconciliation);
                            return self.finish_parallel_workspace_failure(
                                &control,
                                started,
                                "workspace_cleanup_failed",
                                cleanup_error,
                            );
                        }
                        reconciliation.timings.cleanup_ms =
                            cleanup_started.elapsed().as_millis() as u64;
                        reconciliation.state = WorkspaceState::Cleaned;
                        super::execution_workspace::persist_workspace_manifest(&reconciliation);
                        let _ = update_snapshot(&control, |snapshot| {
                            snapshot.counters["reconciliation_applied"] = json!(true);
                        });
                    } else {
                        reconciliation.state = WorkspaceState::Preserved;
                        reconciliation.preserve_reason = reconciliation_error
                            .clone()
                            .or_else(|| Some("source_changed_reconciliation_failed".to_string()));
                        super::execution_workspace::persist_workspace_manifest(&reconciliation);
                        let original_projection = integration.projection();
                        let reconciliation_projection = reconciliation.projection();
                        let message = reconciliation_error
                            .as_deref()
                            .unwrap_or("safe isolated reconciliation failed");
                        let _ = update_snapshot(&control, |snapshot| {
                            snapshot.status = "blocked".to_string();
                            snapshot.finished_at_ms = Some(now_ms());
                            snapshot.duration_ms = Some(elapsed_ms(started));
                            snapshot.workspace = json!({
                                "mode": "integration",
                                "state": "preserved",
                                "source_project": snapshot.project.clone(),
                                "reason": "source_overlap",
                                "original": original_projection,
                                "reconciliation": reconciliation_projection,
                                "reconciled": false,
                            });
                            snapshot.failure = Some(json!({
                                "kind": "workspace_source_overlap",
                                "category": "source_overlap",
                                "message": bounded_message(message),
                            }));
                        });
                        return execute_task_result(&control);
                    }
                } else {
                    return self.finish_parallel_workspace_failure(
                        &control,
                        started,
                        "workspace_source_overlap",
                        error,
                    );
                }
            } else {
                return self.finish_parallel_workspace_failure(
                    &control,
                    started,
                    "workspace_apply_back_failed",
                    error,
                );
            }
        }
        integration.timings.apply_back_ms = apply_back_started.elapsed().as_millis() as u64;
        integration.state = WorkspaceState::Applied;
        let cleanup_all_started = Instant::now();
        let package_cleanup_started = Instant::now();
        let mut cleanup_jobs = JoinSet::new();
        for (package_index, (workspace, _)) in package_results.iter().enumerate() {
            let runtime = self.clone();
            let mut workspace = workspace.clone();
            let auth = auth.cloned();
            cleanup_jobs.spawn(async move {
                let cleanup_started = Instant::now();
                let error = match runtime
                    .cleanup_execution_workspace(&workspace, auth.as_ref())
                    .await
                {
                    Ok(()) => {
                        workspace.state = WorkspaceState::Cleaned;
                        None
                    }
                    Err(error) => {
                        workspace.state = WorkspaceState::Preserved;
                        workspace.preserve_reason = Some(error.clone());
                        Some(error)
                    }
                };
                workspace.timings.cleanup_ms = cleanup_started.elapsed().as_millis() as u64;
                super::execution_workspace::persist_workspace_manifest(&workspace);
                (package_index, workspace, error)
            });
        }
        let mut cleanup_error = None;
        while let Some(joined) = cleanup_jobs.join_next().await {
            match joined {
                Ok((package_index, workspace, error)) => {
                    package_results[package_index].0 = workspace;
                    if cleanup_error.is_none() {
                        cleanup_error = error;
                    }
                }
                Err(error) => {
                    if cleanup_error.is_none() {
                        cleanup_error = Some(error.to_string());
                    }
                }
            }
        }
        let package_cleanup_wall_ms = package_cleanup_started.elapsed().as_millis() as u64;
        let mut integration_cleanup_ms = 0_u64;
        if cleanup_error.is_none() {
            let cleanup_started = Instant::now();
            match self.cleanup_execution_workspace(&integration, auth).await {
                Ok(()) => {
                    integration_cleanup_ms = cleanup_started.elapsed().as_millis() as u64;
                    integration.timings.cleanup_ms = integration_cleanup_ms;
                    integration.state = WorkspaceState::Cleaned;
                    super::execution_workspace::persist_workspace_manifest(&integration);
                }
                Err(error) => {
                    integration_cleanup_ms = cleanup_started.elapsed().as_millis() as u64;
                    integration.state = WorkspaceState::Preserved;
                    integration.preserve_reason = Some(error.clone());
                    integration.timings.cleanup_ms = integration_cleanup_ms;
                    super::execution_workspace::persist_workspace_manifest(&integration);
                    cleanup_error = Some(error);
                }
            }
        }
        if let Some(error) = cleanup_error {
            integration.state = WorkspaceState::Preserved;
            integration.preserve_reason = Some(error.clone());
            super::execution_workspace::persist_workspace_manifest(&integration);
            return self.finish_parallel_workspace_failure(
                &control,
                started,
                "workspace_cleanup_failed",
                error,
            );
        }
        let cleanup_total_ms = cleanup_all_started.elapsed().as_millis() as u64;
        let package_execution_sum_ms = package_results
            .iter()
            .flat_map(|(_, outcome)| outcome.steps.iter())
            .map(|step| step.duration_ms)
            .sum::<u64>();
        let package_validation_ms = package_results
            .iter()
            .flat_map(|(_, outcome)| outcome.steps.iter())
            .filter(|step| step.kind == "validate")
            .map(|step| step.duration_ms)
            .sum::<u64>();
        let package_workspace_component_sum_ms = package_results
            .iter()
            .map(|(workspace, _)| {
                workspace
                    .timings
                    .snapshot_ms
                    .saturating_add(workspace.timings.worktree_creation_ms)
            })
            .sum::<u64>();
        let measured_parallel_overhead_ms = package_workspace_prepare_wall_ms
            .saturating_add(package_commit_ms)
            .saturating_add(integration_prepare_ms)
            .saturating_add(integration_merge_ms)
            .saturating_add(integration.timings.validation_ms)
            .saturating_add(integration.timings.apply_back_ms)
            .saturating_add(cleanup_total_ms);
        let (adaptive_after, adaptive_sample_persisted) = record_parallel_overhead_sample(
            measured_parallel_overhead_ms,
            packaging.packages.len(),
        );
        let projection = integration.projection();
        let _ = update_snapshot(&control, |snapshot| {
            snapshot.status = "completed".to_string();
            snapshot.workspace = projection;
            snapshot.finished_at_ms = Some(now_ms());
            snapshot.duration_ms = Some(elapsed_ms(started));
            snapshot.counters["performance"] = json!({
                "package_execution_wall_ms": package_execution_wall_ms,
                "package_execution_sum_ms": package_execution_sum_ms,
                "package_validation_ms": package_validation_ms,
                "initial_workspace_prepare_wall_ms": initial_workspace_prepare_wall_ms,
                "derived_workspace_prepare_wall_ms": derived_workspace_prepare_wall_ms,
                "package_workspace_prepare_wall_ms": package_workspace_prepare_wall_ms,
                "package_workspace_component_sum_ms": package_workspace_component_sum_ms,
                "package_commit_ms": package_commit_ms,
                "integration_prepare_ms": integration_prepare_ms,
                "integration_merge_ms": integration_merge_ms,
                "integration_checks_deduplicated": integration_checks_deduplicated,
                "final_validation_ms": integration.timings.validation_ms,
                "apply_back_ms": integration.timings.apply_back_ms,
                "package_cleanup_wall_ms": package_cleanup_wall_ms,
                "integration_cleanup_ms": integration_cleanup_ms,
                "cleanup_ms": cleanup_total_ms,
                "measured_parallel_overhead_ms": measured_parallel_overhead_ms,
                "parallel_overhead_sample_persisted": adaptive_sample_persisted,
                "parallel_overhead_base_ewma_ms": adaptive_after.parallel_overhead_base_ewma_ms.round() as u64,
                "parallel_overhead_samples": adaptive_after.parallel_samples,
            });
        });
        execute_task_result(&control)
    }

    fn finish_parallel_workspace_failure(
        &self,
        control: &Arc<TaskControl>,
        started: Instant,
        kind: &str,
        message: String,
    ) -> ToolResult {
        let _ = update_snapshot(control, |snapshot| {
            snapshot.status = "blocked".to_string();
            snapshot.finished_at_ms = Some(now_ms());
            snapshot.duration_ms = Some(elapsed_ms(started));
            snapshot.failure = Some(json!({
                "kind": kind,
                "category": "workspace",
                "message": bounded_message(&message),
            }));
        });
        execute_task_result(control)
    }

    pub(crate) async fn execute_chadex_tasks(
        &self,
        resolved: ResolvedProject,
        tasks: Vec<ChadexTaskBatchItem>,
        auth: Option<&AuthContext>,
        transport: sessions::SessionTransport,
    ) -> ToolResult {
        if !(2..=4).contains(&tasks.len()) {
            return ToolResult::err_with_output(
                "execute_tasks_invalid_count",
                json!({
                    "error_kind":"execute_tasks_invalid_count",
                    "minimum":2,
                    "maximum":4,
                    "requested_count":tasks.len(),
                    "state_changed":false,
                }),
            );
        }
        let mut explicit_ids = HashSet::new();
        for task in &tasks {
            if let Some(task_id) = task.task_id.as_deref() {
                if !valid_task_id(task_id) {
                    return ToolResult::err_with_output(
                        "invalid_task_id",
                        json!({"error_kind":"invalid_task_id","state_changed":false}),
                    );
                }
                if !explicit_ids.insert(task_id.to_string()) {
                    return ToolResult::err_with_output(
                        "execute_tasks_duplicate_task_id",
                        json!({
                            "error_kind":"execute_tasks_duplicate_task_id",
                            "task_id":task_id,
                            "state_changed":false,
                        }),
                    );
                }
            }
        }

        let started = Instant::now();
        let project_id = resolved.resolved_id.clone();
        let auth = auth.cloned();
        let mut joins = JoinSet::new();
        for (index, task) in tasks.into_iter().enumerate() {
            let runtime = self.clone();
            let resolved = resolved.clone();
            let auth = auth.clone();
            let transport = transport.clone();
            joins.spawn(async move {
                let result = runtime
                    .execute_chadex_task(
                        resolved,
                        task.task_id,
                        task.goal,
                        task.preplan,
                        task.package_count,
                        task.steps,
                        task.policy,
                        task.acceptance,
                        task.session_id,
                        auth.as_ref(),
                        transport,
                    )
                    .await;
                (index, result)
            });
        }

        let mut items = Vec::new();
        while let Some(joined) = joins.join_next().await {
            match joined {
                Ok((index, result)) => items.push((index, result)),
                Err(error) => {
                    items.push((
                        usize::MAX,
                        ToolResult::err_with_output(
                            "execute_tasks_join_failed",
                            json!({
                                "error_kind":"execute_tasks_join_failed",
                                "message":bounded_message(&error.to_string()),
                            }),
                        ),
                    ));
                }
            }
        }
        items.sort_by_key(|(index, _)| *index);
        let requested_count = items.len();
        let succeeded_count = items.iter().filter(|(_, result)| result.success).count();
        let failed_count = requested_count.saturating_sub(succeeded_count);
        let projected_items = items
            .into_iter()
            .enumerate()
            .map(|(fallback_index, (index, result))| {
                json!({
                    "index": if index == usize::MAX { fallback_index } else { index },
                    "success": result.success,
                    "error": result.error,
                    "output": result.output,
                })
            })
            .collect::<Vec<_>>();
        let output = json!({
            "project":project_id,
            "requested_count":requested_count,
            "succeeded_count":succeeded_count,
            "failed_count":failed_count,
            "duration_ms":started.elapsed().as_millis().min(u64::MAX as u128) as u64,
            "items":projected_items,
        });
        if failed_count == 0 {
            ToolResult::ok(output)
        } else {
            ToolResult::err_with_output("execute_tasks_partial_failure", output)
        }
    }

    pub(crate) async fn execute_chadex_task(
        &self,
        resolved: ResolvedProject,
        requested_task_id: Option<String>,
        goal: String,
        preplan: Option<ChadexTaskPreplan>,
        requested_package_count: Option<usize>,
        steps: Vec<ChadexTaskStep>,
        policy: Option<ChadexTaskPolicy>,
        acceptance: Option<ChadexTaskAcceptance>,
        session_id: Option<String>,
        auth: Option<&AuthContext>,
        transport: sessions::SessionTransport,
    ) -> ToolResult {
        self.execute_chadex_task_attempt(
            resolved,
            requested_task_id,
            goal,
            preplan,
            requested_package_count,
            steps,
            policy,
            acceptance,
            session_id,
            auth,
            transport,
            None,
        )
        .await
    }

    async fn execute_chadex_task_attempt(
        &self,
        resolved: ResolvedProject,
        requested_task_id: Option<String>,
        goal: String,
        preplan: Option<ChadexTaskPreplan>,
        requested_package_count: Option<usize>,
        steps: Vec<ChadexTaskStep>,
        policy: Option<ChadexTaskPolicy>,
        acceptance: Option<ChadexTaskAcceptance>,
        session_id: Option<String>,
        auth: Option<&AuthContext>,
        transport: sessions::SessionTransport,
        previous_execution_id: Option<String>,
    ) -> ToolResult {
        let project = resolved.resolved_id.clone();
        let source_path = resolved.config.path.clone();
        let recovery_goal = goal.clone();
        let recovery_preplan = preplan.clone();
        let recovery_steps = steps.clone();
        let recovery_policy = policy.clone();
        let recovery_acceptance = acceptance.clone();
        let policy = match normalize_policy(policy) {
            Ok(policy) => policy,
            Err(error) => {
                return ToolResult::err_with_output(
                    error.clone(),
                    json!({"error_kind":"task_policy_rejected","reason_code":error,"state_changed":false}),
                )
            }
        };
        let acceptance = normalize_acceptance(acceptance);
        if let Some(preplan) = preplan.as_ref() {
            if let Err(error) = validate_task_preplan(preplan) {
                return ToolResult::err_with_output(
                    error.clone(),
                    json!({"error_kind":"task_preflight_rejected","reason_code":error,"state_changed":false}),
                );
            }
        }
        if !acceptance.require_review
            && steps.iter().any(|step| {
                matches!(
                    step,
                    ChadexTaskStep::RunProcess { .. } | ChadexTaskStep::Validate { .. }
                )
            })
        {
            return ToolResult::err_with_output(
                "process_step_requires_review",
                json!({"error_kind":"task_preflight_rejected","reason_code":"process_step_requires_review","state_changed":false}),
            );
        }
        let (planned_mutations, planned_changed_files) = match preflight_task(
            &goal, &steps, &policy,
        ) {
            Ok(counts) => counts,
            Err(error) => {
                return ToolResult::err_with_output(
                    error.clone(),
                    json!({"error_kind":"task_preflight_rejected","reason_code":error,"state_changed":false}),
                )
            }
        };
        if planned_mutations > 0 && !resolved.config.allow_patch {
            return ToolResult::err_with_output(
                "project_patch_not_allowed",
                json!({
                    "error_kind": "task_preflight_rejected",
                    "reason_code": "project_patch_not_allowed",
                    "state_changed": false,
                }),
            );
        }
        let task_id = match requested_task_id {
            Some(task_id) if valid_task_id(&task_id) => task_id,
            Some(_) => {
                return ToolResult::err_with_output(
                    "invalid_task_id",
                    json!({"error_kind":"task_preflight_rejected","reason_code":"invalid_task_id","state_changed":false}),
                )
            }
            None => format!("chadex_task_{}", Uuid::new_v4().simple()),
        };
        let started_at_ms = now_ms();
        let started = Instant::now();
        let total_steps = steps.len();
        let plan = steps
            .iter()
            .map(|step| task_step_kind(step).to_string())
            .collect::<Vec<_>>();
        let complexity = assess_task_complexity(
            preplan.as_ref(),
            &steps,
            planned_mutations,
            planned_changed_files,
        );
        let package_target = initial_package_target(
            requested_package_count,
            preplan.is_some(),
            complexity.recommended_task_count,
        );
        let mut packaging = match plan_task_packages(
            &steps,
            package_target,
            complexity.recommended_task_count,
        ) {
            Ok(packaging) => packaging,
            Err(error) => {
                return ToolResult::err_with_output(
                    error.clone(),
                    json!({"error_kind":"task_preflight_rejected","reason_code":error,"state_changed":false}),
                )
            }
        };
        let requires_mutation_boundary = planned_mutations > 0
            || steps.iter().any(|step| {
                matches!(
                    step,
                    ChadexTaskStep::RunProcess { .. } | ChadexTaskStep::Validate { .. }
                )
            });
        let (graphify_evidence, graphify_planning_ms) = self
            .collect_graphify_dependency_evidence(
                &project,
                session_id.as_deref(),
                &steps,
                &packaging,
                complexity.band,
                requires_mutation_boundary,
                &policy,
                auth,
                transport.clone(),
            )
            .await;
        apply_graphify_dependency_evidence(&mut packaging, graphify_evidence, graphify_planning_ms);
        let adaptive_execution = load_adaptive_execution_telemetry();
        let adaptive_base_overhead_ms = adaptive_execution
            .parallel_overhead_base_ewma_ms
            .round()
            .clamp(
                PARALLEL_OVERHEAD_MIN_MS as f64,
                PARALLEL_OVERHEAD_MAX_MS as f64,
            ) as u64;
        apply_parallel_cost_gate(
            &mut packaging,
            &steps,
            self.chadex_tasks.scheduler().limits().1,
            adaptive_base_overhead_ms,
            adaptive_execution.parallel_samples,
        );
        let package_files = packaging
            .packages
            .iter()
            .map(|package| {
                (package.start_step..=package.end_step)
                    .flat_map(|index| dependency_paths_for_step(&steps[index]))
                    .collect::<HashSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let paths_are_independent = package_paths_are_independent(&package_files);
        let packages_independent = paths_are_independent
            && packaging.package_parallelism_proven
            && packaging.parallel_cost_gate_passed
            && requires_mutation_boundary;
        let isolation_mode = choose_isolation_mode(
            requires_mutation_boundary,
            complexity.band,
            packaging.effective_package_count,
            packages_independent,
        );
        let execution = match self
            .chadex_tasks
            .execution_root()
            .and_then(|root| Execution::claim(&root, &task_id, previous_execution_id))
        {
            Ok(execution) => execution,
            Err(error) => {
                return ToolResult::err_with_output(
                    error.clone(),
                    json!({
                        "error_kind":"execution_claim_rejected", "reason_code":error, "state_changed":false
                    }),
                )
            }
        };
        let control = Arc::new(TaskControl {
            snapshot: StdMutex::new(TaskSnapshot {
                execution,
                task_id: task_id.clone(),
                project: project.clone(),
                source_path,
                goal,
                status: "queued".to_string(),
                current_step: 0,
                total_steps,
                completed_steps: 0,
                plan,
                cancel_requested: false,
                started_at_ms,
                finished_at_ms: None,
                duration_ms: None,
                limits: policy.clone(),
                counters: json!({"planned_mutations":planned_mutations,"planned_changed_files":planned_changed_files,"completed_mutations":0,"retries":0}),
                validation: json!({"status":"not_run","checks_passed":0,"checks_failed":0}),
                review: json!({"status":"not_run"}),
                steps: Vec::new(),
                failure: None,
                complexity,
                packaging: packaging.clone(),
                workspace: json!({
                    "mode": isolation_mode.as_str(),
                    "state": "queued",
                    "source_project": project.clone(),
                    "worktree_created": false,
                }),
            }),
            raw_log: StdMutex::new(()),
            cancel_requested: AtomicBool::new(false),
        });
        if let Err(error) = self.chadex_tasks.insert(control.clone()) {
            return ToolResult::err_with_output(
                error.clone(),
                json!({"error_kind":"task_admission_rejected","reason_code":error,"state_changed":false}),
            );
        }
        let _execution_guard = ExecutionGuard(control.clone());
        if let Ok(snapshot) = control.snapshot.lock().map(|snapshot| snapshot.clone()) {
            persist_task_state(&snapshot);
        }
        let recovery_plan = TaskRecoveryPlan {
            version: 1,
            project: project.clone(),
            task_id: task_id.clone(),
            goal: recovery_goal,
            preplan: recovery_preplan,
            package_count: requested_package_count,
            steps: recovery_steps,
            policy: recovery_policy,
            acceptance: recovery_acceptance,
            created_at_ms: started_at_ms,
        };
        let recovery_plan_persisted = self
            .chadex_tasks
            .execution_root()
            .is_ok_and(|root| persist_task_recovery_plan_at(&root, &recovery_plan));
        let _ = update_snapshot(&control, |snapshot| {
            snapshot.counters["recovery_plan_persisted"] = json!(recovery_plan_persisted);
        });
        let (_task_permit, scheduler_wait_ms) = self.chadex_tasks.scheduler().acquire_task().await;
        let deadline = started + Duration::from_secs(policy.timeout_secs);
        // Cancellation/deadline are checked after admission and before any worktree mutation.
        let stopped = control.cancel_requested.load(Ordering::SeqCst) || Instant::now() >= deadline;
        let transition = update_snapshot(&control, |snapshot| {
            snapshot.status = if stopped {
                if control.cancel_requested.load(Ordering::SeqCst) {
                    "cancelled"
                } else {
                    "blocked"
                }
            } else {
                "running"
            }
            .into();
            if stopped {
                snapshot.finished_at_ms = Some(now_ms());
                snapshot.failure = Some(
                    json!({"kind":"admission_cancelled_or_timed_out", "category":"admission"}),
                );
            } else {
                snapshot.execution.effects_possible = requires_mutation_boundary;
            }
        });
        if stopped || transition.is_err() {
            return execute_task_result(&control);
        }
        let initial_workspace_prepare_started = Instant::now();
        let mut workspace = match self
            .prepare_execution_workspace(&resolved, &task_id, isolation_mode, None, auth)
            .await
        {
            Ok(workspace) => workspace,
            Err(error) => {
                let _ = update_snapshot(&control, |snapshot| {
                    snapshot.status = "blocked".to_string();
                    snapshot.finished_at_ms = Some(now_ms());
                    snapshot.duration_ms = Some(elapsed_ms(started));
                    snapshot.failure = Some(json!({
                        "kind": "workspace_prepare_failed",
                        "category": "workspace",
                        "message": bounded_message(&error),
                    }));
                    snapshot.workspace = json!({
                        "mode": isolation_mode.as_str(),
                        "state": "preserved",
                        "source_project": snapshot.project.clone(),
                        "worktree_created": false,
                    });
                });
                return execute_task_result(&control);
            }
        };
        let initial_workspace_prepare_wall_ms =
            initial_workspace_prepare_started.elapsed().as_millis() as u64;
        workspace.timings.scheduler_wait_ms = scheduler_wait_ms;
        let workspace_projection = workspace.projection();
        let _ = update_snapshot(&control, |snapshot| {
            snapshot.status = "running".to_string();
            snapshot.workspace = workspace_projection.clone();
            snapshot.counters["scheduler_wait_ms"] = json!(scheduler_wait_ms);
            snapshot.counters["max_concurrent_tasks"] =
                json!(self.chadex_tasks.scheduler().limits().0);
            snapshot.counters["global_executor_concurrency"] =
                json!(self.chadex_tasks.scheduler().limits().1);
        });
        let execution_project = workspace.execution_project.clone();
        let execution_session_id = if execution_project == project {
            session_id.clone()
        } else {
            None
        };
        if isolation_mode == IsolationMode::Package {
            return self
                .execute_parallel_package_task(
                    resolved,
                    task_id,
                    control,
                    steps,
                    packaging,
                    policy,
                    acceptance,
                    execution_session_id.clone(),
                    auth,
                    transport,
                    started,
                    deadline,
                    workspace,
                    initial_workspace_prepare_wall_ms,
                    planned_mutations,
                    planned_changed_files,
                )
                .await;
        }
        let mut validation_seen = false;
        let mut validation_ok = true;
        let mut review_seen = false;
        let reconciliation_checks = steps
            .iter()
            .filter(|step| is_integration_check(step))
            .cloned()
            .collect::<Vec<_>>();
        let mut completed_mutations = 0usize;
        let mut total_retries = 0usize;
        let mut total_result_bytes_before = 0usize;
        let mut total_result_bytes_after = 0usize;
        let execution_started = Instant::now();
        let mut validation_execution_ms = 0_u64;

        for (index, step) in steps.into_iter().enumerate() {
            let package_index = package_index_for_step(&packaging, index);
            if control.cancel_requested.load(Ordering::SeqCst) {
                let _ = update_snapshot(&control, |snapshot| {
                    snapshot.status = "cancelled".to_string();
                    snapshot.cancel_requested = true;
                    snapshot.finished_at_ms = Some(now_ms());
                    snapshot.duration_ms = Some(elapsed_ms(started));
                });
                return execute_task_result(&control);
            }
            let now = Instant::now();
            if now >= deadline {
                let _ = update_snapshot(&control, |snapshot| {
                    snapshot.status = "blocked".to_string();
                    snapshot.finished_at_ms = Some(now_ms());
                    snapshot.duration_ms = Some(elapsed_ms(started));
                    snapshot.failure = Some(
                        json!({"kind":"task_timeout","category":"timeout","message":"task runtime budget exhausted before the next step","step":index,"package":package_index}),
                    );
                });
                return execute_task_result(&control);
            }
            let kind = task_step_kind(&step).to_string();
            let step_started = Instant::now();
            let _ = update_snapshot(&control, |snapshot| snapshot.current_step = index);
            let mut attempts = 0usize;
            let mut retries = 0usize;
            let mut raw_log_retained = false;
            let mut step_result_bytes_before = 0usize;
            let mut step_result_bytes_after = 0usize;
            let (result, summary, mutated) = loop {
                attempts += 1;
                let _executor_permit = self.chadex_tasks.scheduler().acquire_executor().await;
                let outcome = self
                    .dispatch_task_step(
                        &control,
                        &execution_project,
                        execution_session_id.clone(),
                        step.clone(),
                        &policy,
                        deadline.saturating_duration_since(Instant::now()),
                        auth,
                        transport.clone(),
                    )
                    .await;
                step_result_bytes_before = step_result_bytes_before.saturating_add(
                    serde_json::to_vec(&outcome.0)
                        .map(|bytes| bytes.len())
                        .unwrap_or(0),
                );
                step_result_bytes_after = step_result_bytes_after
                    .saturating_add(outcome.1.as_ref().map(serialized_len).unwrap_or(0));
                raw_log_retained |= persist_task_step_result(
                    &control, &task_id, index, &kind, attempts, &outcome.0,
                );
                if should_retry_read_only_failure(&kind, &outcome.0, retries)
                    && !control.cancel_requested.load(Ordering::SeqCst)
                    && Instant::now() < deadline
                {
                    retries += 1;
                    total_retries += 1;
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    continue;
                }
                break outcome;
            };
            total_result_bytes_before =
                total_result_bytes_before.saturating_add(step_result_bytes_before);
            total_result_bytes_after =
                total_result_bytes_after.saturating_add(step_result_bytes_after);
            let duration_ms = elapsed_ms(step_started);
            if kind == "validate" {
                validation_execution_ms = validation_execution_ms.saturating_add(duration_ms);
                validation_seen = true;
                validation_ok = result.success;
            }
            if kind == "review" {
                review_seen = result.success;
            }
            if mutated && result.success {
                completed_mutations += 1;
            }
            if result_requires_observation(&result) {
                let mut blocked_failure = json!({
                    "kind": "job_observation_required",
                    "category": "blocked_job",
                    "message": "a nested process outlived the synchronous task step; observe the underlying Job before continuing",
                    "step": index,
                    "package": package_index,
                    "step_kind": kind,
                    "attempts": attempts,
                    "retries": retries,
                    "raw_log_retained": raw_log_retained,
                    "job_id": result.output.get("job_id"),
                    "execution_state": result.output.get("execution_state"),
                    "stdout_truncated": result.output.get("stdout_truncated"),
                    "stderr_truncated": result.output.get("stderr_truncated"),
                });
                omit_null_object_fields(&mut blocked_failure);
                let _ = update_snapshot(&control, |snapshot| {
                    snapshot.steps.push(TaskStepSummary {
                        index,
                        kind: kind.clone(),
                        status: "blocked".to_string(),
                        duration_ms,
                        attempts,
                        retries,
                        result_bytes_before: step_result_bytes_before,
                        result_bytes_after: step_result_bytes_after,
                        summary: summary.clone(),
                    });
                    snapshot.counters = json!({
                        "planned_mutations": planned_mutations,
                        "planned_changed_files": planned_changed_files,
                        "completed_mutations": completed_mutations,
                        "retries": total_retries,
                        "result_bytes_before": total_result_bytes_before,
                        "result_bytes_after": total_result_bytes_after,
                    });
                    snapshot.status = "blocked".to_string();
                    snapshot.finished_at_ms = Some(now_ms());
                    snapshot.duration_ms = Some(elapsed_ms(started));
                    snapshot.failure = Some(blocked_failure.clone());
                });
                return execute_task_result(&control);
            }
            if !result.success {
                let failure_status = if kind == "validate" {
                    "failed_validation"
                } else {
                    "failed"
                };
                let mut failure = compact_failure_details(
                    &result,
                    summary.as_ref(),
                    index,
                    &kind,
                    attempts,
                    retries,
                    raw_log_retained,
                );
                failure["package"] = json!(package_index);
                let _ = update_snapshot(&control, |snapshot| {
                    snapshot.steps.push(TaskStepSummary {
                        index,
                        kind: kind.clone(),
                        status: "failed".to_string(),
                        duration_ms,
                        attempts,
                        retries,
                        result_bytes_before: step_result_bytes_before,
                        result_bytes_after: step_result_bytes_after,
                        summary: summary.clone(),
                    });
                    snapshot.counters = json!({
                        "planned_mutations": planned_mutations,
                        "planned_changed_files": planned_changed_files,
                        "completed_mutations": completed_mutations,
                        "retries": total_retries,
                        "result_bytes_before": total_result_bytes_before,
                        "result_bytes_after": total_result_bytes_after,
                    });
                    snapshot.status = failure_status.to_string();
                    snapshot.finished_at_ms = Some(now_ms());
                    snapshot.duration_ms = Some(elapsed_ms(started));
                    snapshot.failure = Some(failure.clone());
                    if failure_status == "failed_validation" {
                        snapshot.validation = summary.clone().unwrap_or_else(|| json!({}));
                        snapshot.validation["status"] = json!("failed");
                    }
                });
                return execute_task_result(&control);
            }
            let _ = update_snapshot(&control, |snapshot| {
                snapshot.steps.push(TaskStepSummary {
                    index,
                    kind: kind.clone(),
                    status: "completed".to_string(),
                    duration_ms,
                    attempts,
                    retries,
                    result_bytes_before: step_result_bytes_before,
                    result_bytes_after: step_result_bytes_after,
                    summary: summary.clone(),
                });
                snapshot.completed_steps += 1;
                snapshot.current_step = index + 1;
                snapshot.counters = json!({
                    "planned_mutations": planned_mutations,
                    "planned_changed_files": planned_changed_files,
                    "completed_mutations": completed_mutations,
                    "retries": total_retries,
                    "result_bytes_before": total_result_bytes_before,
                    "result_bytes_after": total_result_bytes_after,
                });
                if kind == "validate" {
                    snapshot.validation = summary
                        .clone()
                        .unwrap_or_else(|| json!({"status":"passed"}));
                    snapshot.validation["status"] = json!("passed");
                }
                if kind == "review" {
                    snapshot.review = summary
                        .clone()
                        .unwrap_or_else(|| json!({"status":"completed"}));
                    snapshot.review["status"] = json!("completed");
                }
            });
            if kind == "review" {
                if let Ok(snapshot) = control.snapshot.lock() {
                    let changed = snapshot
                        .review
                        .get("changed_file_count")
                        .and_then(Value::as_u64)
                        .unwrap_or(0) as usize;
                    if changed > policy.max_changed_files {
                        drop(snapshot);
                        let _ = update_snapshot(&control, |snapshot| {
                            snapshot.status = "blocked".to_string();
                            snapshot.finished_at_ms = Some(now_ms());
                            snapshot.duration_ms = Some(elapsed_ms(started));
                            snapshot.failure = Some(json!({
                                "kind":"changed_file_limit_exceeded",
                                "category":"safety_block",
                                "message":"final review exceeds task max_changed_files"
                            }));
                        });
                        return execute_task_result(&control);
                    }
                }
            }
            if control.cancel_requested.load(Ordering::SeqCst) {
                let _ = update_snapshot(&control, |snapshot| {
                    snapshot.status = "cancelled".to_string();
                    snapshot.cancel_requested = true;
                    snapshot.finished_at_ms = Some(now_ms());
                    snapshot.duration_ms = Some(elapsed_ms(started));
                });
                return execute_task_result(&control);
            }
        }

        let terminal_failure =
            if acceptance.require_validation_success && (!validation_seen || !validation_ok) {
                Some((
                    "blocked",
                    "validation_required",
                    "task acceptance requires a successful validate step",
                ))
            } else if acceptance.require_review && !review_seen {
                Some((
                    "blocked",
                    "review_required",
                    "task acceptance requires a successful review step",
                ))
            } else {
                None
            };
        workspace.timings.execution_ms = elapsed_ms(execution_started);
        workspace.timings.validation_ms = validation_execution_ms;
        if let Some((_, _, message)) = terminal_failure {
            if isolation_mode != IsolationMode::None {
                workspace.state = WorkspaceState::Preserved;
                workspace.preserve_reason = Some(message.to_string());
                super::execution_workspace::persist_workspace_manifest(&workspace);
                let projection = workspace.projection();
                let _ = update_snapshot(&control, |snapshot| snapshot.workspace = projection);
            }
        } else if isolation_mode != IsolationMode::None {
            let commit_started = Instant::now();
            let result_sha = match self.commit_execution_workspace(&workspace).await {
                Ok(result_sha) => result_sha,
                Err(error) => {
                    workspace.state = WorkspaceState::Preserved;
                    workspace.preserve_reason = Some(error.clone());
                    super::execution_workspace::persist_workspace_manifest(&workspace);
                    let projection = workspace.projection();
                    let _ = update_snapshot(&control, |snapshot| {
                        snapshot.status = "blocked".to_string();
                        snapshot.finished_at_ms = Some(now_ms());
                        snapshot.duration_ms = Some(elapsed_ms(started));
                        snapshot.workspace = projection;
                        snapshot.failure = Some(json!({
                            "kind": "workspace_commit_failed",
                            "category": "workspace",
                            "message": bounded_message(&error),
                        }));
                    });
                    return execute_task_result(&control);
                }
            };
            workspace.timings.integration_ms = workspace
                .timings
                .integration_ms
                .saturating_add(commit_started.elapsed().as_millis() as u64);
            if let Some(result_sha) = result_sha.as_deref() {
                let apply_started = Instant::now();
                if let Err(error) = self
                    .apply_execution_result_if_write_set_unchanged(&workspace, result_sha)
                    .await
                {
                    if error == "execution_workspace_source_overlap" {
                        if let Ok(mut reconciliation) = self
                            .prepare_execution_workspace(
                                &resolved,
                                &format!("{}-reconcile", task_id),
                                IsolationMode::Integration,
                                None,
                                auth,
                            )
                            .await
                        {
                            let reconciliation_result = self
                                .apply_execution_result_three_way(
                                    &reconciliation,
                                    workspace.base_sha.as_deref().unwrap_or_default(),
                                    result_sha,
                                )
                                .await;
                            if reconciliation_result
                                .as_ref()
                                .is_err_and(|error| execution::uncertain_workspace_error(error))
                            {
                                let _ = update_snapshot(&control, |s| {
                                    s.execution.outcome_uncertain = true
                                });
                            }
                            let mut checks_ok = reconciliation_result.is_ok();
                            if checks_ok {
                                for check in reconciliation_checks.clone() {
                                    let _executor_permit =
                                        self.chadex_tasks.scheduler().acquire_executor().await;
                                    let (check_result, _, _) = self
                                        .dispatch_task_step(
                                            &control,
                                            &reconciliation.execution_project,
                                            execution_session_id.clone(),
                                            check,
                                            &policy,
                                            deadline.saturating_duration_since(Instant::now()),
                                            auth,
                                            transport.clone(),
                                        )
                                        .await;
                                    if !check_result.success
                                        || result_requires_observation(&check_result)
                                    {
                                        checks_ok = false;
                                        break;
                                    }
                                }
                            }
                            let mut reconciliation_error = if checks_ok {
                                None
                            } else {
                                Some("source_changed_reconciliation_failed".to_string())
                            };
                            if checks_ok {
                                match self.commit_execution_workspace(&reconciliation).await {
                                    Ok(Some(reconciled_sha)) => {
                                        if let Err(apply_error) = self
                                            .apply_execution_result_if_unchanged(
                                                &reconciliation,
                                                &reconciled_sha,
                                            )
                                            .await
                                        {
                                            checks_ok = false;
                                            if execution::uncertain_workspace_error(&apply_error) {
                                                let _ = update_snapshot(&control, |s| {
                                                    s.execution.outcome_uncertain = true
                                                });
                                            }
                                            reconciliation_error = Some(apply_error);
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(commit_error) => {
                                        checks_ok = false;
                                        let _ = update_snapshot(&control, |s| {
                                            s.execution.outcome_uncertain = true
                                        });
                                        reconciliation_error = Some(commit_error);
                                    }
                                }
                            }
                            if checks_ok {
                                reconciliation.state = WorkspaceState::Applied;
                                super::execution_workspace::persist_workspace_manifest(
                                    &reconciliation,
                                );
                                let cleanup_started = Instant::now();
                                if let Err(cleanup_error) = self
                                    .cleanup_execution_workspace(&reconciliation, auth)
                                    .await
                                {
                                    reconciliation.state = WorkspaceState::Preserved;
                                    reconciliation.preserve_reason = Some(cleanup_error.clone());
                                    reconciliation.timings.cleanup_ms =
                                        cleanup_started.elapsed().as_millis() as u64;
                                    super::execution_workspace::persist_workspace_manifest(
                                        &reconciliation,
                                    );
                                    workspace.state = WorkspaceState::Preserved;
                                    workspace.preserve_reason = Some(cleanup_error.clone());
                                    super::execution_workspace::persist_workspace_manifest(
                                        &workspace,
                                    );
                                    let projection = reconciliation.projection();
                                    let _ = update_snapshot(&control, |snapshot| {
                                        snapshot.status = "blocked".to_string();
                                        snapshot.finished_at_ms = Some(now_ms());
                                        snapshot.duration_ms = Some(elapsed_ms(started));
                                        snapshot.workspace = projection;
                                        snapshot.failure = Some(json!({
                                            "kind": "workspace_cleanup_failed",
                                            "category": "workspace",
                                            "message": bounded_message(&cleanup_error),
                                        }));
                                    });
                                    return execute_task_result(&control);
                                }
                                reconciliation.timings.cleanup_ms =
                                    cleanup_started.elapsed().as_millis() as u64;
                                reconciliation.state = WorkspaceState::Cleaned;
                                super::execution_workspace::persist_workspace_manifest(
                                    &reconciliation,
                                );
                                let _ = update_snapshot(&control, |snapshot| {
                                    snapshot.counters["reconciliation_applied"] = json!(true);
                                });
                            } else {
                                reconciliation.state = WorkspaceState::Preserved;
                                reconciliation.preserve_reason =
                                    reconciliation_error.clone().or_else(|| {
                                        Some("source_changed_reconciliation_failed".to_string())
                                    });
                                super::execution_workspace::persist_workspace_manifest(
                                    &reconciliation,
                                );
                                workspace.state = WorkspaceState::Preserved;
                                workspace.preserve_reason = reconciliation_error.clone();
                                super::execution_workspace::persist_workspace_manifest(&workspace);
                                let original_projection = workspace.projection();
                                let reconciliation_projection = reconciliation.projection();
                                let message = reconciliation_error
                                    .as_deref()
                                    .unwrap_or("safe isolated reconciliation did not pass");
                                let _ = update_snapshot(&control, |snapshot| {
                                    snapshot.status = "blocked".to_string();
                                    snapshot.finished_at_ms = Some(now_ms());
                                    snapshot.duration_ms = Some(elapsed_ms(started));
                                    snapshot.workspace = json!({
                                        "mode": "integration",
                                        "state": "preserved",
                                        "source_project": snapshot.project.clone(),
                                        "reason": "source_overlap",
                                        "original": original_projection,
                                        "reconciliation": reconciliation_projection,
                                        "reconciled": false,
                                    });
                                    snapshot.failure = Some(json!({
                                        "kind": "workspace_source_overlap",
                                        "category": "source_overlap",
                                        "message": bounded_message(message),
                                    }));
                                });
                                return execute_task_result(&control);
                            }
                        } else {
                            workspace.state = WorkspaceState::Preserved;
                            workspace.preserve_reason = Some(error.clone());
                            super::execution_workspace::persist_workspace_manifest(&workspace);
                            let projection = workspace.projection();
                            let _ = update_snapshot(&control, |snapshot| {
                                snapshot.status = "blocked".to_string();
                                snapshot.finished_at_ms = Some(now_ms());
                                snapshot.duration_ms = Some(elapsed_ms(started));
                                snapshot.workspace = projection;
                                snapshot.failure = Some(json!({
                                    "kind": "workspace_source_overlap",
                                    "category": "source_overlap",
                                    "message": bounded_message(&error),
                                }));
                                snapshot.execution.outcome_uncertain = true;
                            });
                            return execute_task_result(&control);
                        }
                    } else {
                        workspace.state = WorkspaceState::Preserved;
                        workspace.preserve_reason = Some(error.clone());
                        super::execution_workspace::persist_workspace_manifest(&workspace);
                        let projection = workspace.projection();
                        let _ = update_snapshot(&control, |snapshot| {
                            snapshot.status = "blocked".to_string();
                            snapshot.finished_at_ms = Some(now_ms());
                            snapshot.duration_ms = Some(elapsed_ms(started));
                            snapshot.workspace = projection;
                            snapshot.failure = Some(json!({
                                "kind": "workspace_apply_back_blocked",
                                "category": "workspace",
                                "message": bounded_message(&error),
                            }));
                        });
                        return execute_task_result(&control);
                    }
                }
                workspace.timings.apply_back_ms = apply_started.elapsed().as_millis() as u64;
            }
            let cleanup_started = Instant::now();
            if let Err(error) = self.cleanup_execution_workspace(&workspace, auth).await {
                workspace.state = WorkspaceState::Preserved;
                workspace.preserve_reason = Some(error.clone());
                super::execution_workspace::persist_workspace_manifest(&workspace);
                let projection = workspace.projection();
                let _ = update_snapshot(&control, |snapshot| {
                    snapshot.status = "blocked".to_string();
                    snapshot.finished_at_ms = Some(now_ms());
                    snapshot.duration_ms = Some(elapsed_ms(started));
                    snapshot.workspace = projection;
                    snapshot.failure = Some(json!({
                        "kind": "workspace_cleanup_failed",
                        "category": "workspace",
                        "message": bounded_message(&error),
                    }));
                });
                return execute_task_result(&control);
            }
            workspace.timings.cleanup_ms = cleanup_started.elapsed().as_millis() as u64;
            workspace.state = WorkspaceState::Cleaned;
            super::execution_workspace::persist_workspace_manifest(&workspace);
            let projection = workspace.projection();
            let _ = update_snapshot(&control, |snapshot| snapshot.workspace = projection);
        }
        let _ = update_snapshot(&control, |snapshot| {
            snapshot.finished_at_ms = Some(now_ms());
            snapshot.duration_ms = Some(elapsed_ms(started));
            snapshot.cancel_requested = control.cancel_requested.load(Ordering::SeqCst);
            snapshot.counters["performance"] = json!({
                "execution_ms": workspace.timings.execution_ms,
                "validation_ms": workspace.timings.validation_ms,
                "integration_ms": workspace.timings.integration_ms,
                "apply_back_ms": workspace.timings.apply_back_ms,
                "cleanup_ms": workspace.timings.cleanup_ms,
            });
            if let Some((status, kind, message)) = terminal_failure {
                snapshot.status = status.to_string();
                snapshot.failure =
                    Some(json!({"kind":kind,"category":"acceptance","message":message}));
            } else {
                snapshot.status = "completed".to_string();
            }
        });
        execute_task_result(&control)
    }

    async fn recovery_worktree_paths(&self, project: &str) -> Result<HashSet<String>, String> {
        let output = self
            .run_project_internal_posix_script_capture(
                project,
                "set -eu\ngit worktree list --porcelain -z\n".to_string(),
                30,
                Some(".".to_string()),
            )
            .await?;
        if output.exit_code != Some(0) {
            return Err(output
                .error
                .unwrap_or_else(|| "task_recovery_worktree_list_failed".to_string()));
        }
        Ok(output
            .stdout
            .split('\0')
            .filter_map(|field| field.strip_prefix("worktree "))
            .map(str::to_string)
            .collect())
    }

    async fn cleanup_recovered_workspace(
        &self,
        workspace: &ExecutionWorkspace,
        auth: Option<&AuthContext>,
    ) -> Result<(), String> {
        if workspace.mode == IsolationMode::None {
            let _ = super::execution_workspace::remove_workspace_manifest(workspace);
            return Ok(());
        }
        // A failed unregister/CAS is not permission for a path-only fallback.
        // Leave both the recovery handle and worktree for explicit inspection.
        self.cleanup_execution_workspace(workspace, auth).await?;
        super::execution_workspace::remove_workspace_manifest(workspace).map_err(|e| e.to_string())
    }

    pub(crate) async fn chadex_task_recovery(
        &self,
        resolved: ResolvedProject,
        task_id: Option<String>,
        action: String,
        auth: Option<&AuthContext>,
        transport: sessions::SessionTransport,
    ) -> ToolResult {
        let project = resolved.resolved_id.clone();
        gc_task_storage();
        match action.as_str() {
            "list" => {
                let worktree_paths = self
                    .recovery_worktree_paths(&project)
                    .await
                    .unwrap_or_default();
                let manifests = super::execution_workspace::load_workspace_manifests()
                    .into_iter()
                    .filter(|(_, value)| {
                        value.get("source_project").and_then(Value::as_str)
                            == Some(project.as_str())
                    })
                    .collect::<Vec<_>>();
                let known_roots = manifests
                    .iter()
                    .filter_map(|(_, value)| value.get("execution_root").and_then(Value::as_str))
                    .collect::<HashSet<_>>();
                let registered_workspace_count = manifests
                    .iter()
                    .filter(|(_, value)| {
                        value
                            .get("execution_root")
                            .and_then(Value::as_str)
                            .is_some_and(|root| worktree_paths.contains(root))
                    })
                    .count();
                let orphan_managed_worktree_count = worktree_paths
                    .iter()
                    .filter(|root| {
                        root.contains(".webcodex-managed-worktrees/")
                            && !known_roots.contains(root.as_str())
                    })
                    .count();
                return ToolResult::ok(json!({
                    "action": "list",
                    "project": project,
                    "recovered_tasks": self.chadex_tasks.recovered_for_project(&project),
                    "workspace_manifest_count": manifests.len(),
                    "registered_workspace_count": registered_workspace_count,
                    "orphan_managed_worktree_count": orphan_managed_worktree_count,
                    "state_changed": false,
                }));
            }
            "retry" => {
                let Some(task_id) = task_id else {
                    return ToolResult::err_with_output(
                        "task_recovery_task_id_required",
                        json!({"error_kind":"task_recovery_task_id_required","state_changed":false}),
                    );
                };
                let Some(recovered) = self.chadex_tasks.recovered(&task_id) else {
                    return ToolResult::err_with_output(
                        "recovered_task_not_found",
                        json!({"error_kind":"recovered_task_not_found","state_changed":false}),
                    );
                };
                if recovered.get("project").and_then(Value::as_str) != Some(project.as_str()) {
                    return ToolResult::err_with_output(
                        "task_project_mismatch",
                        json!({"error_kind":"task_project_mismatch","state_changed":false}),
                    );
                }
                if recovered.get("execution_state").and_then(Value::as_str) == Some("unknown")
                    || recovered.get("status").and_then(Value::as_str) == Some("unknown")
                {
                    return ToolResult::err_with_output(
                        "unknown_execution_not_retryable",
                        json!({"error_kind":"unknown_execution_not_retryable", "state_changed":false}),
                    );
                }
                let Some(plan) = self
                    .chadex_tasks
                    .execution_root()
                    .ok()
                    .and_then(|root| load_task_recovery_plan_at(&root, &task_id))
                else {
                    return ToolResult::err_with_output(
                        "task_recovery_plan_unavailable",
                        json!({"error_kind":"task_recovery_plan_unavailable","state_changed":false}),
                    );
                };
                if plan.project != project {
                    return ToolResult::err_with_output(
                        "task_project_mismatch",
                        json!({"error_kind":"task_project_mismatch","state_changed":false}),
                    );
                }
                let mut result = self
                    .execute_chadex_task_attempt(
                        resolved,
                        None,
                        plan.goal,
                        plan.preplan,
                        plan.package_count,
                        plan.steps,
                        plan.policy,
                        plan.acceptance,
                        None,
                        auth,
                        transport,
                        Some(task_id.clone()),
                    )
                    .await;
                if result.success {
                    let new_task_id = result.output.get("task_id").cloned().unwrap_or(Value::Null);
                    let mut recovered_update = recovered;
                    recovered_update["recovery"]["last_action"] = json!("retry");
                    recovered_update["recovery"]["last_retry_task_id"] = new_task_id.clone();
                    recovered_update["recovery"]["last_action_ms"] = json!(now_ms());
                    persist_recovered_projection(&task_id, &recovered_update);
                    self.chadex_tasks
                        .update_recovered(&task_id, recovered_update);
                    result.output["recovery"] = json!({
                        "action": "retry",
                        "retried_from": task_id,
                        "new_task_id": new_task_id,
                    });
                }
                return result;
            }
            "discard" => {
                let Some(task_id) = task_id else {
                    return ToolResult::err_with_output(
                        "task_recovery_task_id_required",
                        json!({"error_kind":"task_recovery_task_id_required","state_changed":false}),
                    );
                };
                let Some(recovered) = self.chadex_tasks.recovered(&task_id) else {
                    return ToolResult::err_with_output(
                        "recovered_task_not_found",
                        json!({"error_kind":"recovered_task_not_found","state_changed":false}),
                    );
                };
                if recovered.get("project").and_then(Value::as_str) != Some(project.as_str()) {
                    return ToolResult::err_with_output(
                        "task_project_mismatch",
                        json!({"error_kind":"task_project_mismatch","state_changed":false}),
                    );
                }
                let workspaces = super::execution_workspace::load_workspace_manifests()
                    .into_iter()
                    .filter_map(|(_, value)| {
                        (value.get("task_id").and_then(Value::as_str) == Some(task_id.as_str()))
                            .then(|| super::execution_workspace::workspace_from_manifest(&value))
                    })
                    .collect::<Result<Vec<_>, _>>();
                let workspaces = match workspaces {
                    Ok(workspaces) => workspaces,
                    Err(error) => {
                        return ToolResult::err_with_output(
                            error.clone(),
                            json!({"error_kind":"task_recovery_manifest_invalid","message":error,"state_changed":false}),
                        )
                    }
                };
                let mut cleanup_errors = Vec::new();
                for workspace in &workspaces {
                    if workspace.source_project != project {
                        continue;
                    }
                    if let Err(error) = self.cleanup_recovered_workspace(workspace, auth).await {
                        cleanup_errors.push(error);
                    }
                }
                if !cleanup_errors.is_empty() {
                    return ToolResult::err_with_output(
                        "task_recovery_cleanup_failed",
                        json!({
                            "error_kind":"task_recovery_cleanup_failed",
                            "errors":cleanup_errors,
                            "state_changed":false,
                        }),
                    );
                }
                if let Ok(root) = self.chadex_tasks.execution_root() {
                    let claim = root.join("executions").join(&task_id);
                    if claim.is_dir() {
                        if let Err(error) =
                            write_private_file(&claim.join("discarded"), b"discarded")
                        {
                            return ToolResult::err(format!(
                                "execution_discard_receipt_failed: {error}"
                            ));
                        }
                    }
                }
                self.chadex_tasks.remove_recovered(&task_id);
                remove_task_persistence(&task_id);
                return ToolResult::ok(json!({
                    "action":"discard",
                    "task_id":task_id,
                    "project":project,
                    "status":"discarded",
                    "workspace_count":workspaces.len(),
                    "state_changed":true,
                }));
            }
            _ => ToolResult::err_with_output(
                "task_recovery_invalid_action",
                json!({"error_kind":"task_recovery_invalid_action","allowed_actions":["list","retry","discard"],"state_changed":false}),
            ),
        }
    }

    pub(crate) fn observe_chadex_task(&self, project: &str, task_id: &str) -> ToolResult {
        let control = match self.chadex_tasks.get(task_id) {
            Ok(control) => control,
            Err(_) => {
                if let Some(value) = self.chadex_tasks.recovered(task_id) {
                    if value.get("project").and_then(Value::as_str) != Some(project) {
                        return ToolResult::err_with_output(
                            "task_project_mismatch",
                            json!({"error_kind":"task_project_mismatch","state_changed":false}),
                        );
                    }
                    return ToolResult::ok(value);
                }
                return ToolResult::err_with_output(
                    "task_not_found",
                    json!({"error_kind":"task_not_found","state_changed":false}),
                );
            }
        };
        let project_matches = control
            .snapshot
            .lock()
            .map(|snapshot| snapshot.project == project)
            .unwrap_or(false);
        if !project_matches {
            return ToolResult::err_with_output(
                "task_project_mismatch",
                json!({"error_kind":"task_project_mismatch","state_changed":false}),
            );
        }
        task_value(&control)
            .map(ToolResult::ok)
            .unwrap_or_else(ToolResult::err)
    }

    pub(crate) fn cancel_chadex_task(&self, project: &str, task_id: &str) -> ToolResult {
        let control = match self.chadex_tasks.get(task_id) {
            Ok(control) => control,
            Err(error) => {
                return ToolResult::err_with_output(
                    error.clone(),
                    json!({"error_kind":error,"state_changed":false}),
                )
            }
        };
        let project_matches = control
            .snapshot
            .lock()
            .map(|snapshot| snapshot.project == project)
            .unwrap_or(false);
        if !project_matches {
            return ToolResult::err_with_output(
                "task_project_mismatch",
                json!({"error_kind":"task_project_mismatch","state_changed":false}),
            );
        }
        control.cancel_requested.store(true, Ordering::SeqCst);
        let _ = update_snapshot(&control, |snapshot| {
            if !is_terminal(&snapshot.status) {
                snapshot.cancel_requested = true;
                snapshot.status = "cancelling".to_string();
            }
        });
        task_value(&control)
            .map(ToolResult::ok)
            .unwrap_or_else(ToolResult::err)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use webcodex_tool_runtime_contracts::ChadexTaskProcess;

    fn read_step(path: &str) -> ChadexTaskStep {
        ChadexTaskStep::Read {
            items: vec![ReadFilesItem {
                path: path.to_string(),
                start_line: None,
                limit: None,
                expected_read_revision: None,
            }],
            with_line_numbers: Some(false),
            max_result_bytes: Some(8 * 1024),
        }
    }

    fn validate_step() -> ChadexTaskStep {
        ChadexTaskStep::Validate {
            checks: vec![ChadexTaskProcess {
                executable: "git".to_string(),
                args: vec!["diff".to_string(), "--check".to_string()],
                stdin: None,
                cwd: None,
                timeout_secs: Some(10),
            }],
        }
    }

    fn review_step() -> ChadexTaskStep {
        ChadexTaskStep::Review {
            include_diff: Some(false),
            max_hunks: Some(8),
            max_hunk_lines: Some(80),
        }
    }

    fn sleep_step(seconds: u64) -> ChadexTaskStep {
        ChadexTaskStep::RunProcess {
            executable: "/bin/sleep".to_string(),
            args: vec![seconds.to_string()],
            stdin: None,
            cwd: None,
            timeout_secs: Some(seconds.saturating_add(5)),
            purpose: Some(ExecutionPurpose::Other),
        }
    }

    fn scoped_validate_step(path: &str) -> ChadexTaskStep {
        ChadexTaskStep::Validate {
            checks: vec![ChadexTaskProcess {
                executable: "python3".to_string(),
                args: vec!["-m".to_string(), "py_compile".to_string(), path.to_string()],
                stdin: None,
                cwd: None,
                timeout_secs: Some(10),
            }],
        }
    }

    fn edit_step(path: &str) -> ChadexTaskStep {
        ChadexTaskStep::Edit {
            changes: vec![webcodex_tool_runtime_contracts::ApplyFileChangeInput {
                kind: webcodex_tool_runtime_contracts::ApplyFileChangeKind::Edit,
                path: path.to_string(),
                to_path: None,
                content: None,
                edits: vec![webcodex_tool_runtime_contracts::ApplyTextEditInput {
                    kind: webcodex_tool_runtime_contracts::ApplyTextEditKind::ReplaceExact,
                    old_text: Some("before".to_string()),
                    new_text: Some("after".to_string()),
                    anchor_text: None,
                    occurrence: None,
                    line_scope: None,
                }],
                expected_read_revision: None,
            }],
            dry_run: Some(false),
        }
    }

    #[test]
    fn adaptive_parallel_cost_gate_rejects_short_and_accepts_heavy_packages() {
        let short_steps = vec![
            edit_step("a.py"),
            sleep_step(2),
            scoped_validate_step("a.py"),
            edit_step("b.py"),
            sleep_step(2),
            scoped_validate_step("b.py"),
            edit_step("c.py"),
            sleep_step(2),
            scoped_validate_step("c.py"),
            review_step(),
        ];
        let mut short = plan_task_packages(&short_steps, Some(3), 3).unwrap();
        short.package_parallelism_proven = true;
        apply_parallel_cost_gate(&mut short, &short_steps, 3, PARALLEL_BASE_OVERHEAD_MS, 0);
        assert_eq!(short.effective_package_count, 3);
        assert!(!short.parallel_cost_gate_passed);
        assert_eq!(
            short.parallel_cost_reason,
            "estimated_gain_below_15_percent"
        );
        assert!(short.estimated_parallel_gain_pct < PARALLEL_MIN_GAIN_PCT);

        let heavy_steps = vec![
            edit_step("a.py"),
            sleep_step(8),
            scoped_validate_step("a.py"),
            edit_step("b.py"),
            sleep_step(8),
            scoped_validate_step("b.py"),
            edit_step("c.py"),
            sleep_step(8),
            scoped_validate_step("c.py"),
            review_step(),
        ];
        let mut heavy = plan_task_packages(&heavy_steps, Some(3), 3).unwrap();
        heavy.package_parallelism_proven = true;
        apply_parallel_cost_gate(&mut heavy, &heavy_steps, 3, PARALLEL_BASE_OVERHEAD_MS, 0);
        assert!(heavy.parallel_cost_gate_passed);
        assert!(heavy.estimated_parallel_gain_pct >= PARALLEL_MIN_GAIN_PCT);
        assert!(heavy.estimated_parallel_ms < heavy.estimated_sequential_ms);
    }

    #[test]
    fn adaptive_parallel_overhead_ewma_learns_and_persists_local_cost() {
        let dir = tempfile::tempdir().unwrap();
        let first =
            update_parallel_overhead_telemetry(AdaptiveExecutionTelemetry::default(), 12_000, 3);
        assert_eq!(first.parallel_samples, 1);
        assert_eq!(first.parallel_overhead_base_ewma_ms.round() as u64, 9_000);
        assert!(persist_adaptive_execution_telemetry_at(dir.path(), &first));
        let loaded = load_adaptive_execution_telemetry_at(dir.path());
        assert_eq!(loaded.parallel_samples, 1);
        assert_eq!(loaded.parallel_overhead_base_ewma_ms.round() as u64, 9_000);

        let second = update_parallel_overhead_telemetry(loaded, 16_000, 3);
        assert_eq!(second.parallel_samples, 2);
        assert_eq!(second.parallel_overhead_base_ewma_ms.round() as u64, 10_000);
    }

    #[test]
    fn integration_validation_deduplicates_scoped_checks_but_keeps_global_checks() {
        let steps = vec![
            edit_step("src/a.py"),
            scoped_validate_step("src/a.py"),
            edit_step("src/b.py"),
            scoped_validate_step("src/b.py"),
            ChadexTaskStep::RunProcess {
                executable: "cargo".to_string(),
                args: vec!["test".to_string()],
                stdin: None,
                cwd: None,
                timeout_secs: Some(120),
                purpose: Some(ExecutionPurpose::Test),
            },
            review_step(),
        ];
        let packaging = plan_task_packages(&steps, Some(2), 2).unwrap();
        let package_files = packaging
            .packages
            .iter()
            .map(|package| {
                (package.start_step..=package.end_step)
                    .flat_map(|index| dependency_paths_for_step(&steps[index]))
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();
        let (checks, deduplicated) =
            integration_checks_for_parallel(&steps, &packaging, &package_files);
        assert!(deduplicated >= 1);
        assert!(checks.iter().any(|step| matches!(
            step,
            ChadexTaskStep::RunProcess {
                purpose: Some(ExecutionPurpose::Test),
                ..
            }
        )));
        assert!(checks
            .iter()
            .any(|step| matches!(step, ChadexTaskStep::Review { .. })));
    }

    #[test]
    fn startup_recovery_marks_inflight_task_interrupted_and_retryable() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state");
        fs::create_dir_all(&state_dir).unwrap();
        let task_id = "chadex_task_0123456789abcdef0123456789abcdef";
        let state = json!({
            "task_id": task_id,
            "project": "project-recovery",
            "source_path": "/tmp/project-recovery",
            "goal": "recover me",
            "status": "running",
            "started_at_ms": 1234,
            "workspace": {
                "mode": "task",
                "state": "active",
                "source_project": "project-recovery",
                "execution_project": "execution-recovery",
                "worktree_created": true
            }
        });
        write_private_file(
            &state_dir.join(format!("{task_id}.json")),
            &serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        let plan = TaskRecoveryPlan {
            version: 1,
            project: "project-recovery".to_string(),
            task_id: task_id.to_string(),
            goal: "recover me".to_string(),
            preplan: None,
            package_count: Some(1),
            steps: vec![read_step("README.md")],
            policy: Some(ChadexTaskPolicy::default()),
            acceptance: Some(ChadexTaskAcceptance::default()),
            created_at_ms: 1234,
        };
        assert!(persist_task_recovery_plan_at(dir.path(), &plan));
        let recovery_path = task_recovery_plan_path(dir.path(), task_id);
        let recovery_bytes = fs::read(&recovery_path).unwrap();
        let decoded =
            serde_json::from_slice::<TaskRecoveryPlan>(&recovery_bytes).unwrap_or_else(|error| {
                panic!(
                    "recovery plan must deserialize: {error}; {}",
                    String::from_utf8_lossy(&recovery_bytes)
                )
            });
        assert_eq!(decoded.task_id, task_id);
        assert!(load_task_recovery_plan_at(dir.path(), task_id).is_some());

        let recovered = load_recovered_task_projections_at(dir.path());
        let value = recovered.get(task_id).expect("recovered task");
        assert_eq!(value["status"], "interrupted");
        assert_eq!(value["failure"]["kind"], "runtime_interrupted");
        assert_eq!(value["workspace"]["state"], "preserved");
        assert_eq!(value["recovery"]["plan_available"], true);
        assert_eq!(value["recovery"]["actions"], json!(["retry", "discard"]));

        let persisted: Value =
            serde_json::from_slice(&fs::read(state_dir.join(format!("{task_id}.json"))).unwrap())
                .unwrap();
        assert_eq!(persisted["status"], "interrupted");
        let latest: Value =
            serde_json::from_slice(&fs::read(state_dir.join("latest.json")).unwrap()).unwrap();
        assert_eq!(latest["task_id"], task_id);
        assert_eq!(latest["status"], "interrupted");

        let reloaded = load_recovered_task_projections_at(dir.path());
        assert_eq!(reloaded.get(task_id).unwrap()["status"], "interrupted");
    }

    #[test]
    fn startup_recovery_ignores_completed_active_projection_and_removes_stale_latest() {
        let dir = tempfile::tempdir().unwrap();
        let state_dir = dir.path().join("state");
        fs::create_dir_all(&state_dir).unwrap();
        let task_id = "chadex_task_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab";
        let completed = json!({
            "task_id": task_id,
            "project": "project-completed",
            "source_path": "/tmp/project-completed",
            "goal": "already done",
            "status": "completed",
            "started_at_ms": 1234,
            "finished_at_ms": 1250,
            "workspace": {
                "mode": "none",
                "state": "active",
                "source_project": "project-completed",
                "execution_project": "project-completed",
                "worktree_created": false
            },
            "recovery": {
                "available": true,
                "plan_available": true,
                "actions": ["retry", "discard"],
                "restored_from_disk": true
            }
        });
        let bytes = serde_json::to_vec(&completed).unwrap();
        write_private_file(&state_dir.join(format!("{task_id}.json")), &bytes).unwrap();
        write_private_file(&state_dir.join("latest.json"), &bytes).unwrap();

        let recovered = load_recovered_task_projections_at(dir.path());
        assert!(recovered.is_empty());
        assert!(!state_dir.join("latest.json").exists());
        let persisted: Value =
            serde_json::from_slice(&fs::read(state_dir.join(format!("{task_id}.json"))).unwrap())
                .unwrap();
        assert_eq!(persisted["status"], "completed");
    }

    #[test]
    fn packaging_caps_request_to_complexity_recommendation() {
        let plan = plan_task_packages(&[read_step("src/lib.rs")], Some(4), 1).unwrap();
        assert_eq!(plan.requested_package_count, 4);
        assert_eq!(plan.recommended_package_count, 1);
        assert_eq!(plan.effective_package_count, 1);
        assert!(plan.limited_by_recommendation);
        assert_eq!(plan.outer_execute_task_count, 1);
    }

    #[test]
    fn packaging_uses_only_safe_validation_boundaries() {
        let steps = vec![
            read_step("runtime/a.rs"),
            validate_step(),
            read_step("helper/b.rs"),
            validate_step(),
            read_step("ui/App.swift"),
            validate_step(),
            ChadexTaskStep::Review {
                include_diff: Some(false),
                max_hunks: Some(8),
                max_hunk_lines: Some(80),
            },
        ];
        let plan = plan_task_packages(&steps, Some(3), 3).unwrap();
        assert_eq!(plan.safe_boundary_count, 2);
        assert_eq!(plan.effective_package_count, 3);
        assert_eq!(plan.packages[0].start_step, 0);
        assert_eq!(plan.packages[0].end_step, 1);
        assert_eq!(plan.packages[1].start_step, 2);
        assert_eq!(plan.packages[1].end_step, 3);
        assert_eq!(plan.packages[2].start_step, 4);
        assert_eq!(plan.packages[2].end_step, 6);
        assert!(!plan.limited_by_safe_boundaries);
    }

    #[test]
    fn packaging_degrades_when_no_safe_boundary_exists() {
        let steps = vec![read_step("a.rs"), read_step("b.rs"), read_step("c.rs")];
        let plan = plan_task_packages(&steps, Some(3), 3).unwrap();
        assert_eq!(plan.effective_package_count, 1);
        assert!(plan.limited_by_safe_boundaries);
        assert_eq!(plan.outer_execute_task_count, 1);
    }

    #[test]
    fn integration_rechecks_validation_like_processes() {
        let build = ChadexTaskStep::RunProcess {
            executable: "cargo".to_string(),
            args: vec!["check".to_string()],
            stdin: None,
            cwd: None,
            timeout_secs: Some(10),
            purpose: Some(ExecutionPurpose::Build),
        };
        let diagnostic = ChadexTaskStep::RunProcess {
            executable: "python3".to_string(),
            args: Vec::new(),
            stdin: None,
            cwd: None,
            timeout_secs: Some(10),
            purpose: Some(ExecutionPurpose::Diagnostic),
        };
        assert!(is_integration_check(&build));
        assert!(!is_integration_check(&diagnostic));
        assert!(is_integration_check(&review_step()));
    }

    #[test]
    fn graphify_requires_large_complexity_and_two_or_more_packages() {
        let two = plan_task_packages(
            &[
                read_step("a.rs"),
                validate_step(),
                read_step("b.rs"),
                review_step(),
            ],
            Some(2),
            2,
        )
        .unwrap();
        assert_eq!(two.effective_package_count, 2);
        assert!(!should_use_graphify(&two, "medium"));
        assert!(should_use_graphify(&two, "large"));

        let three = plan_task_packages(
            &[
                read_step("a.rs"),
                validate_step(),
                read_step("b.rs"),
                validate_step(),
                read_step("c.rs"),
                review_step(),
            ],
            Some(3),
            3,
        )
        .unwrap();
        assert_eq!(three.effective_package_count, 3);
        assert!(should_use_graphify(&three, "very_large"));
    }

    #[test]
    fn graphify_strong_boundaries_merge_only_adjacent_packages() {
        let steps = vec![
            read_step("AppModel.swift"),
            validate_step(),
            read_step("HelperClient.swift"),
            validate_step(),
            read_step("runtime_bridge.rs"),
            validate_step(),
            read_step("runtime_backend.rs"),
            review_step(),
        ];
        let mut plan = plan_task_packages(&steps, Some(4), 4).unwrap();
        assert_eq!(plan.effective_package_count, 4);
        apply_graphify_dependency_evidence(
            &mut plan,
            GraphifyDependencyEvidence {
                status: "used".to_string(),
                fresh: true,
                analyzed_file_count: 4,
                missing_file_count: 0,
                boundaries: vec![
                    GraphifyBoundaryEvidence {
                        boundary_after_step: 1,
                        direct_edges: 2,
                        shared_neighbor_count: 0,
                        strong: true,
                    },
                    GraphifyBoundaryEvidence {
                        boundary_after_step: 3,
                        direct_edges: 0,
                        shared_neighbor_count: 0,
                        strong: false,
                    },
                    GraphifyBoundaryEvidence {
                        boundary_after_step: 5,
                        direct_edges: 0,
                        shared_neighbor_count: 4,
                        strong: true,
                    },
                ],
                package_relations: Vec::new(),
                parallelism_proven: false,
            },
            12,
        );
        assert_eq!(plan.effective_package_count, 2);
        assert_eq!(plan.dependency_merge_count, 2);
        assert!(plan.graphify_adjusted);
        assert_eq!(plan.packages[0].start_step, 0);
        assert_eq!(plan.packages[0].end_step, 3);
        assert_eq!(plan.packages[1].start_step, 4);
        assert_eq!(plan.packages[1].end_step, 7);
    }

    #[test]
    fn graphify_stale_evidence_never_changes_packaging() {
        let steps = vec![
            read_step("a.rs"),
            validate_step(),
            read_step("b.rs"),
            review_step(),
        ];
        let mut plan = plan_task_packages(&steps, Some(2), 2).unwrap();
        apply_graphify_dependency_evidence(
            &mut plan,
            GraphifyDependencyEvidence {
                status: "graph_stale".to_string(),
                fresh: false,
                analyzed_file_count: 2,
                missing_file_count: 0,
                boundaries: vec![GraphifyBoundaryEvidence {
                    boundary_after_step: 1,
                    direct_edges: 10,
                    shared_neighbor_count: 10,
                    strong: true,
                }],
                package_relations: Vec::new(),
                parallelism_proven: false,
            },
            7,
        );
        assert_eq!(plan.effective_package_count, 2);
        assert_eq!(plan.dependency_merge_count, 0);
        assert!(!plan.graphify_adjusted);
        assert_eq!(plan.graphify_status, "graph_stale");
        assert!(!plan.package_parallelism_proven);
    }

    #[test]
    fn complete_graphify_evidence_proves_disjoint_packages() {
        let steps = vec![
            read_step("a.rs"),
            validate_step(),
            read_step("b.rs"),
            review_step(),
        ];
        let mut plan = plan_task_packages(&steps, Some(2), 2).unwrap();
        apply_graphify_dependency_evidence(
            &mut plan,
            GraphifyDependencyEvidence {
                status: "used".to_string(),
                fresh: true,
                analyzed_file_count: 2,
                missing_file_count: 0,
                boundaries: vec![GraphifyBoundaryEvidence {
                    boundary_after_step: 1,
                    direct_edges: 0,
                    shared_neighbor_count: 0,
                    strong: false,
                }],
                package_relations: vec![GraphifyPackageRelation {
                    left_package: 0,
                    right_package: 1,
                    direct_edges: 0,
                    shared_neighbor_count: 0,
                    strong: false,
                }],
                parallelism_proven: true,
            },
            3,
        );
        assert!(plan.package_parallelism_proven);
        assert_eq!(
            plan.dependency_proof,
            "graphify_no_cross_package_dependency"
        );
    }

    #[test]
    fn incomplete_or_non_adjacent_graphify_dependency_never_proves_parallelism() {
        let steps = vec![
            read_step("a.rs"),
            validate_step(),
            read_step("b.rs"),
            validate_step(),
            read_step("c.rs"),
            review_step(),
        ];
        let mut plan = plan_task_packages(&steps, Some(3), 3).unwrap();
        apply_graphify_dependency_evidence(
            &mut plan,
            GraphifyDependencyEvidence {
                status: "used".to_string(),
                fresh: true,
                analyzed_file_count: 3,
                missing_file_count: 0,
                boundaries: Vec::new(),
                package_relations: vec![
                    GraphifyPackageRelation {
                        left_package: 0,
                        right_package: 1,
                        direct_edges: 0,
                        shared_neighbor_count: 0,
                        strong: false,
                    },
                    GraphifyPackageRelation {
                        left_package: 0,
                        right_package: 2,
                        direct_edges: 1,
                        shared_neighbor_count: 0,
                        strong: true,
                    },
                    GraphifyPackageRelation {
                        left_package: 1,
                        right_package: 2,
                        direct_edges: 0,
                        shared_neighbor_count: 0,
                        strong: false,
                    },
                ],
                parallelism_proven: false,
            },
            3,
        );
        assert!(!plan.package_parallelism_proven);
        assert_eq!(
            plan.dependency_proof,
            "graphify_dependency_or_incomplete_proof"
        );
    }

    #[test]
    fn complexity_small_task_recommends_one_task() {
        let assessment = assess_task_complexity(None, &[read_step("src/lib.rs")], 0, 0);
        assert_eq!(assessment.score, 1);
        assert_eq!(assessment.band, "small");
        assert_eq!(assessment.recommended_task_count, 1);
        assert_eq!(assessment.execution_task_count, 1);
        assert!(!assessment.advisory_only);
    }

    #[test]
    fn complexity_medium_task_recommends_two_tasks_but_executes_one() {
        let steps = vec![
            read_step("Sources/App.swift"),
            read_step("Sources/Model.swift"),
            read_step("Tests/AppTests.swift"),
            read_step("Sources/Helper.swift"),
        ];
        let assessment = assess_task_complexity(None, &steps, 2, 3);
        assert!((4..=6).contains(&assessment.score));
        assert_eq!(assessment.band, "medium");
        assert_eq!(assessment.recommended_task_count, 2);
        assert_eq!(assessment.execution_task_count, 1);
    }

    #[test]
    fn complexity_cross_language_large_task_recommends_three_tasks_but_executes_one() {
        let steps = vec![
            read_step("Sources/App.swift"),
            read_step("Sources/Model.swift"),
            read_step("rust-helper/src/main.rs"),
            read_step("rust-helper/src/runtime.rs"),
            read_step("vendor/runtime/src/lib.rs"),
            read_step("Tests/AppTests.swift"),
            read_step("vendor/runtime/tests/task.rs"),
        ];
        let assessment = assess_task_complexity(None, &steps, 4, 7);
        assert_eq!(assessment.score, 8);
        assert_eq!(assessment.band, "large");
        assert_eq!(assessment.recommended_task_count, 3);
        assert_eq!(assessment.execution_task_count, 1);
        assert_eq!(assessment.signals["language_count"], 2);
        assert!(assessment.signals["subsystem_count"].as_u64().unwrap_or(0) >= 2);
    }

    #[test]
    fn complexity_band_reserves_four_tasks_for_very_large_scores() {
        assert_eq!(complexity_band(8), ("large", 3));
        assert_eq!(complexity_band(9), ("very_large", 4));
        assert_eq!(complexity_band(10), ("very_large", 4));
    }

    #[test]
    fn preplan_complexity_is_independent_of_execution_step_shape() {
        let preplan = ChadexTaskPreplan {
            estimated_file_count: 4,
            estimated_subsystem_count: 4,
            estimated_language_count: 1,
            validation_domain_count: 3,
            cross_runtime_boundary: false,
            stateful_or_schema_change: true,
            concurrency_or_security_sensitive: true,
            graphify_informed: true,
        };
        let coarse_steps = vec![read_step("taskdesk/store.py"), review_step()];
        let fine_steps = vec![
            read_step("taskdesk/store.py"),
            validate_step(),
            read_step("taskdesk/service.py"),
            validate_step(),
            read_step("taskdesk/bridge.py"),
            validate_step(),
            read_step("taskdesk/viewmodel.py"),
            validate_step(),
            review_step(),
        ];

        let coarse = assess_task_complexity(Some(&preplan), &coarse_steps, 1, 4);
        let fine = assess_task_complexity(Some(&preplan), &fine_steps, 4, 4);

        assert_eq!(coarse.score, 7);
        assert_eq!(fine.score, 7);
        assert_eq!(coarse.band, "large");
        assert_eq!(fine.band, "large");
        assert_eq!(coarse.recommended_task_count, 3);
        assert_eq!(fine.recommended_task_count, 3);
        assert_eq!(coarse.signals["estimation_source"], "preplan");
        assert_eq!(fine.signals["estimation_source"], "preplan");
        assert_ne!(
            coarse.signals["plan_shape_shadow_score"],
            fine.signals["plan_shape_shadow_score"]
        );
    }

    #[test]
    fn preplan_recommendation_becomes_default_package_target() {
        assert_eq!(initial_package_target(None, true, 3), Some(3));
        assert_eq!(initial_package_target(Some(2), true, 3), Some(2));
        assert_eq!(initial_package_target(None, false, 3), None);
    }

    #[test]
    fn invalid_preplan_counts_fail_closed() {
        let invalid = ChadexTaskPreplan {
            estimated_file_count: 0,
            estimated_subsystem_count: 1,
            estimated_language_count: 1,
            validation_domain_count: 0,
            cross_runtime_boundary: false,
            stateful_or_schema_change: false,
            concurrency_or_security_sensitive: false,
            graphify_informed: false,
        };
        assert_eq!(
            validate_task_preplan(&invalid),
            Err("preplan_estimated_file_count_out_of_range".to_string())
        );
    }

    #[test]
    fn scoped_path_policy_rejects_escape_and_accepts_children() {
        assert!(path_allowed("Sources/App.swift", &["Sources".to_string()]));
        assert!(path_allowed(
            "Sources/Nested/App.swift",
            &["Sources".to_string()]
        ));
        assert!(!path_allowed(
            "Tests/AppTests.swift",
            &["Sources".to_string()]
        ));
        assert!(!path_allowed("../Secrets.txt", &["Sources".to_string()]));
        assert!(!path_allowed("/tmp/Secrets.txt", &[]));
    }

    #[test]
    fn normalized_policy_is_bounded_and_closed() {
        let policy = normalize_policy(Some(ChadexTaskPolicy {
            max_steps: Some(20),
            max_mutations: Some(8),
            max_changed_files: Some(32),
            timeout_secs: Some(600),
            max_result_bytes: Some(262_144),
            allowed_path_prefixes: vec!["Sources/".to_string(), "Sources".to_string()],
            allowed_operations: vec!["read".to_string(), "edit".to_string()],
        }))
        .unwrap();
        assert_eq!(policy.allowed_path_prefixes, vec!["Sources"]);
        assert_eq!(policy.allowed_operations, vec!["edit", "read"]);
    }

    #[test]
    fn scoped_policy_rejects_workspace_wide_review() {
        let policy = normalize_policy(Some(ChadexTaskPolicy {
            allowed_path_prefixes: vec!["Sources".to_string()],
            ..Default::default()
        }))
        .unwrap();
        let steps = vec![ChadexTaskStep::Review {
            include_diff: Some(false),
            max_hunks: None,
            max_hunk_lines: None,
        }];
        let error = preflight_task("review scoped work", &steps, &policy).unwrap_err();
        assert_eq!(error, "path_scoped_task_cannot_review");
    }

    #[test]
    fn review_summary_prefers_authoritative_total_when_file_list_is_truncated() {
        let result = ToolResult::ok(json!({
            "clean": false,
            "files_total": 12,
            "files": [{"path": "returned-only.txt"}],
            "files_truncated": true,
            "diff_stat": "12 files changed",
            "counts": {"conflicted": 0}
        }));
        let summary = review_summary(&result);
        assert_eq!(summary["changed_file_count"], 12);
        assert_eq!(summary["changed_paths"], json!(["returned-only.txt"]));
    }

    #[test]
    fn compact_projection_reduces_result_bytes_and_respects_budget() {
        let policy = normalize_policy(Some(ChadexTaskPolicy {
            max_result_bytes: Some(MIN_MAX_RESULT_BYTES),
            ..Default::default()
        }))
        .unwrap();
        let steps = (0..20)
            .map(|index| TaskStepSummary {
                index,
                kind: "read".to_string(),
                status: "completed".to_string(),
                duration_ms: 3,
                attempts: 1,
                retries: 0,
                result_bytes_before: 4_096,
                result_bytes_after: 64,
                summary: Some(json!({"verbose": "x".repeat(4_000)})),
            })
            .collect::<Vec<_>>();
        let execution_dir = tempfile::tempdir().unwrap();
        let snapshot = TaskSnapshot {
            execution: Execution::claim(
                execution_dir.path(),
                "chadex_task_0123456789abcdef0123456789abcdef",
                None,
            )
            .unwrap(),
            task_id: "chadex_task_0123456789abcdef0123456789abcdef".to_string(),
            project: "agent:test:fixture".to_string(),
            source_path: "/tmp/fixture".to_string(),
            goal: "g".repeat(4_000),
            status: "completed".to_string(),
            current_step: 20,
            total_steps: 20,
            completed_steps: 20,
            plan: vec!["read".to_string(); 20],
            cancel_requested: false,
            started_at_ms: 1,
            finished_at_ms: Some(2),
            duration_ms: Some(1),
            limits: policy,
            counters: json!({"retries": 0}),
            validation: json!({"status":"passed","checks_passed":1}),
            review: json!({
                "status":"completed",
                "changed_file_count":32,
                "changed_paths": (0..32).map(|index| format!("src/{index}.rs")).collect::<Vec<_>>(),
                "diff_stat":"32 files changed"
            }),
            steps,
            failure: None,
            complexity: TaskComplexityAssessment {
                score: 3,
                band: "small",
                recommended_task_count: 1,
                execution_task_count: 1,
                advisory_only: false,
                signals: json!({"step_count":20}),
            },
            packaging: TaskPackagingPlan {
                requested_package_count: 1,
                recommended_package_count: 1,
                effective_package_count: 1,
                safe_boundary_count: 0,
                limited_by_recommendation: false,
                limited_by_safe_boundaries: false,
                outer_execute_task_count: 1,
                graphify_status: "not_requested".to_string(),
                graphify_fresh: None,
                graphify_adjusted: false,
                dependency_merge_count: 0,
                graphify_planning_ms: 0,
                graphify_analyzed_file_count: 0,
                graphify_missing_file_count: 0,
                dependency_boundaries: Vec::new(),
                dependency_proof: "unknown".to_string(),
                package_parallelism_proven: false,
                parallel_cost_gate_passed: false,
                parallel_cost_reason: "not_evaluated".to_string(),
                estimated_sequential_ms: 0,
                estimated_parallel_ms: 0,
                estimated_parallel_gain_pct: 0.0,
                estimated_parallel_overhead_ms: 0,
                parallel_cost_source: "cold_prior".to_string(),
                parallel_overhead_samples: 0,
                packages: vec![TaskPackageRange {
                    index: 0,
                    start_step: 0,
                    end_step: 19,
                }],
            },
            workspace: json!({"mode":"none","state":"cleaned"}),
        };
        let before = serde_json::to_vec(&snapshot).unwrap().len();
        let compact = compact_task_projection(&snapshot);
        let after = serialized_len(&compact);
        assert!(
            after <= MIN_MAX_RESULT_BYTES,
            "compact result is {after} bytes"
        );
        assert!(after < before, "expected {after} < {before}");
        assert!(compact["goal_truncated"].as_bool().unwrap_or(false));
        assert_eq!(compact["packaging"]["outer_execute_task_count"], 1);
        assert_eq!(compact["packaging"]["effective_package_count"], 1);
        assert!(
            compact.get("source_path").is_none(),
            "private recovery path must not enter tool output"
        );
        assert!(
            compact["steps"]
                .as_array()
                .unwrap()
                .iter()
                .all(|step| step.get("summary").is_none()),
            "successful read summaries should be omitted"
        );
    }

    #[test]
    fn failure_details_preserve_actionable_diagnostics_but_bound_stdio() {
        let result = ToolResult::err_with_output(
            "validation failed",
            json!({
                "exit_code": 17,
                "job_id": "job-123",
                "execution_state": "completed",
                "stderr_tail": "e".repeat(MAX_FAILURE_STDIO_CHARS + 500),
                "stdout_truncated": true,
                "stderr_truncated": true,
                "state_changed": false
            }),
        );
        let summary = json!({"failed_check": 2});
        let failure = compact_failure_details(&result, Some(&summary), 4, "validate", 1, 0, true);
        assert_eq!(failure["category"], "validation");
        assert_eq!(failure["exit_code"], 17);
        assert_eq!(failure["failed_check"], 2);
        assert_eq!(failure["job_id"], "job-123");
        assert_eq!(failure["stdout_truncated"], true);
        assert_eq!(failure["stderr_truncated"], true);
        assert_eq!(failure["raw_log_retained"], true);
        assert!(
            failure["stderr_tail"].as_str().unwrap().chars().count() <= MAX_FAILURE_STDIO_CHARS
        );
    }

    #[test]
    fn retry_policy_is_bounded_to_read_only_transient_failures() {
        let disconnected = ToolResult::err_with_output(
            "runner disconnected",
            json!({
                "error_kind":"runner_disconnected",
                "execution_state":"outcome_unknown",
                "state_changed":false
            }),
        );
        assert!(retryable_read_only_failure("read", &disconnected));
        assert!(retryable_read_only_failure("search", &disconnected));
        assert!(!retryable_read_only_failure("edit", &disconnected));
        assert!(!retryable_read_only_failure("validate", &disconnected));

        let ambiguous_mutation = ToolResult::err_with_output(
            "mutation outcome unknown",
            json!({
                "failure_kind":"outcome_unknown",
                "execution_state":"outcome_unknown",
                "state_changed":true
            }),
        );
        assert_eq!(
            failure_category("edit", &ambiguous_mutation),
            "ambiguous_mutation"
        );
        assert!(!retryable_read_only_failure("edit", &ambiguous_mutation));
        assert_eq!(MAX_READ_ONLY_RETRIES, 1);
        // An explicit unknown marker is stronger than the transient failure category.
        assert!(!should_retry_read_only_failure("read", &disconnected, 0));
        let known_no_dispatch = ToolResult::err_with_output(
            "runner unavailable",
            json!({"error_kind":"runner_disconnected", "state_changed":false}),
        );
        assert!(should_retry_read_only_failure(
            "read",
            &known_no_dispatch,
            0
        ));
        assert!(!should_retry_read_only_failure(
            "read",
            &known_no_dispatch,
            1
        ));
        assert!(!should_retry_read_only_failure("read", &disconnected, 1));
    }

    #[test]
    fn failure_categories_keep_timeout_and_stale_write_distinct() {
        let timed_out = ToolResult::err_with_output(
            "timed out",
            json!({"failure_kind":"runner_timeout","state_changed":false}),
        );
        assert_eq!(failure_category("read", &timed_out), "timeout");

        let stale = ToolResult::err_with_output(
            "revision changed",
            json!({"error_kind":"stale_read_revision","state_changed":false}),
        );
        assert_eq!(failure_category("edit", &stale), "stale_write");
        assert!(!retryable_read_only_failure("edit", &stale));
    }

    #[test]
    fn invalid_policy_values_fail_closed() {
        assert!(valid_task_id(
            "chadex_task_0123456789abcdef0123456789abcdef"
        ));
        assert!(!valid_task_id(
            "chadex_task_0123456789ABCDEF0123456789ABCDEF"
        ));
        assert!(!valid_task_id("wc_task_0123456789abcdef0123456789abcdef"));
        let error = normalize_policy(Some(ChadexTaskPolicy {
            max_steps: Some(21),
            ..Default::default()
        }))
        .unwrap_err();
        assert_eq!(error, "max_steps_out_of_range");
        let error = normalize_policy(Some(ChadexTaskPolicy {
            allowed_path_prefixes: vec!["../outside".to_string()],
            ..Default::default()
        }))
        .unwrap_err();
        assert_eq!(error, "invalid_allowed_path_prefix");
    }
}
