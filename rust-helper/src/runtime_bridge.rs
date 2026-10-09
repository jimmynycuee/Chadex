use crate::chadex_core::activity::{sanitize_message, RuntimeActivityEntry};
use crate::chadex_core::computer_overlay::{self, ComputerOverlayHub};
use crate::chadex_core::computer_safety::ComputerControlMode;
use crate::chadex_core::external_skills::{discover_external_skill_sources, DiscoveryEnv};
use crate::chadex_core::graphify::GraphifyStatus;
use crate::chadex_core::performance::{
    duration_us, now_ms, LifecyclePerformanceTrace, McpPerformanceTrace, PerformanceTraceStore,
};
use crate::chadex_core::runtime::{
    ChadexRuntimeCore, RuntimeMascotJob, RuntimeProject, RuntimeProxyMode, RuntimeSnapshot,
};
use crate::chadex_core::tunnel::{RuntimeTunnelTarget, TunnelManager, TunnelSnapshot, TunnelState};
use crate::chadex_core::verification::VerificationTracker;
use crate::chadex_core::ChadexError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::Instant;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, watch, Mutex};
use tokio::task::JoinSet;
use zeroize::Zeroizing;

const PROTOCOL_VERSION: u32 = 1;
const ACTIVITY_QUERY_MAX: usize = 200;
const PERFORMANCE_QUERY_MAX: usize = 100;
const TASK_PROGRESS_MAX_BYTES: u64 = 64 * 1024;
const MASCOT_JOBS_OBSERVATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const MAX_IN_FLIGHT_REQUESTS: usize = 64;
const SHUTDOWN_REQUEST_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const MAX_REQUEST_FRAME_BYTES: usize = 256 * 1024;
// Cancelling a launch warm-up retries until the operation has unwound; bounded
// so a stuck warm-up can never block the request that cancelled it.
const PREWARM_CANCEL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(12);
const PREWARM_CANCEL_RETRY: std::time::Duration = std::time::Duration::from_millis(40);
// A project switch or realignment must answer before the App stops waiting
// for it (60s / 45s): otherwise the helper could finish a switch the App
// already reported as failed, leaving the App and the runtime on different
// projects. Waiting for another project operation and the activation itself
// are therefore bounded; an activation past its bound is cancelled through the
// coordinator (never dropped, so its operation slot is always released).
const PROJECT_SWITCH_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(10);
const PROJECT_ACTIVATION_DEADLINE: std::time::Duration = std::time::Duration::from_secs(25);

#[derive(Debug, Deserialize)]
struct Request {
    protocol_version: u32,
    request_id: String,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct Response {
    protocol_version: u32,
    request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<ResponseResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorPayload>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
enum ResponseResult {
    Snapshot(BackendSnapshot),
    Project(ProjectInspection),
    Activities(Vec<RuntimeActivityEntry>),
    Performance(Vec<McpPerformanceTrace>),
    LifecyclePerformance(Vec<LifecyclePerformanceTrace>),
    Json(Value),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct ErrorPayload {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    recovery: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

impl ErrorPayload {
    fn new(
        code: impl Into<String>,
        message: impl Into<String>,
        recovery: impl Into<String>,
    ) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            recovery: Some(recovery.into()),
            details: None,
        }
    }

    fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl From<ChadexError> for ErrorPayload {
    fn from(error: ChadexError) -> Self {
        Self {
            code: error.code,
            message: error.message,
            recovery: Some(error.recovery),
            details: error.details,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ConnectionPhase {
    Unconfigured,
    Preparing,
    #[serde(rename = "waiting_for_chatgpt_verification")]
    WaitingForChatGptVerification,
    Verified,
    Stopped,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ProjectInspection {
    path: String,
    allowed_root: String,
    is_git_repository: bool,
    readable: bool,
    writable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct OperationSnapshot {
    id: String,
    kind: String,
    phase: String,
    started_at_ms: u64,
    cancellable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TaskProgressStep {
    index: usize,
    kind: String,
    status: String,
    duration_ms: u64,
    #[serde(default)]
    attempts: usize,
    #[serde(default)]
    retries: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TaskValidationSummary {
    status: String,
    #[serde(default)]
    checks_passed: u64,
    #[serde(default)]
    checks_failed: u64,
    #[serde(default)]
    failed_check: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TaskReviewSummary {
    status: String,
    #[serde(default)]
    changed_file_count: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TaskProgressSnapshot {
    task_id: String,
    project: String,
    goal: String,
    status: String,
    current_step: usize,
    total_steps: usize,
    completed_steps: usize,
    #[serde(default)]
    plan: Vec<String>,
    cancel_requested: bool,
    started_at_ms: u64,
    #[serde(default)]
    finished_at_ms: Option<u64>,
    #[serde(default)]
    duration_ms: Option<u64>,
    validation: TaskValidationSummary,
    review: TaskReviewSummary,
    #[serde(default)]
    steps: Vec<TaskProgressStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct BackendSnapshot {
    phase: ConnectionPhase,
    graphify: GraphifyStatus,
    selected_project: Option<ProjectInspection>,
    tunnel_ready: bool,
    chat_gpt_connected: bool,
    chat_gpt_verified_for_selected_project: bool,
    last_verified_at_ms: Option<u64>,
    current_operation: Option<OperationSnapshot>,
    task_progress: Option<TaskProgressSnapshot>,
    #[serde(default)]
    mascot_jobs: Option<Vec<RuntimeMascotJob>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    runtime_status: Option<RuntimeStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tunnel_status: Option<TunnelStatus>,
    error: Option<ErrorPayload>,
    activity_sequence: u64,
    state_revision: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct RuntimeStatus {
    runtime_configured: bool,
    runtime_ready: bool,
    needs_attention: bool,
    summary: String,
    next_action: Option<String>,
    summary_kind: String,
    server: String,
    runner: String,
    exposure: String,
    project: String,
}

impl RuntimeStatus {
    fn from_snapshot(snapshot: &RuntimeSnapshot) -> Self {
        Self {
            runtime_configured: snapshot.runtime_configured,
            runtime_ready: snapshot.readiness.runtime_ready,
            needs_attention: snapshot.readiness.needs_attention,
            summary: snapshot.readiness.summary.clone(),
            next_action: snapshot.readiness.next_action.clone(),
            summary_kind: snapshot.readiness.summary_kind.to_string(),
            server: snapshot.readiness.server.to_string(),
            runner: snapshot.readiness.runner.to_string(),
            exposure: snapshot.readiness.exposure.to_string(),
            project: snapshot.readiness.project.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct TunnelStatus {
    configured: bool,
    state: TunnelState,
}

impl TunnelStatus {
    fn from_snapshot(snapshot: &TunnelSnapshot) -> Self {
        Self {
            configured: snapshot.configured,
            state: snapshot.state,
        }
    }
}

impl BackendSnapshot {
    fn semantically_eq(&self, other: &Self) -> bool {
        self.phase == other.phase
            && self.graphify == other.graphify
            && self.selected_project == other.selected_project
            && self.tunnel_ready == other.tunnel_ready
            && self.chat_gpt_connected == other.chat_gpt_connected
            && self.chat_gpt_verified_for_selected_project
                == other.chat_gpt_verified_for_selected_project
            && self.last_verified_at_ms == other.last_verified_at_ms
            && self.current_operation == other.current_operation
            && self.task_progress == other.task_progress
            && self.mascot_jobs == other.mascot_jobs
            && self.runtime_status == other.runtime_status
            && self.tunnel_status == other.tunnel_status
            && self.error == other.error
            && self.activity_sequence == other.activity_sequence
    }
}

#[derive(Default)]
struct SnapshotRevisionState {
    revision: u64,
    last_snapshot: Option<BackendSnapshot>,
}

impl SnapshotRevisionState {
    fn assign_revision(&mut self, mut candidate: BackendSnapshot) -> BackendSnapshot {
        let changed = self
            .last_snapshot
            .as_ref()
            .map(|previous| !previous.semantically_eq(&candidate))
            .unwrap_or(true);
        if changed {
            self.revision = self.revision.saturating_add(1);
        }
        candidate.state_revision = self.revision;
        self.last_snapshot = Some(candidate.clone());
        candidate
    }
}

struct ScopedMascotJobs {
    project_path: String,
    epoch: u64,
    jobs: Vec<RuntimeMascotJob>,
}

impl ScopedMascotJobs {
    fn for_target(self, path: Option<&str>, epoch: u64) -> Option<Vec<RuntimeMascotJob>> {
        (path == Some(self.project_path.as_str()) && epoch == self.epoch).then_some(self.jobs)
    }
}

/// How a request interacts with an in-flight launch warm-up. Only requests that
/// actually contend for the runtime mutation slot are affected; everything else
/// proceeds immediately and leaves the warm-up running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrewarmInteraction {
    /// Benefits from the warm-up: wait for it, then continue (Connect then only
    /// has to start the tunnel).
    Join,
    /// Conflicts with or invalidates the warm-up: cancel it and proceed at once.
    Cancel,
    Ignore,
}

fn prewarm_interaction(method: &str) -> PrewarmInteraction {
    match method {
        "connectChatGPT" | "startTunnel" | "configureLocalSetup" | "resumeService"
        | "updateProxySettings" | "realignLocalProject" => PrewarmInteraction::Join,
        "stopLocalService" | "stopTunnel" | "disconnectAI" | "switchLocalProject" => {
            PrewarmInteraction::Cancel
        }
        _ => PrewarmInteraction::Ignore,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrewarmDecision {
    Resume,
    NotConfigured,
    ExplicitlyStopped,
    NoRuntimeProject,
    AlreadyReady,
    OperationInFlight,
    NoTargetProject,
}

impl PrewarmDecision {
    /// Trace completion for a warm-up that did not run. Fixed vocabulary only.
    fn skip_completion(self) -> &'static str {
        match self {
            PrewarmDecision::Resume => "completed",
            PrewarmDecision::NotConfigured => "skipped:not_configured",
            PrewarmDecision::ExplicitlyStopped => "skipped:explicit_stop",
            PrewarmDecision::NoRuntimeProject => "skipped:no_runtime_project",
            PrewarmDecision::AlreadyReady => "skipped:already_ready",
            PrewarmDecision::OperationInFlight => "skipped:operation_in_flight",
            PrewarmDecision::NoTargetProject => "skipped:no_project",
        }
    }
}

/// Launch warm-up resumes only a runtime the user already set up and did not
/// explicitly stop, when nothing else is mutating it.
fn prewarm_decision(current: &RuntimeSnapshot, has_target_project: bool) -> PrewarmDecision {
    if !current.runtime_configured {
        PrewarmDecision::NotConfigured
    } else if !current.runtime_autostart {
        PrewarmDecision::ExplicitlyStopped
    } else if current.project.is_none() {
        PrewarmDecision::NoRuntimeProject
    } else if current.readiness.runtime_ready {
        PrewarmDecision::AlreadyReady
    } else if current.current_operation.is_some() {
        PrewarmDecision::OperationInFlight
    } else if !has_target_project {
        PrewarmDecision::NoTargetProject
    } else {
        PrewarmDecision::Resume
    }
}

/// After a launch warm-up resumed the runtime, the project it must still
/// activate: the selected project, when the runtime came up serving another
/// one. `None` when there is no selection, the runtime is not ready, or it
/// already serves the selection.
fn prewarm_activation_path<'a>(
    runtime: &RuntimeSnapshot,
    target: Option<&'a ProjectInspection>,
) -> Option<&'a str> {
    let target = target?;
    if !runtime.readiness.runtime_ready {
        return None;
    }
    let serves_target = runtime
        .project
        .as_ref()
        .is_some_and(|project| project.path == target.path);
    (!serves_target).then_some(target.path.as_str())
}

/// What a launch warm-up needs from the runtime. Implemented by the real
/// runtime core; tests drive the same orchestration against a fake.
trait PrewarmPort {
    async fn resume(&self) -> Result<RuntimeSnapshot, ChadexError>;
    fn runtime_snapshot(&self) -> RuntimeSnapshot;
    async fn activate(&self, path: &str) -> Result<RuntimeSnapshot, ChadexError>;
}

impl PrewarmPort for ChadexRuntimeCore {
    async fn resume(&self) -> Result<RuntimeSnapshot, ChadexError> {
        self.resume_saved_runtime_background().await
    }

    fn runtime_snapshot(&self) -> RuntimeSnapshot {
        self.snapshot()
    }

    async fn activate(&self, path: &str) -> Result<RuntimeSnapshot, ChadexError> {
        self.activate_local_project_background(path).await
    }
}

fn project_switch_busy_error() -> ErrorPayload {
    ErrorPayload::new(
        "project_switch_busy",
        "Another project operation is still running in Chadex",
        "Wait a moment, then choose the project again.",
    )
}

fn project_activation_timed_out_error() -> ErrorPayload {
    ErrorPayload::new(
        "project_activation_timed_out",
        "The local runtime did not finish opening the project in time",
        "The previous project is still active. Retry, or check the project folder and the local runtime in Diagnostics.",
    )
}

/// Waits for the project-operation lock, but never longer than `wait`: a
/// request that cannot start in time fails at once instead of running long
/// after its caller gave up.
async fn lock_project_operation<'a>(
    lock: &'a Mutex<()>,
    wait: std::time::Duration,
) -> Result<tokio::sync::MutexGuard<'a, ()>, ErrorPayload> {
    tokio::time::timeout(wait, lock.lock())
        .await
        .map_err(|_| project_switch_busy_error())
}

/// Runs a project activation with an upper bound. Past `deadline` the
/// activation is asked to cancel (`cancel`) and is still awaited, so the
/// coordinator unwinds and releases its operation slot; a cancelled or failed
/// activation then reports the timeout. An activation that completed anyway
/// keeps its result.
async fn bounded_activation<F, C>(
    activation: F,
    deadline: std::time::Duration,
    cancel: C,
) -> Result<RuntimeSnapshot, ErrorPayload>
where
    F: std::future::Future<Output = Result<RuntimeSnapshot, ChadexError>>,
    C: FnOnce(),
{
    tokio::pin!(activation);
    tokio::select! {
        result = &mut activation => result.map_err(ErrorPayload::from),
        _ = tokio::time::sleep(deadline) => {
            cancel();
            activation
                .await
                .map_err(|_| project_activation_timed_out_error())
        }
    }
}

struct PrewarmSteps {
    resume_started: (u64, Instant),
    /// When resume returned, so its trace does not include the lock wait and activation.
    resume_finished: Instant,
    resume_completion: &'static str,
    /// Start and completion of the alignment activation, when one ran.
    activation: Option<(u64, Instant, &'static str)>,
    result: Result<RuntimeSnapshot, ChadexError>,
}

impl PrewarmSteps {
    fn total_completion(&self) -> &'static str {
        match self.activation {
            Some((_, _, completion)) if self.resume_completion == "completed" => completion,
            _ => self.resume_completion,
        }
    }
}

fn step_completion<T, E>(result: &Result<T, E>, cancelled: bool) -> &'static str {
    match (result, cancelled) {
        (Ok(_), _) => "completed",
        (Err(_), true) => "cancelled",
        (Err(_), false) => "failed",
    }
}

/// Resume the saved runtime, then align it with the selected project. The
/// resume restores the last *activated* project, which can lag the selection
/// (a switch while the runtime was stopped only moves the selection), so the
/// warm-up must never finish with the runtime serving a stale project. The
/// alignment is serialised with project switches through `switch_lock`; a
/// switch cancels the warm-up before it takes that lock, so it cannot deadlock.
async fn run_prewarm_steps<P, T>(
    port: &P,
    run: &PrewarmRun<'_>,
    switch_lock: &Mutex<()>,
    target: T,
) -> PrewarmSteps
where
    P: PrewarmPort,
    T: Fn() -> Option<ProjectInspection>,
{
    let resume_started = (now_ms(), Instant::now());
    let mut result = port.resume().await;
    let resume_finished = Instant::now();
    let resume_completion = step_completion(&result, run.cancel_requested());
    let mut activation = None;
    if result.is_ok() && !run.cancel_requested() {
        let _switch = switch_lock.lock().await;
        let selected = target();
        let path = prewarm_activation_path(&port.runtime_snapshot(), selected.as_ref())
            .map(str::to_string);
        if let Some(path) = path.filter(|_| !run.cancel_requested()) {
            let activation_started_at_ms = now_ms();
            let activation_started = Instant::now();
            result = port.activate(&path).await;
            activation = Some((
                activation_started_at_ms,
                activation_started,
                step_completion(&result, run.cancel_requested()),
            ));
        }
    }
    PrewarmSteps {
        resume_started,
        resume_finished,
        resume_completion,
        activation,
        result,
    }
}

/// The project a requested realignment must activate. Only the current
/// selection may be realigned (a stale request never moves the runtime); a
/// runtime that is not ready, or already serves it, needs nothing.
fn realign_activation_path<'a>(
    runtime: &RuntimeSnapshot,
    target: Option<&'a ProjectInspection>,
    requested_path: &str,
) -> Result<Option<&'a str>, ErrorPayload> {
    let target = target.filter(|target| target.path == requested_path).ok_or_else(|| {
        ErrorPayload::new(
            "project_realign_stale",
            "The project to realign is no longer the selected project",
            "Refresh and retry with the current project.",
        )
    })?;
    Ok(prewarm_activation_path(runtime, Some(target)))
}

/// The operation the UI may present. Background warm-up is not user work: it
/// is never surfaced as an operation (and so never as Preparing / Cancel).
fn visible_operation(desktop: &RuntimeSnapshot) -> Option<OperationSnapshot> {
    desktop
        .current_operation
        .as_ref()
        .filter(|operation| !operation.background)
        .map(|operation| OperationSnapshot {
            id: operation.id.clone(),
            kind: operation.kind.to_string(),
            phase: operation.phase.as_str().to_string(),
            started_at_ms: operation.started_at_ms,
            cancellable: operation.cancellable,
        })
}

/// Tracks the single in-flight launch warm-up so other requests can join or
/// cancel it. Completion is signalled when the run guard drops, including when
/// the warm-up future is aborted.
struct PrewarmTracker {
    active: StdMutex<Option<watch::Receiver<bool>>>,
    cancel_requested: std::sync::atomic::AtomicBool,
}

struct PrewarmRun<'a> {
    tracker: &'a PrewarmTracker,
    done: watch::Sender<bool>,
}

impl Drop for PrewarmRun<'_> {
    fn drop(&mut self) {
        *self
            .tracker
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.done.send_replace(true);
    }
}

impl PrewarmRun<'_> {
    fn cancel_requested(&self) -> bool {
        self.tracker
            .cancel_requested
            .load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl PrewarmTracker {
    fn new() -> Self {
        Self {
            active: StdMutex::new(None),
            cancel_requested: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn begin(&self) -> Option<PrewarmRun<'_>> {
        let mut active = self
            .active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if active.is_some() {
            return None;
        }
        let (done, receiver) = watch::channel(false);
        *active = Some(receiver);
        self.cancel_requested
            .store(false, std::sync::atomic::Ordering::SeqCst);
        Some(PrewarmRun {
            tracker: self,
            done,
        })
    }

    fn receiver(&self) -> Option<watch::Receiver<bool>> {
        self.active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Waits for the in-flight warm-up (if any) to finish.
    async fn join(&self) {
        if let Some(mut receiver) = self.receiver() {
            let _ = receiver.wait_for(|done| *done).await;
        }
    }

    /// Asks the warm-up to stop and waits (bounded) until it has unwound.
    /// `cancel_operation` is retried because the warm-up may not have admitted
    /// its runtime operation yet when the first attempt lands.
    async fn cancel<F, Fut>(&self, timeout: std::time::Duration, retry: std::time::Duration, mut cancel_operation: F)
    where
        F: FnMut() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        let Some(mut receiver) = self.receiver() else {
            return;
        };
        self.cancel_requested
            .store(true, std::sync::atomic::Ordering::SeqCst);
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // The coordinator's own wait may exceed our remaining budget;
            // dropping it only stops waiting, the cancellation stays requested.
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let _ = tokio::time::timeout(remaining, cancel_operation()).await;
            let wait = retry.min(deadline.saturating_duration_since(tokio::time::Instant::now()));
            if tokio::time::timeout(wait, receiver.wait_for(|done| *done))
                .await
                .is_ok()
            {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                return;
            }
        }
    }
}

struct Bridge {
    runtime: ChadexRuntimeCore,
    graphify: GraphifyStatus,
    tunnel: Arc<TunnelManager>,
    /// Computer Use cursor overlay: runner events in, `computer_overlay` event frames out.
    overlay: Arc<ComputerOverlayHub>,
    overlay_events: StdMutex<Option<mpsc::Receiver<Value>>>,
    verification: Arc<VerificationTracker>,
    performance: Arc<PerformanceTraceStore>,
    target_project: RwLock<Option<ProjectInspection>>,
    task_state_dir: PathBuf,
    snapshot_state: StdMutex<SnapshotRevisionState>,
    project_switch_lifecycle: Mutex<()>,
    mascot_jobs_refresh: Mutex<()>,
    prewarm: PrewarmTracker,
    started_at: Instant,
}

impl Bridge {
    fn new() -> Result<Self, ErrorPayload> {
        let data_dir = chadex_data_dir()?;
        let resource_dir = chadex_resource_dir()?;
        Self::with_dirs(data_dir, resource_dir)
    }

    fn with_dirs(data_dir: PathBuf, resource_dir: PathBuf) -> Result<Self, ErrorPayload> {
        fs::create_dir_all(&data_dir).map_err(|error| {
            ErrorPayload::new(
                "data_directory_unavailable",
                "Chadex could not prepare its private data directory",
                "Check Application Support permissions and retry.",
            )
            .with_details(json!({ "io_kind": format!("{:?}", error.kind()) }))
        })?;
        let task_state_dir = data_dir.join("phase9-tasks");
        let runtime =
            ChadexRuntimeCore::new(data_dir.clone(), resource_dir).map_err(ErrorPayload::from)?;
        let graphify = GraphifyStatus::detect();
        let verification = Arc::new(VerificationTracker::default());
        let performance = Arc::new(PerformanceTraceStore::default());
        let (overlay, overlay_events) = ComputerOverlayHub::new();
        let tunnel = Arc::new(
            TunnelManager::new(
                data_dir,
                Arc::clone(&verification),
                Arc::clone(&performance),
            )
            .with_computer_overlay(Arc::clone(&overlay)),
        );
        Ok(Self {
            runtime,
            graphify,
            tunnel,
            overlay,
            overlay_events: StdMutex::new(Some(overlay_events)),
            verification,
            performance,
            target_project: RwLock::new(None),
            task_state_dir,
            snapshot_state: StdMutex::new(SnapshotRevisionState::default()),
            project_switch_lifecycle: Mutex::new(()),
            mascot_jobs_refresh: Mutex::new(()),
            prewarm: PrewarmTracker::new(),
            started_at: Instant::now(),
        })
    }

    /// The stdout event writer owns the receiving end; it can be taken once.
    fn take_overlay_events(&self) -> Option<mpsc::Receiver<Value>> {
        self.overlay_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }

    fn target_project(&self) -> Option<ProjectInspection> {
        self.target_project
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_target_project(&self, project: ProjectInspection) {
        // Preserve only startup-recovered task progress on the first project
        // selection. Ordinary completed/stale latest.json state should not leak
        // into a newly selected project. Switching projects always clears it.
        let current = self.target_project();
        let should_clear_progress = match current.as_ref() {
            Some(current) => current.path != project.path,
            None => !self.task_progress_recovery_matches_path(&project.path),
        };
        if should_clear_progress {
            self.clear_latest_task_progress();
        }
        self.verification.reset(Some(project.path.clone()));
        *self
            .target_project
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(project);
    }

    fn reset_verification_for_target(&self) {
        self.verification
            .reset(self.target_project().map(|project| project.path));
    }

    fn clear_latest_task_progress(&self) {
        let _ = fs::remove_file(self.task_state_dir.join("state").join("latest.json"));
    }

    fn task_progress_bytes(&self) -> Option<Vec<u8>> {
        let path = self.task_state_dir.join("state").join("latest.json");
        let metadata = fs::symlink_metadata(&path).ok()?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() == 0
            || metadata.len() > TASK_PROGRESS_MAX_BYTES
        {
            return None;
        }
        fs::read(path).ok()
    }

    fn task_progress_value(&self) -> Option<Value> {
        let bytes = self.task_progress_bytes()?;
        serde_json::from_slice(&bytes).ok()
    }

    fn task_progress_is_presentable(value: &Value) -> bool {
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let workspace_state = value
            .pointer("/workspace/state")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let recovery_available = value
            .pointer("/recovery/available")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let active = matches!(
            status,
            "queued"
                | "preparing"
                | "running"
                | "integrating"
                | "validating"
                | "ready_to_apply"
                | "cancelling"
        );
        let recoverable =
            recovery_available && (status == "interrupted" || workspace_state == "preserved");
        let recent_terminal = matches!(status, "completed" | "failed" | "failed_validation" | "cancelled")
            && value.get("finished_at_ms").and_then(Value::as_u64)
                .map(|finished| finished <= now_ms() && now_ms().saturating_sub(finished) < 8_000)
                .unwrap_or(false);
        active || recoverable || recent_terminal
    }

    fn task_progress_matches_path(value: &Value, selected_path: &str) -> bool {
        value.get("source_path").and_then(Value::as_str) == Some(selected_path)
    }

    fn task_progress_recovery_matches_path(&self, selected_path: &str) -> bool {
        let Some(value) = self.task_progress_value() else {
            return false;
        };
        Self::task_progress_is_presentable(&value)
            && value
                .pointer("/recovery/available")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            && Self::task_progress_matches_path(&value, selected_path)
    }

    fn task_progress(&self) -> Option<TaskProgressSnapshot> {
        let value = self.task_progress_value()?;
        let selected_path = self.target_project()?.path;
        (Self::task_progress_is_presentable(&value)
            && Self::task_progress_matches_path(&value, &selected_path))
        .then(|| serde_json::from_value(value).ok())
        .flatten()
    }

    async fn inspect_project(&self, path: &str) -> Result<ProjectInspection, ErrorPayload> {
        let project = self
            .runtime
            .inspect_project(path)
            .await
            .map_err(ErrorPayload::from)?;
        Ok(project_inspection(project))
    }

    async fn skill_catalog(&self, path: &str) -> Result<Value, ErrorPayload> {
        self.runtime
            .skill_catalog(path)
            .await
            .map_err(ErrorPayload::from)
    }

    async fn discover_external_skill_sources(&self) -> Result<Value, ErrorPayload> {
        let started_at_ms = now_ms();
        let started = Instant::now();
        let result = self.discover_external_skill_sources_inner().await;
        let completion = if result.is_ok() { "completed" } else { "error" };
        self.push_lifecycle("skill_discovery", "total", started_at_ms, started, completion);
        result
    }

    async fn discover_external_skill_sources_inner(&self) -> Result<Value, ErrorPayload> {
        let env = DiscoveryEnv::from_process().ok_or_else(|| {
            ErrorPayload::new(
                "home_directory_unavailable",
                "Chadex could not determine the home directory to look for Skill folders",
                "Check that HOME is set for Chadex and retry.",
            )
        })?;
        let discovery = tokio::task::spawn_blocking(move || discover_external_skill_sources(&env))
            .await
            .map_err(|_| {
                ErrorPayload::new(
                    "external_skill_discovery_failed",
                    "Chadex could not finish looking for external Skill folders",
                    "Retry. If this keeps happening, restart Chadex.",
                )
            })?;
        serde_json::to_value(discovery).map_err(|_| {
            ErrorPayload::new(
                "external_skill_discovery_failed",
                "Chadex could not encode the external Skill folder results",
                "Retry. If this keeps happening, restart Chadex.",
            )
        })
    }

    async fn skill_inventory(&self, path: &str) -> Result<Value, ErrorPayload> {
        self.runtime
            .skill_inventory(path)
            .await
            .map_err(ErrorPayload::from)
    }

    async fn skill_definition(
        &self,
        path: &str,
        skill_id: &str,
        definition_revision: &str,
        package_revision: Option<&str>,
    ) -> Result<Value, ErrorPayload> {
        self.runtime
            .skill_definition(path, skill_id, definition_revision, package_revision)
            .await
            .map_err(ErrorPayload::from)
    }

    async fn create_project_skill(
        &self,
        path: &str,
        skill_key: &str,
        content: &str,
    ) -> Result<Value, ErrorPayload> {
        self.runtime
            .create_project_skill(path, skill_key, content)
            .await
            .map_err(ErrorPayload::from)
    }

    async fn install_skill(
        &self,
        path: &str,
        skill_key: &str,
        artifact_path: &str,
    ) -> Result<Value, ErrorPayload> {
        self.runtime
            .install_skill(path, skill_key, artifact_path)
            .await
            .map_err(ErrorPayload::from)
    }

    async fn activate_skill(
        &self,
        path: &str,
        skill_key: &str,
        package_revision: &str,
        state_revision: &str,
    ) -> Result<Value, ErrorPayload> {
        self.runtime
            .activate_skill(path, skill_key, package_revision, state_revision)
            .await
            .map_err(ErrorPayload::from)
    }

    async fn deactivate_skill(
        &self,
        path: &str,
        skill_key: &str,
        state_revision: &str,
    ) -> Result<Value, ErrorPayload> {
        self.runtime
            .deactivate_skill(path, skill_key, state_revision)
            .await
            .map_err(ErrorPayload::from)
    }

    async fn remove_skill(
        &self,
        path: &str,
        skill_key: &str,
        state_revision: &str,
    ) -> Result<Value, ErrorPayload> {
        self.runtime
            .remove_skill(path, skill_key, state_revision)
            .await
            .map_err(ErrorPayload::from)
    }

    async fn memory_catalog(&self, path: &str) -> Result<Value, ErrorPayload> {
        self.runtime
            .memory_catalog(path)
            .await
            .map_err(ErrorPayload::from)
    }

    async fn memory_read(
        &self,
        path: &str,
        memory_key: &str,
        expected_revision: Option<&str>,
    ) -> Result<Value, ErrorPayload> {
        self.runtime
            .memory_read(path, memory_key, expected_revision)
            .await
            .map_err(ErrorPayload::from)
    }

    async fn memory_set(
        &self,
        path: &str,
        memory_key: &str,
        summary: &str,
        body: &str,
        priority: &str,
        bootstrap: bool,
        tags: &[String],
        expected_revision: Option<&str>,
    ) -> Result<Value, ErrorPayload> {
        self.runtime
            .memory_set(
                path,
                memory_key,
                summary,
                body,
                priority,
                bootstrap,
                tags,
                expected_revision,
            )
            .await
            .map_err(ErrorPayload::from)
    }

    async fn memory_delete(
        &self,
        path: &str,
        memory_key: &str,
        expected_revision: &str,
    ) -> Result<Value, ErrorPayload> {
        self.runtime
            .memory_delete(path, memory_key, expected_revision)
            .await
            .map_err(ErrorPayload::from)
    }

    async fn choose_target_project(&self, path: &str) -> Result<BackendSnapshot, ErrorPayload> {
        let project = self.inspect_project(path).await?;
        self.set_target_project(project);
        Ok(self.snapshot())
    }

    async fn switch_local_project(&self, path: &str) -> Result<BackendSnapshot, ErrorPayload> {
        // Project switching mutates runtime routing and verification state. Keep the
        // single ChatGPT tunnel alive whenever its local target remains unchanged.
        // The ingress is paused so no new request can enter while the active project
        // is transitioning; in-flight requests keep their old verification epoch.
        let _switch =
            lock_project_operation(&self.project_switch_lifecycle, PROJECT_SWITCH_LOCK_WAIT)
                .await?;
        let previous_target = self.target_project();
        let before = self.runtime.snapshot();
        let tunnel_state = self.tunnel.snapshot().state;
        let restore_connection = should_restore_connection_after_project_switch(tunnel_state);
        let ingress_paused = if tunnel_state == TunnelState::Ready {
            self.tunnel
                .pause_ingress()
                .await
                .map_err(ErrorPayload::from)?
        } else if tunnel_state == TunnelState::Starting {
            // A starting tunnel has not published its ingress yet. Cancel that
            // startup and restore the user's connection intent after activation.
            self.tunnel.stop().await.map_err(ErrorPayload::from)?;
            false
        } else {
            false
        };
        let previous_tunnel_target = if restore_connection {
            self.runtime.tunnel_target().await.ok()
        } else {
            None
        };

        let switch_result: Result<ProjectInspection, ErrorPayload> = async {
            if before.readiness.runtime_ready && before.project.is_some() {
                let activated = self.bounded_local_activation(path).await?;
                let activated_project = activated.project.ok_or_else(|| {
                    ErrorPayload::new(
                        "project_activation_incomplete",
                        "The activated project was not returned by the local runtime",
                        "Retry the project switch. If the problem continues, restart Chadex.",
                    )
                })?;
                Ok(project_inspection(activated_project))
            } else {
                self.inspect_project(path).await
            }
        }
        .await;

        let project = match switch_result {
            Ok(project) => project,
            Err(error) => {
                let restored = self
                    .restore_previous_project_after_failed_switch(previous_target, &before)
                    .await;
                if restore_connection {
                    if restored {
                        let _ = self
                            .restore_tunnel_after_project_switch(
                                previous_tunnel_target.as_ref(),
                                ingress_paused,
                            )
                            .await;
                    } else {
                        let _ = self.tunnel.stop().await;
                    }
                }
                return Err(error);
            }
        };

        self.set_target_project(project);
        if restore_connection {
            if let Err(error) = self
                .restore_tunnel_after_project_switch(
                    previous_tunnel_target.as_ref(),
                    ingress_paused,
                )
                .await
            {
                let restored = self
                    .restore_previous_project_after_failed_switch(previous_target, &before)
                    .await;
                if restored {
                    let _ = self
                        .restore_tunnel_after_project_switch(previous_tunnel_target.as_ref(), false)
                        .await;
                } else {
                    let _ = self.tunnel.stop().await;
                }
                return Err(error);
            }
        }
        Ok(self.snapshot())
    }

    async fn restore_tunnel_after_project_switch(
        &self,
        previous_tunnel_target: Option<&RuntimeTunnelTarget>,
        ingress_paused: bool,
    ) -> Result<(), ErrorPayload> {
        let current_tunnel = self.tunnel.refresh().await;
        let current_target = match self.runtime.tunnel_target().await {
            Ok(target) => target,
            Err(error) => {
                // A paused ingress must never remain published as Ready when
                // the runtime endpoint can no longer be resolved.
                let _ = self.tunnel.stop().await;
                return Err(ErrorPayload::from(error));
            }
        };
        if ingress_paused
            && current_tunnel.state == TunnelState::Ready
            && previous_tunnel_target == Some(&current_target)
            && self.tunnel.resume_ingress().await
        {
            return Ok(());
        }

        self.tunnel.stop().await.map_err(ErrorPayload::from)?;
        self.start_tunnel().await.map(|_| ())
    }

    async fn restore_previous_project_after_failed_switch(
        &self,
        previous_target: Option<ProjectInspection>,
        previous_runtime: &RuntimeSnapshot,
    ) -> bool {
        let Some(previous_target) = previous_target else {
            return false;
        };

        let previous_runtime_matches_target = previous_runtime
            .project
            .as_ref()
            .is_some_and(|project| project.path == previous_target.path);
        let mut runtime_matches_previous = self
            .runtime
            .snapshot()
            .project
            .as_ref()
            .is_some_and(|project| project.path == previous_target.path);

        if previous_runtime.readiness.runtime_ready
            && previous_runtime_matches_target
            && !runtime_matches_previous
        {
            runtime_matches_previous = self
                .runtime
                .activate_local_project(&previous_target.path)
                .await
                .ok()
                .and_then(|snapshot| snapshot.project)
                .is_some_and(|project| project.path == previous_target.path);
        }

        if runtime_matches_previous {
            let selected_matches_previous = self
                .target_project()
                .as_ref()
                .is_some_and(|project| project.path == previous_target.path);
            if !selected_matches_previous {
                self.set_target_project(previous_target);
            }
        }
        runtime_matches_previous
    }

    /// A user-visible project activation, bounded by
    /// `PROJECT_ACTIVATION_DEADLINE` (see `bounded_activation`).
    async fn bounded_local_activation(&self, path: &str) -> Result<RuntimeSnapshot, ErrorPayload> {
        bounded_activation(
            self.runtime.activate_local_project(path),
            PROJECT_ACTIVATION_DEADLINE,
            || {
                if let Some(operation) = self.runtime.snapshot().current_operation.filter(|operation| {
                    operation.kind == "local_project_activate" && !operation.background
                }) {
                    let _ = self.runtime.cancel_operation(&operation.id);
                }
            },
        )
        .await
    }

    /// Re-activates the selected project in a ready runtime that serves
    /// another one. Unlike a project switch it never touches the tunnel or the
    /// selection: on failure everything stays as it was.
    async fn realign_local_project(&self, path: &str) -> Result<BackendSnapshot, ErrorPayload> {
        let _switch =
            lock_project_operation(&self.project_switch_lifecycle, PROJECT_SWITCH_LOCK_WAIT)
                .await?;
        let target = self.target_project();
        let current = self.runtime.snapshot();
        let Some(path) = realign_activation_path(&current, target.as_ref(), path)? else {
            return Ok(self.snapshot_from(current));
        };
        let started_at_ms = now_ms();
        let started = Instant::now();
        let result = self.bounded_local_activation(path).await;
        self.push_lifecycle(
            "realign",
            "project_activation",
            started_at_ms,
            started,
            step_completion(&result, false),
        );
        let activated = result?;
        // Any verification observed while the runtime served another project
        // does not count for the selection.
        self.reset_verification_for_target();
        Ok(self.snapshot_from(activated))
    }

    async fn ensure_runtime_for_target(&self) -> Result<BackendSnapshot, ErrorPayload> {
        let target = self.target_project().ok_or_else(|| {
            ErrorPayload::new(
                "project_not_selected",
                "No Chadex project is selected",
                "Choose a project before connecting ChatGPT.",
            )
        })?;

        let before = self.runtime.snapshot();
        let mut current = if before.readiness.runtime_ready {
            before
        } else if !before.runtime_configured || before.project.is_none() {
            self.runtime
                .configure_local_setup(Some(&target.path))
                .await
                .map_err(ErrorPayload::from)?
        } else {
            self.runtime
                .resume_saved_runtime()
                .await
                .map_err(ErrorPayload::from)?
        };

        if current
            .project
            .as_ref()
            .map(|project| project.path.as_str())
            != Some(target.path.as_str())
        {
            let started_at_ms = now_ms();
            let started = Instant::now();
            let activation = self.runtime.activate_local_project(&target.path).await;
            self.performance.push_lifecycle(LifecyclePerformanceTrace {
                sequence: 0,
                started_at_ms,
                operation: "connect".to_string(),
                phase: "project_activation".to_string(),
                total_us: duration_us(started.elapsed()),
                completion: if activation.is_ok() {
                    "completed"
                } else {
                    "error"
                }
                .to_string(),
            });
            current = activation.map_err(ErrorPayload::from)?;
        }
        Ok(self.snapshot_from(current))
    }

    // Launch-time warm-up: resume only a runtime the user already set up and
    // did not explicitly stop, so a later Connect only has to start the tunnel.
    // The resume runs as a background operation: it is never published as the
    // current user operation, so the UI keeps offering Connect while it runs.
    async fn prewarm_runtime(&self) -> Result<BackendSnapshot, ErrorPayload> {
        let started_at_ms = now_ms();
        let started = Instant::now();
        let current = self.runtime.snapshot();
        let decision = prewarm_decision(&current, self.target_project().is_some());
        if decision != PrewarmDecision::Resume {
            self.record_prewarm(started_at_ms, started, None, decision.skip_completion());
            return Ok(self.snapshot_from(current));
        }
        let Some(run) = self.prewarm.begin() else {
            self.record_prewarm(started_at_ms, started, None, "skipped:already_running");
            return Ok(self.snapshot_from(current));
        };
        if run.cancel_requested() {
            self.record_prewarm(started_at_ms, started, None, "cancelled");
            return Ok(self.snapshot_from(current));
        }
        let steps = run_prewarm_steps(&self.runtime, &run, &self.project_switch_lifecycle, || {
            self.target_project()
        })
        .await;
        drop(run);
        self.record_prewarm_steps(started_at_ms, started, &steps);
        steps
            .result
            .map(|snapshot| self.snapshot_from(snapshot))
            .map_err(ErrorPayload::from)
    }

    /// Records a warm-up that ran: the resume and the launch-to-ready benchmark
    /// report the resume itself; an alignment activation is reported on its own
    /// and only the overall total reflects it. Fixed strings and durations only.
    fn record_prewarm_steps(&self, started_at_ms: u64, started: Instant, steps: &PrewarmSteps) {
        let (resume_started_at_ms, resume_started) = steps.resume_started;
        self.push_lifecycle_until(
            "prewarm",
            "runtime_resume",
            resume_started_at_ms,
            resume_started,
            steps.resume_finished,
            steps.resume_completion,
        );
        if let Some((activation_started_at_ms, activation_started, completion)) = steps.activation {
            self.push_lifecycle(
                "prewarm",
                "project_activation",
                activation_started_at_ms,
                activation_started,
                completion,
            );
        }
        self.push_lifecycle("prewarm", "total", started_at_ms, started, steps.total_completion());
        let launch_started_at_ms = now_ms().saturating_sub(duration_us(self.started_at.elapsed()) / 1000);
        self.push_lifecycle_until(
            "launch",
            "helper_start_to_runtime_ready",
            launch_started_at_ms,
            self.started_at,
            steps.resume_finished,
            steps.resume_completion,
        );
    }

    /// Records the prewarm phases plus the launch-to-ready benchmark. Carries
    /// only fixed strings and durations: no paths, tokens or project names.
    fn record_prewarm(
        &self,
        started_at_ms: u64,
        started: Instant,
        resume: Option<(u64, Instant)>,
        completion: &str,
    ) {
        if let Some((resume_started_at_ms, resume_started)) = resume {
            self.push_lifecycle("prewarm", "runtime_resume", resume_started_at_ms, resume_started, completion);
        }
        self.push_lifecycle("prewarm", "total", started_at_ms, started, completion);
        let launch_started_at_ms = now_ms().saturating_sub(duration_us(self.started_at.elapsed()) / 1000);
        self.push_lifecycle("launch", "helper_start_to_runtime_ready", launch_started_at_ms, self.started_at, completion);
    }

    fn push_lifecycle(&self, operation: &str, phase: &str, started_at_ms: u64, started: Instant, completion: &str) {
        self.push_lifecycle_until(operation, phase, started_at_ms, started, Instant::now(), completion);
    }

    /// Like `push_lifecycle`, for a phase that ended at `finished` rather than now.
    fn push_lifecycle_until(
        &self,
        operation: &str,
        phase: &str,
        started_at_ms: u64,
        started: Instant,
        finished: Instant,
        completion: &str,
    ) {
        self.performance.push_lifecycle(LifecyclePerformanceTrace {
            sequence: 0,
            started_at_ms,
            operation: operation.to_string(),
            phase: phase.to_string(),
            total_us: duration_us(finished.saturating_duration_since(started)),
            completion: completion.to_string(),
        });
    }

    /// Waits for an in-flight warm-up; returns how long it waited when there was one.
    async fn join_prewarm(&self) -> Option<(u64, Instant)> {
        let started_at_ms = now_ms();
        let started = Instant::now();
        self.prewarm.receiver()?;
        self.prewarm.join().await;
        Some((started_at_ms, started))
    }

    async fn cancel_prewarm(&self) {
        self.prewarm
            .cancel(PREWARM_CANCEL_TIMEOUT, PREWARM_CANCEL_RETRY, || {
                self.runtime.cancel_background_operation()
            })
            .await;
    }

    async fn refreshed_snapshot(&self, include_mascot_jobs: bool) -> BackendSnapshot {
        self.tunnel.refresh().await;
        let jobs = if include_mascot_jobs { self.observe_mascot_jobs().await } else { None };
        self.snapshot_from_with_mascot_jobs(self.runtime.snapshot(), jobs)
    }

    // Only getStatus calls this observer. It neither changes readiness nor
    // publishes errors/activities, and concurrent polls do not queue more work.
    async fn observe_mascot_jobs(&self) -> Option<ScopedMascotJobs> {
        let Ok(_guard) = self.mascot_jobs_refresh.try_lock() else { return None; };
        let desktop = self.runtime.snapshot();
        if !desktop.readiness.runtime_ready || desktop.current_operation.is_some() { return None; }
        let target = self.target_project()?;
        let epoch = self.verification.snapshot().epoch;
        let jobs = tokio::time::timeout(
            MASCOT_JOBS_OBSERVATION_TIMEOUT, self.runtime.observe_mascot_jobs(&target.path),
        ).await.ok().flatten();
        if self.target_project().as_ref().map(|p| &p.path) != Some(&target.path)
            || self.verification.snapshot().epoch != epoch
        {
            return None;
        }
        let current = self.runtime.snapshot();
        if !current.readiness.runtime_ready || current.current_operation.is_some() { return None; }
        Some(ScopedMascotJobs { project_path: target.path, epoch, jobs: jobs? })
    }

    async fn refresh_runtime(&self) -> Result<BackendSnapshot, ErrorPayload> {
        let desktop = self
            .runtime
            .refresh_runtime_status()
            .await
            .map_err(ErrorPayload::from)?;
        self.tunnel.refresh().await;
        Ok(self.snapshot_from(desktop))
    }

    async fn observe_chatgpt_activity(&self) -> Result<BackendSnapshot, ErrorPayload> {
        self.tunnel.refresh().await;
        Ok(self.snapshot())
    }

    async fn start_tunnel(&self) -> Result<BackendSnapshot, ErrorPayload> {
        let target = self
            .runtime
            .tunnel_target()
            .await
            .map_err(ErrorPayload::from)?;
        let proxy = self.runtime.snapshot().tunnel_proxy_effective_url;
        self.reset_verification_for_target();
        self.tunnel
            .start(target, proxy.as_deref())
            .await
            .map_err(ErrorPayload::from)?;
        Ok(self.snapshot())
    }

    async fn connect_chatgpt(&self) -> Result<BackendSnapshot, ErrorPayload> {
        let connect_started_at_ms = now_ms();
        let connect_started = Instant::now();

        let runtime_started_at_ms = now_ms();
        let runtime_started = Instant::now();
        let runtime_result = self.ensure_runtime_for_target().await;
        self.performance.push_lifecycle(LifecyclePerformanceTrace {
            sequence: 0,
            started_at_ms: runtime_started_at_ms,
            operation: "connect".to_string(),
            phase: "runtime_ensure".to_string(),
            total_us: duration_us(runtime_started.elapsed()),
            completion: if runtime_result.is_ok() {
                "completed"
            } else {
                "error"
            }
            .to_string(),
        });
        if let Err(error) = runtime_result {
            self.performance.push_lifecycle(LifecyclePerformanceTrace {
                sequence: 0,
                started_at_ms: connect_started_at_ms,
                operation: "connect".to_string(),
                phase: "total".to_string(),
                total_us: duration_us(connect_started.elapsed()),
                completion: "error".to_string(),
            });
            return Err(error);
        }

        let tunnel_started_at_ms = now_ms();
        let tunnel_started = Instant::now();
        let tunnel_result = self.start_tunnel().await;
        self.performance.push_lifecycle(LifecyclePerformanceTrace {
            sequence: 0,
            started_at_ms: tunnel_started_at_ms,
            operation: "connect".to_string(),
            phase: "tunnel_start".to_string(),
            total_us: duration_us(tunnel_started.elapsed()),
            completion: if tunnel_result.is_ok() {
                "completed"
            } else {
                "error"
            }
            .to_string(),
        });
        self.performance.push_lifecycle(LifecyclePerformanceTrace {
            sequence: 0,
            started_at_ms: connect_started_at_ms,
            operation: "connect".to_string(),
            phase: "total".to_string(),
            total_us: duration_us(connect_started.elapsed()),
            completion: if tunnel_result.is_ok() {
                "completed"
            } else {
                "error"
            }
            .to_string(),
        });
        tunnel_result
    }

    async fn stop_tunnel(&self) -> Result<BackendSnapshot, ErrorPayload> {
        self.tunnel.stop().await.map_err(ErrorPayload::from)?;
        self.reset_verification_for_target();
        Ok(self.snapshot())
    }

    async fn stop_local_service(&self) -> Result<BackendSnapshot, ErrorPayload> {
        self.tunnel.stop().await.map_err(ErrorPayload::from)?;
        self.reset_verification_for_target();
        let desktop = self
            .runtime
            .stop_local_runtime()
            .await
            .map_err(ErrorPayload::from)?;
        Ok(self.snapshot_from(desktop))
    }

    async fn provide_credentials(
        &self,
        tunnel_id: &str,
        api_key: Zeroizing<String>,
    ) -> Result<BackendSnapshot, ErrorPayload> {
        self.tunnel
            .provide_credentials(tunnel_id, api_key)
            .await
            .map_err(ErrorPayload::from)?;
        self.reset_verification_for_target();
        Ok(self.snapshot())
    }

    async fn clear_credentials(&self) -> Result<BackendSnapshot, ErrorPayload> {
        self.tunnel
            .clear_credentials()
            .await
            .map_err(ErrorPayload::from)?;
        self.reset_verification_for_target();
        Ok(self.snapshot())
    }

    async fn cancel_task(
        &self,
        project: &str,
        task_id: &str,
    ) -> Result<BackendSnapshot, ErrorPayload> {
        self.runtime
            .cancel_chadex_task(project, task_id)
            .await
            .map_err(ErrorPayload::from)?;
        Ok(self.snapshot())
    }

    fn snapshot(&self) -> BackendSnapshot {
        self.snapshot_from(self.runtime.snapshot())
    }

    fn snapshot_from(&self, desktop: RuntimeSnapshot) -> BackendSnapshot {
        self.snapshot_from_with_mascot_jobs(desktop, None)
    }

    fn snapshot_from_with_mascot_jobs(&self, desktop: RuntimeSnapshot, mascot_jobs: Option<ScopedMascotJobs>) -> BackendSnapshot {
        let tunnel = self.tunnel.snapshot();
        let verification = self.verification.snapshot();
        let target = self
            .target_project()
            .or_else(|| desktop.project.clone().map(project_inspection));
        let target_matches_runtime = match (&target, &desktop.project) {
            (Some(target), Some(runtime)) => target.path == runtime.path,
            _ => false,
        };
        let mascot_jobs = mascot_jobs
            .filter(|_| target_matches_runtime && desktop.readiness.runtime_ready && desktop.current_operation.is_none())
            .and_then(|jobs| jobs.for_target(target.as_ref().map(|p| p.path.as_str()), verification.epoch));
        let tunnel_ready = tunnel.state == TunnelState::Ready;
        let verification_matches_target = match (&target, &verification.project_path) {
            (Some(target), Some(verified_path)) => target.path == *verified_path,
            _ => false,
        };
        let chat_gpt_connected = tunnel_ready
            && target_matches_runtime
            && verification_matches_target
            && verification.chatgpt_connected;
        let verified = chat_gpt_connected && verification.verified;
        let current_operation = visible_operation(&desktop)
            .or_else(|| {
                (tunnel.state == TunnelState::Starting).then(|| OperationSnapshot {
                    id: format!("chadex-tunnel-{}", tunnel.epoch),
                    kind: "regular_tunnel_start".to_string(),
                    phase: "running".to_string(),
                    started_at_ms: 0,
                    cancellable: false,
                })
            });
        let runtime_error = desktop.readiness.needs_attention;
        let tunnel_error = tunnel.state == TunnelState::Error;
        let phase = if current_operation.is_some() || tunnel.state == TunnelState::Starting {
            ConnectionPhase::Preparing
        } else if target.is_none() || !tunnel.configured {
            ConnectionPhase::Unconfigured
        } else if verified {
            ConnectionPhase::Verified
        } else if runtime_error || tunnel_error {
            ConnectionPhase::Error
        } else if tunnel_ready {
            ConnectionPhase::WaitingForChatGptVerification
        } else {
            ConnectionPhase::Stopped
        };
        let error = if tunnel_error {
            Some(ErrorPayload {
                code: "tunnel_unavailable".to_string(),
                message: tunnel
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "OpenAI Secure MCP Tunnel needs attention".to_string()),
                recovery: Some(
                    "Check the Tunnel ID, restricted API key, network access, and local MCP runtime, then retry."
                        .to_string(),
                ),
                details: Some(json!({ "epoch": tunnel.epoch })),
            })
        } else if phase == ConnectionPhase::Error {
            Some(ErrorPayload {
                code: "runtime_needs_attention".to_string(),
                message: desktop.readiness.summary.clone(),
                recovery: desktop.readiness.next_action.clone(),
                details: Some(json!({
                    "summary_kind": desktop.readiness.summary_kind,
                    "server": desktop.readiness.server,
                    "runner": desktop.readiness.runner,
                    "exposure": desktop.readiness.exposure,
                    "project": desktop.readiness.project
                })),
            })
        } else {
            None
        };
        let last_verified_at_ms = if verified {
            verification.verified_at_ms
        } else {
            None
        };

        let candidate = BackendSnapshot {
            phase,
            graphify: self.graphify.clone(),
            selected_project: target,
            tunnel_ready,
            chat_gpt_connected,
            chat_gpt_verified_for_selected_project: verified,
            last_verified_at_ms,
            current_operation,
            task_progress: self.task_progress(),
            mascot_jobs,
            runtime_status: Some(RuntimeStatus::from_snapshot(&desktop)),
            tunnel_status: Some(TunnelStatus::from_snapshot(&tunnel)),
            error,
            activity_sequence: desktop.activity_sequence,
            state_revision: 0,
        };
        self.revisioned_snapshot(candidate)
    }

    fn revisioned_snapshot(&self, candidate: BackendSnapshot) -> BackendSnapshot {
        let mut state = self
            .snapshot_state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.assign_revision(candidate)
    }
}

pub fn run() {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!(
                "helper runtime error: {}",
                sanitize_message(&error.to_string())
            );
            return;
        }
    };
    runtime.block_on(async {
        if let Err(error) = run_async().await {
            eprintln!("helper fatal error: {}", sanitize_message(&error));
        }
    });
}

fn log_request_task_result(result: Result<(), tokio::task::JoinError>) {
    if let Err(error) = result {
        if !error.is_cancelled() {
            eprintln!(
                "helper request task error: {}",
                sanitize_message(&error.to_string())
            );
        }
    }
}

fn reap_finished_request_tasks(tasks: &mut JoinSet<()>) {
    while let Some(result) = tasks.try_join_next() {
        log_request_task_result(result);
    }
}

async fn abort_and_drain_request_tasks(tasks: &mut JoinSet<()>) {
    tasks.abort_all();
    let drain = async {
        while let Some(result) = tasks.join_next().await {
            log_request_task_result(result);
        }
    };
    if tokio::time::timeout(SHUTDOWN_REQUEST_DRAIN_TIMEOUT, drain)
        .await
        .is_err()
    {
        eprintln!("helper request task drain timed out during shutdown");
    }
}

async fn read_bounded_ndjson_line<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    max_bytes: usize,
) -> std::io::Result<Option<String>> {
    let mut line = Vec::with_capacity(max_bytes.min(4096));
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            return String::from_utf8(line).map(Some).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "helper request is not UTF-8",
                )
            });
        }
        if let Some(newline) = available.iter().position(|byte| *byte == b'\n') {
            if line.len().saturating_add(newline) > max_bytes {
                reader.consume(newline + 1);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "helper request frame exceeds maximum size",
                ));
            }
            line.extend_from_slice(&available[..newline]);
            reader.consume(newline + 1);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return String::from_utf8(line).map(Some).map_err(|_| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "helper request is not UTF-8",
                )
            });
        }
        let chunk_len = available.len();
        if line.len().saturating_add(chunk_len) > max_bytes {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "helper request frame exceeds maximum size",
            ));
        }
        line.extend_from_slice(available);
        reader.consume(chunk_len);
    }
}

async fn run_async() -> Result<(), String> {
    let bridge = Arc::new(Bridge::new().map_err(|error| error.message)?);
    // Runners spawned from here on get the overlay channel (macOS); the hub
    // stays silent until the App calls `setComputerOverlayEvents`.
    computer_overlay::install_shared_hub(Arc::clone(&bridge.overlay));
    let stdout = Arc::new(Mutex::new(tokio::io::stdout()));
    let overlay_writer = bridge
        .take_overlay_events()
        .map(|events| tokio::spawn(write_overlay_events(Arc::clone(&stdout), events)));
    let mut stdin = BufReader::new(tokio::io::stdin());
    let mut tasks = JoinSet::new();

    loop {
        // JoinSet retains completed task outputs until they are joined. Reap on
        // every turn and also race stdin against completion so a quiet client
        // cannot leave thousands of completed request tasks resident forever.
        reap_finished_request_tasks(&mut tasks);
        if tasks.len() >= MAX_IN_FLIGHT_REQUESTS {
            if let Some(result) = tasks.join_next().await {
                log_request_task_result(result);
            }
            continue;
        }

        let next_line = if tasks.is_empty() {
            read_bounded_ndjson_line(&mut stdin, MAX_REQUEST_FRAME_BYTES).await
        } else {
            tokio::select! {
                result = tasks.join_next() => {
                    if let Some(result) = result {
                        log_request_task_result(result);
                    }
                    continue;
                }
                line = read_bounded_ndjson_line(&mut stdin, MAX_REQUEST_FRAME_BYTES) => line,
            }
        };
        let line = match next_line {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => return Err(format!("stdin read failed: {error}")),
        };
        if line.trim().is_empty() {
            continue;
        }
        let request = match serde_json::from_str::<Request>(&line) {
            Ok(request) => request,
            Err(error) => {
                eprintln!(
                    "helper protocol parse error: {}",
                    sanitize_message(&error.to_string())
                );
                continue;
            }
        };

        if request.method == "shutdown" {
            abort_and_drain_request_tasks(&mut tasks).await;
            if let Some(writer) = &overlay_writer {
                writer.abort();
            }
            let response = handle_request(Arc::clone(&bridge), request).await;
            write_response(Arc::clone(&stdout), &response)
                .await
                .map_err(|error| error.to_string())?;
            break;
        }

        let bridge = Arc::clone(&bridge);
        let stdout = Arc::clone(&stdout);
        tasks.spawn(async move {
            let response = handle_request(bridge, request).await;
            if let Err(error) = write_response(stdout, &response).await {
                eprintln!(
                    "helper stdout error: {}",
                    sanitize_message(&error.to_string())
                );
            }
        });
    }

    abort_and_drain_request_tasks(&mut tasks).await;
    if let Some(writer) = &overlay_writer {
        writer.abort();
    }
    bridge.tunnel.shutdown().await;
    bridge.runtime.shutdown().await;
    Ok(())
}

type ExternalSkillRootsParams = (Vec<PathBuf>, Vec<PathBuf>, String, Option<String>);

fn external_skill_roots_params(params: &Value) -> Result<ExternalSkillRootsParams, ErrorPayload> {
    let invalid = || {
        ErrorPayload::new(
            "invalid_params",
            "setExternalSkillRoots needs roots, script_roots and expected_revision",
            "Reload the Skill folder settings and retry.",
        )
    };
    let paths = |key: &str| -> Result<Vec<PathBuf>, ErrorPayload> {
        params
            .get(key)
            .and_then(Value::as_array)
            .ok_or_else(invalid)?
            .iter()
            .map(|value| value.as_str().map(PathBuf::from).ok_or_else(invalid))
            .collect()
    };
    let roots = paths("roots")?;
    let script_roots = paths("script_roots")?;
    let expected_revision = param_str(params, "expected_revision")?.to_string();
    let verify_project_path = params
        .get("verify_project_path")
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok((roots, script_roots, expected_revision, verify_project_path))
}

async fn handle_request(bridge: Arc<Bridge>, mut request: Request) -> Response {
    if request.protocol_version != PROTOCOL_VERSION {
        return error_response(
            request.request_id,
            ErrorPayload::new(
                "protocol_version_incompatible",
                "Chadex and its helper use incompatible bridge protocol versions",
                "Reinstall matching Chadex app and helper versions.",
            )
            .with_details(json!({
                "expected": PROTOCOL_VERSION,
                "received": request.protocol_version
            })),
        );
    }

    let request_id = request.request_id.clone();
    match prewarm_interaction(&request.method) {
        PrewarmInteraction::Join => {
            if let Some((started_at_ms, started)) = bridge.join_prewarm().await {
                if request.method == "connectChatGPT" {
                    bridge.push_lifecycle("connect", "prewarm_join_wait", started_at_ms, started, "completed");
                }
            }
        }
        PrewarmInteraction::Cancel => bridge.cancel_prewarm().await,
        PrewarmInteraction::Ignore => {}
    }
    let result: Result<ResponseResult, ErrorPayload> = match request.method.as_str() {
        "getStatus" => Ok(ResponseResult::Snapshot(bridge.refreshed_snapshot(
            requested_mascot_jobs(&request.params),
        ).await)),
        "refreshRuntime" => bridge.refresh_runtime().await.map(ResponseResult::Snapshot),
        "observeChatGPTActivity" => bridge
            .observe_chatgpt_activity()
            .await
            .map(ResponseResult::Snapshot),
        "inspectProject" => match param_str(&request.params, "path") {
            Ok(path) => bridge
                .inspect_project(path)
                .await
                .map(ResponseResult::Project),
            Err(error) => Err(error),
        },
        "getSkillCatalog" => match param_str(&request.params, "path") {
            Ok(path) => bridge.skill_catalog(path).await.map(ResponseResult::Json),
            Err(error) => Err(error),
        },
        "getSkillInventory" => match param_str(&request.params, "path") {
            Ok(path) => bridge.skill_inventory(path).await.map(ResponseResult::Json),
            Err(error) => Err(error),
        },
        "discoverExternalSkillSources" => bridge
            .discover_external_skill_sources()
            .await
            .map(ResponseResult::Json),
        "getExternalSkillRoots" => bridge
            .runtime
            .external_skill_roots()
            .await
            .map(ResponseResult::Json)
            .map_err(ErrorPayload::from),
        "setExternalSkillRoots" => match external_skill_roots_params(&request.params) {
            Ok((roots, script_roots, expected_revision, verify_project_path)) => bridge
                .runtime
                .set_external_skill_roots(roots, script_roots, expected_revision, verify_project_path)
                .await
                .map(ResponseResult::Json)
                .map_err(ErrorPayload::from),
            Err(error) => Err(error),
        },
        "getSkillDefinition" => {
            let path = param_str(&request.params, "path");
            let skill_id = param_str(&request.params, "skill_id");
            let definition_revision = param_str(&request.params, "definition_revision");
            let package_revision = request
                .params
                .get("package_revision")
                .and_then(Value::as_str);
            match (path, skill_id, definition_revision) {
                (Ok(path), Ok(skill_id), Ok(definition_revision)) => bridge
                    .skill_definition(path, skill_id, definition_revision, package_revision)
                    .await
                    .map(ResponseResult::Json),
                (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => Err(error),
            }
        }
        "createProjectSkill" => {
            let path = param_str(&request.params, "path").map(str::to_owned);
            let skill_key = param_str(&request.params, "skill_key").map(str::to_owned);
            let content = take_param_string(&mut request.params, "content");
            match (path, skill_key, content) {
                (Ok(path), Ok(skill_key), Ok(content)) => bridge
                    .create_project_skill(&path, &skill_key, &content)
                    .await
                    .map(ResponseResult::Json),
                (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => Err(error),
            }
        }
        "installSkill" => {
            let path = param_str(&request.params, "path");
            let skill_key = param_str(&request.params, "skill_key");
            let artifact_path = param_str(&request.params, "artifact_path");
            match (path, skill_key, artifact_path) {
                (Ok(path), Ok(skill_key), Ok(artifact_path)) => bridge
                    .install_skill(path, skill_key, artifact_path)
                    .await
                    .map(ResponseResult::Json),
                (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => Err(error),
            }
        }
        "activateSkill" => {
            let path = param_str(&request.params, "path");
            let skill_key = param_str(&request.params, "skill_key");
            let package_revision = param_str(&request.params, "package_revision");
            let state_revision = param_str(&request.params, "state_revision");
            match (path, skill_key, package_revision, state_revision) {
                (Ok(path), Ok(skill_key), Ok(package_revision), Ok(state_revision)) => bridge
                    .activate_skill(path, skill_key, package_revision, state_revision)
                    .await
                    .map(ResponseResult::Json),
                (Err(error), _, _, _)
                | (_, Err(error), _, _)
                | (_, _, Err(error), _)
                | (_, _, _, Err(error)) => Err(error),
            }
        }
        "deactivateSkill" => {
            let path = param_str(&request.params, "path");
            let skill_key = param_str(&request.params, "skill_key");
            let state_revision = param_str(&request.params, "state_revision");
            match (path, skill_key, state_revision) {
                (Ok(path), Ok(skill_key), Ok(state_revision)) => bridge
                    .deactivate_skill(path, skill_key, state_revision)
                    .await
                    .map(ResponseResult::Json),
                (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => Err(error),
            }
        }
        "removeSkill" => {
            let path = param_str(&request.params, "path");
            let skill_key = param_str(&request.params, "skill_key");
            let state_revision = param_str(&request.params, "state_revision");
            match (path, skill_key, state_revision) {
                (Ok(path), Ok(skill_key), Ok(state_revision)) => bridge
                    .remove_skill(path, skill_key, state_revision)
                    .await
                    .map(ResponseResult::Json),
                (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => Err(error),
            }
        }
        "getProjectMemoryCatalog" => match param_str(&request.params, "path") {
            Ok(path) => bridge.memory_catalog(path).await.map(ResponseResult::Json),
            Err(error) => Err(error),
        },
        "getProjectMemory" => {
            let path = param_str(&request.params, "path");
            let memory_key = param_str(&request.params, "memory_key");
            let expected_revision = request
                .params
                .get("expected_revision")
                .and_then(Value::as_str);
            match (path, memory_key) {
                (Ok(path), Ok(memory_key)) => bridge
                    .memory_read(path, memory_key, expected_revision)
                    .await
                    .map(ResponseResult::Json),
                (Err(error), _) | (_, Err(error)) => Err(error),
            }
        }
        "setProjectMemory" => {
            let path = param_str(&request.params, "path");
            let memory_key = param_str(&request.params, "memory_key");
            let summary = param_str(&request.params, "summary");
            let body = param_str(&request.params, "body");
            let priority = param_str(&request.params, "priority");
            let bootstrap = param_bool(&request.params, "bootstrap");
            let tags = param_string_array(&request.params, "tags");
            let expected_revision = request
                .params
                .get("expected_revision")
                .and_then(Value::as_str);
            match (path, memory_key, summary, body, priority, bootstrap, tags) {
                (
                    Ok(path),
                    Ok(memory_key),
                    Ok(summary),
                    Ok(body),
                    Ok(priority),
                    Ok(bootstrap),
                    Ok(tags),
                ) => bridge
                    .memory_set(
                        path,
                        memory_key,
                        summary,
                        body,
                        priority,
                        bootstrap,
                        &tags,
                        expected_revision,
                    )
                    .await
                    .map(ResponseResult::Json),
                (Err(error), _, _, _, _, _, _)
                | (_, Err(error), _, _, _, _, _)
                | (_, _, Err(error), _, _, _, _)
                | (_, _, _, Err(error), _, _, _)
                | (_, _, _, _, Err(error), _, _)
                | (_, _, _, _, _, Err(error), _)
                | (_, _, _, _, _, _, Err(error)) => Err(error),
            }
        }
        "deleteProjectMemory" => {
            let path = param_str(&request.params, "path");
            let memory_key = param_str(&request.params, "memory_key");
            let expected_revision = param_str(&request.params, "expected_revision");
            match (path, memory_key, expected_revision) {
                (Ok(path), Ok(memory_key), Ok(expected_revision)) => bridge
                    .memory_delete(path, memory_key, expected_revision)
                    .await
                    .map(ResponseResult::Json),
                (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => Err(error),
            }
        }
        "activateProject" => match param_str(&request.params, "path") {
            Ok(path) => bridge
                .choose_target_project(path)
                .await
                .map(ResponseResult::Snapshot),
            Err(error) => Err(error),
        },
        "realignLocalProject" => match param_str(&request.params, "path") {
            Ok(path) => bridge
                .realign_local_project(path)
                .await
                .map(ResponseResult::Snapshot),
            Err(error) => Err(error),
        },
        "switchLocalProject" => match param_str(&request.params, "path") {
            Ok(path) => bridge
                .switch_local_project(path)
                .await
                .map(ResponseResult::Snapshot),
            Err(error) => Err(error),
        },
        "getComputerSafety" => serde_json::to_value(bridge.tunnel.computer_safety_snapshot())
            .map(ResponseResult::Json)
            .map_err(|_| {
                ErrorPayload::new(
                    "computer_safety_unavailable",
                    "Chadex could not encode Computer control safety state",
                    "Restart Chadex and retry.",
                )
            }),
        "setComputerControlMode" => match param_str(&request.params, "mode") {
            Ok(mode) => match ComputerControlMode::parse(mode) {
                Some(mode) => serde_json::to_value(bridge.tunnel.set_computer_control_mode(mode))
                    .map(ResponseResult::Json)
                    .map_err(|_| {
                        ErrorPayload::new(
                            "computer_safety_unavailable",
                            "Chadex could not encode Computer control safety state",
                            "Restart Chadex and retry.",
                        )
                    }),
                None => Err(ErrorPayload::new(
                    "invalid_params",
                    "Unsupported Computer control mode",
                    "Use read_only, ask_before_control, allow_session, or always_allow.",
                )),
            },
            Err(error) => Err(error),
        },
        "resumeComputerControl" => serde_json::to_value(bridge.tunnel.resume_computer_control())
            .map(ResponseResult::Json)
            .map_err(|_| {
                ErrorPayload::new(
                    "computer_safety_unavailable",
                    "Chadex could not encode Computer control safety state",
                    "Restart Chadex and retry.",
                )
            }),
        "approveComputerControlAlways" => match param_str(&request.params, "approval_id") {
            Ok(approval_id) if bridge.tunnel.approve_computer_control_always(approval_id) => {
                serde_json::to_value(bridge.tunnel.computer_safety_snapshot())
                    .map(ResponseResult::Json)
                    .map_err(|_| {
                        ErrorPayload::new(
                            "computer_safety_unavailable",
                            "Chadex could not encode Computer control safety state",
                            "Restart Chadex and retry.",
                        )
                    })
            }
            Ok(_) => Err(ErrorPayload::new(
                "computer_approval_expired",
                "Computer control approval is no longer pending",
                "Wait for a new approval request and try again.",
            )),
            Err(error) => Err(error),
        },
        "approveComputerControl" => match param_str(&request.params, "approval_id") {
            Ok(approval_id) if bridge.tunnel.approve_computer_control(approval_id) => {
                serde_json::to_value(bridge.tunnel.computer_safety_snapshot())
                    .map(ResponseResult::Json)
                    .map_err(|_| {
                        ErrorPayload::new(
                            "computer_safety_unavailable",
                            "Chadex could not encode Computer control safety state",
                            "Restart Chadex and retry.",
                        )
                    })
            }
            Ok(_) => Err(ErrorPayload::new(
                "computer_approval_stale",
                "This Computer control approval is no longer pending",
                "Refresh Computer control state before responding again.",
            )),
            Err(error) => Err(error),
        },
        "denyComputerControl" => match param_str(&request.params, "approval_id") {
            Ok(approval_id) if bridge.tunnel.deny_computer_control(approval_id) => {
                serde_json::to_value(bridge.tunnel.computer_safety_snapshot())
                    .map(ResponseResult::Json)
                    .map_err(|_| {
                        ErrorPayload::new(
                            "computer_safety_unavailable",
                            "Chadex could not encode Computer control safety state",
                            "Restart Chadex and retry.",
                        )
                    })
            }
            Ok(_) => Err(ErrorPayload::new(
                "computer_approval_stale",
                "This Computer control approval is no longer pending",
                "Refresh Computer control state before responding again.",
            )),
            Err(error) => Err(error),
        },
        "setComputerOverlayEvents" => match request.params.get("enabled").and_then(Value::as_bool) {
            Some(enabled) => {
                bridge.overlay.set_enabled(enabled);
                Ok(ResponseResult::Json(json!({
                    "enabled": bridge.overlay.is_enabled(),
                    "runner_channel": bridge.overlay.runner_channel(),
                    "counters": bridge.overlay.counters(),
                })))
            }
            None => Err(ErrorPayload::new(
                "invalid_params",
                "setComputerOverlayEvents needs a boolean enabled",
                "Send {\"enabled\": true} or {\"enabled\": false}.",
            )),
        },
        // Read-only: unlike setComputerOverlayEvents it never changes the enabled state.
        "getComputerOverlayStatus" => Ok(ResponseResult::Json(json!({
            "enabled": bridge.overlay.is_enabled(),
            "runner_channel": bridge.overlay.runner_channel(),
            "counters": bridge.overlay.counters(),
        }))),
        "stopComputerControl" => serde_json::to_value(bridge.tunnel.stop_computer_control())
            .map(ResponseResult::Json)
            .map_err(|_| {
                ErrorPayload::new(
                    "computer_safety_unavailable",
                    "Chadex could not encode Computer control safety state",
                    "Restart Chadex and retry.",
                )
            }),
        "configureLocalSetup" | "resumeService" => bridge
            .ensure_runtime_for_target()
            .await
            .map(ResponseResult::Snapshot),
        "prewarmRuntime" => bridge.prewarm_runtime().await.map(ResponseResult::Snapshot),
        "connectChatGPT" => bridge.connect_chatgpt().await.map(ResponseResult::Snapshot),
        "startTunnel" => bridge.start_tunnel().await.map(ResponseResult::Snapshot),
        "stopTunnel" | "disconnectAI" => bridge.stop_tunnel().await.map(ResponseResult::Snapshot),
        "stopLocalService" => bridge
            .stop_local_service()
            .await
            .map(ResponseResult::Snapshot),
        "provideCredential" => {
            let tunnel_id = take_param_string(&mut request.params, "tunnel_id");
            let api_key = take_param_string(&mut request.params, "api_key").map(Zeroizing::new);
            match (tunnel_id, api_key) {
                (Ok(tunnel_id), Ok(api_key)) => bridge
                    .provide_credentials(&tunnel_id, api_key)
                    .await
                    .map(ResponseResult::Snapshot),
                (Err(error), _) | (_, Err(error)) => Err(error),
            }
        }
        "clearCredential" => bridge
            .clear_credentials()
            .await
            .map(ResponseResult::Snapshot),
        "updateProxySettings" => update_proxy(&bridge, &request.params).await,
        "queryActivities" => {
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(ACTIVITY_QUERY_MAX)
                .clamp(1, ACTIVITY_QUERY_MAX);
            let mut entries = bridge.runtime.activity();
            let start = entries.len().saturating_sub(limit);
            if start > 0 {
                entries.drain(..start);
            }
            Ok(ResponseResult::Activities(entries))
        }
        "queryPerformanceTraces" => {
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(PERFORMANCE_QUERY_MAX)
                .clamp(1, PERFORMANCE_QUERY_MAX);
            Ok(ResponseResult::Performance(
                bridge.performance.snapshot(limit),
            ))
        }
        "queryLifecyclePerformanceTraces" => {
            let limit = request
                .params
                .get("limit")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(PERFORMANCE_QUERY_MAX)
                .clamp(1, PERFORMANCE_QUERY_MAX);
            Ok(ResponseResult::LifecyclePerformance(
                bridge.performance.lifecycle_snapshot(limit),
            ))
        }
        "cancelTask" => {
            let project = param_str(&request.params, "project").map(str::to_owned);
            let task_id = param_str(&request.params, "task_id").map(str::to_owned);
            match (project, task_id) {
                (Ok(project), Ok(task_id)) => bridge
                    .cancel_task(&project, &task_id)
                    .await
                    .map(ResponseResult::Snapshot),
                (Err(error), _) | (_, Err(error)) => Err(error),
            }
        }
        "cancelOperation" => match param_str(&request.params, "operation_id") {
            Ok(operation_id) => bridge
                .runtime
                .cancel_operation(operation_id)
                .map_err(ErrorPayload::from)
                .map(|snapshot| ResponseResult::Snapshot(bridge.snapshot_from(snapshot))),
            Err(error) => Err(error),
        },
        "shutdown" => Ok(ResponseResult::Snapshot(bridge.snapshot())),
        other => Err(ErrorPayload::new(
            "method_not_found",
            format!("Unknown Chadex helper method: {other}"),
            "Update the app and helper together.",
        )),
    };

    match result {
        Ok(result) => Response {
            protocol_version: PROTOCOL_VERSION,
            request_id,
            result: Some(result),
            error: None,
        },
        Err(error) => error_response(request_id, error),
    }
}

async fn update_proxy(bridge: &Bridge, params: &Value) -> Result<ResponseResult, ErrorPayload> {
    let mode = match param_str(params, "mode")? {
        "auto" => RuntimeProxyMode::Auto,
        "direct" => RuntimeProxyMode::Direct,
        "custom" => RuntimeProxyMode::Custom,
        _ => {
            return Err(ErrorPayload::new(
                "invalid_params",
                "Unsupported proxy mode",
                "Use auto, direct, or custom.",
            ))
        }
    };
    let custom_url = params.get("custom_url").and_then(Value::as_str);
    bridge
        .runtime
        .update_tunnel_proxy(mode, custom_url)
        .await
        .map_err(ErrorPayload::from)
        .map(|snapshot| ResponseResult::Snapshot(bridge.snapshot_from(snapshot)))
}

fn should_restore_connection_after_project_switch(state: TunnelState) -> bool {
    matches!(state, TunnelState::Starting | TunnelState::Ready)
}

fn project_inspection(project: RuntimeProject) -> ProjectInspection {
    let path = PathBuf::from(&project.path);
    let metadata = fs::metadata(&path).ok();
    ProjectInspection {
        path: project.path,
        allowed_root: project.allowed_root,
        is_git_repository: project.is_git_repository,
        readable: fs::read_dir(&path).is_ok(),
        writable: metadata.is_some_and(|metadata| !metadata.permissions().readonly()),
    }
}

fn requested_mascot_jobs(params: &Value) -> bool {
    params.get("include_mascot_jobs").and_then(Value::as_bool).unwrap_or(false)
}

fn param_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, ErrorPayload> {
    params.get(key).and_then(Value::as_str).ok_or_else(|| {
        ErrorPayload::new(
            "invalid_params",
            format!("Missing required parameter: {key}"),
            "Update the app and helper together, then retry.",
        )
    })
}

fn param_bool(params: &Value, key: &str) -> Result<bool, ErrorPayload> {
    params.get(key).and_then(Value::as_bool).ok_or_else(|| {
        ErrorPayload::new(
            "invalid_params",
            format!("Missing required boolean parameter: {key}"),
            "Update the app and helper together, then retry.",
        )
    })
}

fn param_string_array(params: &Value, key: &str) -> Result<Vec<String>, ErrorPayload> {
    let values = params.get(key).and_then(Value::as_array).ok_or_else(|| {
        ErrorPayload::new(
            "invalid_params",
            format!("Missing required array parameter: {key}"),
            "Update the app and helper together, then retry.",
        )
    })?;
    values
        .iter()
        .map(|value| {
            value.as_str().map(str::to_owned).ok_or_else(|| {
                ErrorPayload::new(
                    "invalid_params",
                    format!("Parameter must contain only strings: {key}"),
                    "Update the app and helper together, then retry.",
                )
            })
        })
        .collect()
}

fn take_param_string(params: &mut Value, key: &str) -> Result<String, ErrorPayload> {
    let object = params.as_object_mut().ok_or_else(|| {
        ErrorPayload::new(
            "invalid_params",
            "Request params must be a JSON object",
            "Update the app and helper together, then retry.",
        )
    })?;
    let value = object.remove(key).ok_or_else(|| {
        ErrorPayload::new(
            "invalid_params",
            format!("Missing required parameter: {key}"),
            "Update the app and helper together, then retry.",
        )
    })?;
    value.as_str().map(str::to_owned).ok_or_else(|| {
        ErrorPayload::new(
            "invalid_params",
            format!("Parameter must be a string: {key}"),
            "Update the app and helper together, then retry.",
        )
    })
}

fn error_response(request_id: String, error: ErrorPayload) -> Response {
    Response {
        protocol_version: PROTOCOL_VERSION,
        request_id,
        result: None,
        error: Some(error),
    }
}

/// Writes `computer_overlay` event frames to stdout, one line each, sharing the
/// stdout lock with responses (each write holds it only for one line).
async fn write_overlay_events<W: AsyncWrite + Unpin>(
    stdout: Arc<Mutex<W>>,
    mut events: mpsc::Receiver<Value>,
) {
    while let Some(data) = events.recv().await {
        let encoded = computer_overlay::encode_event_frame(&data);
        let mut stdout = stdout.lock().await;
        if stdout.write_all(&encoded).await.is_err() || stdout.flush().await.is_err() {
            // Best effort: the App going away ends the helper through stdin EOF.
            break;
        }
    }
}

async fn write_response(
    stdout: Arc<Mutex<tokio::io::Stdout>>,
    response: &Response,
) -> std::io::Result<()> {
    let mut encoded = serde_json::to_vec(response)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    encoded.push(b'\n');
    let mut stdout = stdout.lock().await;
    stdout.write_all(&encoded).await?;
    stdout.flush().await
}

fn chadex_data_dir() -> Result<PathBuf, ErrorPayload> {
    chadex_data_dir_from_env(|name| std::env::var_os(name))
}

fn chadex_data_dir_from_env(
    var_os: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<PathBuf, ErrorPayload> {
    if let Some(path) = var_os("CHADEX_DATA_DIR") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return Ok(path);
        }
    }
    #[cfg(windows)]
    {
        let local_app_data = var_os("LOCALAPPDATA")
            .filter(|path| !path.is_empty())
            .ok_or_else(|| {
                ErrorPayload::new(
                    "home_directory_unavailable",
                    "Chadex could not determine the local application data directory",
                    "Launch Chadex from a normal Windows user session and retry.",
                )
            })?;
        return Ok(PathBuf::from(local_app_data).join("Chadex").join("runtime"));
    }
    #[cfg(not(windows))]
    {
        let home = var_os("HOME").ok_or_else(|| {
            ErrorPayload::new(
                "home_directory_unavailable",
                "Chadex could not determine the user home directory",
                "Launch Chadex from a normal macOS user session and retry.",
            )
        })?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("Chadex")
            .join("runtime"))
    }
}

fn chadex_resource_dir() -> Result<PathBuf, ErrorPayload> {
    if let Some(path) = std::env::var_os("CHADEX_RESOURCE_DIR") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return Ok(path);
        }
    }
    let executable = std::env::current_exe().map_err(|error| {
        ErrorPayload::new(
            "resource_directory_unavailable",
            "Chadex could not locate its bundled resources",
            "Rebuild or reinstall Chadex and retry.",
        )
        .with_details(json!({ "io_kind": format!("{:?}", error.kind()) }))
    })?;
    let bundled = executable
        .parent()
        .and_then(Path::parent)
        .map(|contents| contents.join("Resources"));
    Ok(bundled.unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("Resources")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_data_directory_takes_precedence() {
        let directory = PathBuf::from("fixture data 中文");
        let actual = chadex_data_dir_from_env(|name| {
            (name == "CHADEX_DATA_DIR").then(|| directory.clone().into_os_string())
        })
        .unwrap();
        assert_eq!(actual, directory);
    }

    #[test]
    fn missing_default_data_directory_fails_closed() {
        assert!(chadex_data_dir_from_env(|_| None).is_err());
    }

    #[cfg(windows)]
    #[test]
    fn windows_data_directory_uses_local_app_data_without_home() {
        let directory = PathBuf::from(r"C:\Users\使用者 Name\AppData\Local");
        let actual = chadex_data_dir_from_env(|name| match name {
            "LOCALAPPDATA" => Some(directory.clone().into_os_string()),
            "CHADEX_DATA_DIR" => Some(std::ffi::OsString::new()),
            _ => None,
        })
        .unwrap();
        assert_eq!(actual, directory.join("Chadex").join("runtime"));
        assert!(chadex_data_dir_from_env(|_| Some(std::ffi::OsString::new())).is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn macos_data_directory_preserves_existing_location() {
        let directory = PathBuf::from("/Users/fixture");
        let actual = chadex_data_dir_from_env(|name| {
            (name == "HOME").then(|| directory.clone().into_os_string())
        })
        .unwrap();
        assert_eq!(
            actual,
            directory.join("Library/Application Support/Chadex/runtime")
        );
    }

    #[test]
    fn secret_parameter_is_removed_from_request_object() {
        let mut params = json!({ "tunnel_id": "tunnel_one", "api_key": "secret-value" });
        let secret = Zeroizing::new(take_param_string(&mut params, "api_key").unwrap());
        assert_eq!(secret.as_str(), "secret-value");
        assert!(!serde_json::to_string(&params)
            .unwrap()
            .contains("secret-value"));
    }

    #[test]
    fn project_memory_parameter_helpers_require_exact_types() {
        let params = json!({
            "bootstrap": true,
            "tags": ["architecture", "workflow"]
        });
        assert!(param_bool(&params, "bootstrap").unwrap());
        assert_eq!(
            param_string_array(&params, "tags").unwrap(),
            vec!["architecture".to_string(), "workflow".to_string()]
        );

        assert!(param_bool(&json!({"bootstrap": "true"}), "bootstrap").is_err());
        assert!(param_string_array(&json!({"tags": ["architecture", 1]}), "tags").is_err());
    }

    #[test]
    fn project_switch_preserves_active_connection_intent_only_for_live_tunnel_states() {
        assert!(should_restore_connection_after_project_switch(
            TunnelState::Starting
        ));
        assert!(should_restore_connection_after_project_switch(
            TunnelState::Ready
        ));
        assert!(!should_restore_connection_after_project_switch(
            TunnelState::Unconfigured
        ));
        assert!(!should_restore_connection_after_project_switch(
            TunnelState::Stopped
        ));
        assert!(!should_restore_connection_after_project_switch(
            TunnelState::Error
        ));
    }

    #[test]
    fn backend_snapshot_serializes_connection_separately_from_project_verification() {
        let desktop = RuntimeSnapshot {
            runtime_configured: true,
            runtime_autostart: true,
            readiness: crate::chadex_core::runtime::RuntimeReadiness {
                runtime_ready: true,
                needs_attention: false,
                summary: "Runtime ready".to_string(),
                next_action: None,
                summary_kind: "runtime_ready",
                server: "ready",
                runner: "ready",
                exposure: "local_ready",
                project: "ready",
            },
            project: None,
            current_operation: None,
            activity_sequence: 0,
            tunnel_proxy_effective_url: Some("https://private.example/token-sentinel".to_string()),
        };
        let tunnel = TunnelSnapshot {
            state: TunnelState::Ready,
            configured: true,
            tunnel_id: Some("tunnel-id-sentinel".to_string()),
            epoch: 8,
            last_error: Some("credential-sentinel".to_string()),
        };
        let snapshot = BackendSnapshot {
            phase: ConnectionPhase::WaitingForChatGptVerification,
            graphify: GraphifyStatus::unavailable(),
            selected_project: None,
            tunnel_ready: true,
            chat_gpt_connected: true,
            chat_gpt_verified_for_selected_project: false,
            last_verified_at_ms: None,
            current_operation: None,
            task_progress: None,
            mascot_jobs: None,
            runtime_status: Some(RuntimeStatus::from_snapshot(&desktop)),
            tunnel_status: Some(TunnelStatus::from_snapshot(&tunnel)),
            error: None,
            activity_sequence: 0,
            state_revision: 42,
        };
        let value = serde_json::to_value(snapshot).unwrap();
        assert_eq!(value["phase"], "waiting_for_chatgpt_verification");
        assert_eq!(value["chat_gpt_connected"], true);
        assert_eq!(value["chat_gpt_verified_for_selected_project"], false);
        assert_eq!(value["state_revision"], 42);
        assert!(value["mascot_jobs"].is_null());
        assert_eq!(
            value["runtime_status"],
            json!({
                "runtime_configured": true,
                "runtime_ready": true,
                "needs_attention": false,
                "summary": "Runtime ready",
                "next_action": null,
                "summary_kind": "runtime_ready",
                "server": "ready",
                "runner": "ready",
                "exposure": "local_ready",
                "project": "ready"
            })
        );
        assert_eq!(
            value["tunnel_status"],
            json!({ "configured": true, "state": "ready" })
        );
        let additions = json!({
            "runtime_status": value["runtime_status"],
            "tunnel_status": value["tunnel_status"]
        })
        .to_string();
        for sensitive_value in [
            "https://private.example",
            "token-sentinel",
            "tunnel-id-sentinel",
            "credential-sentinel",
        ] {
            assert!(!additions.contains(sensitive_value));
        }
    }

    fn prewarm_runtime_snapshot() -> RuntimeSnapshot {
        RuntimeSnapshot {
            runtime_configured: true,
            runtime_autostart: true,
            readiness: crate::chadex_core::runtime::RuntimeReadiness {
                runtime_ready: false,
                needs_attention: false,
                summary: "Stopped".to_string(),
                next_action: None,
                summary_kind: "runtime_stopped",
                server: "stopped",
                runner: "stopped",
                exposure: "disabled",
                project: "configured",
            },
            project: Some(RuntimeProject {
                path: "/project".to_string(),
                allowed_root: "/".to_string(),
                is_git_repository: false,
            }),
            current_operation: None,
            activity_sequence: 0,
            tunnel_proxy_effective_url: None,
        }
    }

    fn runtime_operation(background: bool) -> crate::chadex_core::runtime::RuntimeOperation {
        crate::chadex_core::runtime::RuntimeOperation {
            id: "desktop-operation-1".to_string(),
            kind: "runtime_resume",
            phase: crate::chadex_core::runtime::RuntimeOperationPhase::Running,
            started_at_ms: 1,
            cancellable: true,
            background,
        }
    }

    fn selected_project(path: &str) -> ProjectInspection {
        ProjectInspection {
            path: path.to_string(),
            allowed_root: path.to_string(),
            is_git_repository: false,
            readable: true,
            writable: true,
        }
    }

    #[test]
    fn prewarm_activates_the_selection_when_the_resumed_runtime_serves_a_stale_project() {
        // Reproduces the launch where the saved runtime project (the last one
        // activated) lags the selection: the resume brings up "/project" while
        // the App selected "/selected". The warm-up must activate the selection.
        let mut resumed = prewarm_runtime_snapshot();
        resumed.readiness.runtime_ready = true;
        let selected = selected_project("/selected");
        assert_eq!(
            prewarm_activation_path(&resumed, Some(&selected)),
            Some("/selected")
        );
    }

    #[test]
    fn prewarm_skips_activation_when_aligned_not_ready_or_unselected() {
        let mut resumed = prewarm_runtime_snapshot();
        resumed.readiness.runtime_ready = true;
        let same = selected_project("/project");
        assert_eq!(prewarm_activation_path(&resumed, Some(&same)), None);
        assert_eq!(prewarm_activation_path(&resumed, None), None);

        let other = selected_project("/selected");
        let stopped = prewarm_runtime_snapshot();
        assert_eq!(prewarm_activation_path(&stopped, Some(&other)), None);
    }

    /// In-memory runtime for the warm-up orchestration: `resume` brings the
    /// saved ("/old") project up; `activate` takes `activation_delay` and
    /// honours `cancel_background` like the real background operation.
    struct FakeRuntime {
        project: StdMutex<String>,
        ready: std::sync::atomic::AtomicBool,
        cancelled: std::sync::atomic::AtomicBool,
        activation_started: std::sync::atomic::AtomicBool,
        activation_delay: std::time::Duration,
    }

    impl FakeRuntime {
        fn new(saved_project: &str, activation_delay_ms: u64) -> Self {
            Self {
                project: StdMutex::new(saved_project.to_string()),
                ready: std::sync::atomic::AtomicBool::new(false),
                cancelled: std::sync::atomic::AtomicBool::new(false),
                activation_started: std::sync::atomic::AtomicBool::new(false),
                activation_delay: std::time::Duration::from_millis(activation_delay_ms),
            }
        }

        fn project(&self) -> String {
            self.project.lock().unwrap().clone()
        }

        fn cancel_background(&self) {
            self.cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
        }

        async fn wait_for_activation(&self) {
            while !self.activation_started.load(std::sync::atomic::Ordering::SeqCst) {
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
        }
    }

    impl PrewarmPort for FakeRuntime {
        async fn resume(&self) -> Result<RuntimeSnapshot, ChadexError> {
            self.ready.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(self.runtime_snapshot())
        }

        fn runtime_snapshot(&self) -> RuntimeSnapshot {
            let mut snapshot = prewarm_runtime_snapshot();
            snapshot.readiness.runtime_ready = self.ready.load(std::sync::atomic::Ordering::SeqCst);
            snapshot.project.as_mut().unwrap().path = self.project();
            snapshot
        }

        async fn activate(&self, path: &str) -> Result<RuntimeSnapshot, ChadexError> {
            self.activation_started.store(true, std::sync::atomic::Ordering::SeqCst);
            let deadline = tokio::time::Instant::now() + self.activation_delay;
            while tokio::time::Instant::now() < deadline {
                if self.cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(ChadexError::new("desktop_operation_cancelled", "cancelled", "retry"));
                }
                tokio::time::sleep(std::time::Duration::from_millis(2)).await;
            }
            *self.project.lock().unwrap() = path.to_string();
            Ok(self.runtime_snapshot())
        }
    }

    /// What the dispatcher does for a contending request (switch / stop):
    /// cancel the warm-up first, and only then take the switch lock.
    async fn cancel_like_dispatch(tracker: &PrewarmTracker, fake: &FakeRuntime) {
        tracker
            .cancel(
                std::time::Duration::from_secs(5),
                std::time::Duration::from_millis(10),
                || {
                    fake.cancel_background();
                    std::future::ready(())
                },
            )
            .await;
    }

    #[tokio::test]
    async fn prewarm_aligns_a_stale_resumed_project_with_the_selection() {
        let fake = FakeRuntime::new("/old", 0);
        let tracker = PrewarmTracker::new();
        let lock = Mutex::new(());
        let run = tracker.begin().unwrap();
        let steps = run_prewarm_steps(&fake, &run, &lock, || Some(selected_project("/selected"))).await;
        drop(run);
        assert_eq!(fake.project(), "/selected");
        assert_eq!(steps.resume_completion, "completed");
        assert_eq!(steps.activation.map(|(_, _, c)| c), Some("completed"));
        assert_eq!(steps.total_completion(), "completed");
        assert!(steps.result.is_ok());
    }

    #[tokio::test]
    async fn prewarm_skips_activation_when_the_resumed_project_is_selected() {
        let fake = FakeRuntime::new("/selected", 0);
        let tracker = PrewarmTracker::new();
        let lock = Mutex::new(());
        let run = tracker.begin().unwrap();
        let steps = run_prewarm_steps(&fake, &run, &lock, || Some(selected_project("/selected"))).await;
        assert!(steps.activation.is_none());
        assert!(!fake.activation_started.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn switch_during_prewarm_activation_cancels_it_without_deadlock_and_wins() {
        let fake = FakeRuntime::new("/old", 5_000);
        let tracker = PrewarmTracker::new();
        let lock = Mutex::new(());
        let target = RwLock::new(Some(selected_project("/selected")));
        let prewarm = async {
            let run = tracker.begin().unwrap();
            let steps = run_prewarm_steps(&fake, &run, &lock, || target.read().unwrap().clone()).await;
            drop(run);
            steps
        };
        let switch = async {
            fake.wait_for_activation().await;
            cancel_like_dispatch(&tracker, &fake).await;
            // switch_local_project: under the lock, select and activate.
            let _switch = lock.lock().await;
            *target.write().unwrap() = Some(selected_project("/other"));
            *fake.project.lock().unwrap() = "/other".to_string();
        };
        let (steps, ()) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::join!(prewarm, switch)
        })
        .await
        .expect("switching during the warm-up must not deadlock");
        assert_eq!(fake.project(), "/other", "the switch decides the final project");
        assert_eq!(steps.resume_completion, "completed", "the resume itself succeeded");
        assert_eq!(steps.activation.map(|(_, _, c)| c), Some("cancelled"));
        assert_eq!(steps.total_completion(), "cancelled");
        assert!(tracker.receiver().is_none());
    }

    #[tokio::test]
    async fn stop_during_prewarm_activation_cancels_it_promptly() {
        let fake = FakeRuntime::new("/old", 5_000);
        let tracker = PrewarmTracker::new();
        let lock = Mutex::new(());
        let prewarm = async {
            let run = tracker.begin().unwrap();
            let steps = run_prewarm_steps(&fake, &run, &lock, || Some(selected_project("/selected"))).await;
            drop(run);
            steps
        };
        let stop = async {
            fake.wait_for_activation().await;
            cancel_like_dispatch(&tracker, &fake).await;
            fake.ready.store(false, std::sync::atomic::Ordering::SeqCst);
        };
        let (steps, ()) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            tokio::join!(prewarm, stop)
        })
        .await
        .expect("stopping during the warm-up must not deadlock");
        assert_eq!(fake.project(), "/old");
        assert_eq!(steps.resume_completion, "completed");
        assert_eq!(steps.activation.map(|(_, _, c)| c), Some("cancelled"));
        assert!(steps.result.is_err());
    }

    #[tokio::test]
    async fn connect_join_waits_until_the_prewarm_activation_finished() {
        let fake = FakeRuntime::new("/old", 150);
        let tracker = PrewarmTracker::new();
        let lock = Mutex::new(());
        let prewarm = async {
            let run = tracker.begin().unwrap();
            let steps = run_prewarm_steps(&fake, &run, &lock, || Some(selected_project("/selected"))).await;
            drop(run);
            steps
        };
        let connect = async {
            fake.wait_for_activation().await;
            tracker.join().await;
            fake.project()
        };
        let (steps, seen_by_connect) = tokio::join!(prewarm, connect);
        assert_eq!(seen_by_connect, "/selected", "Connect must not start before the alignment ends");
        assert_eq!(steps.activation.map(|(_, _, c)| c), Some("completed"));
    }

    #[test]
    fn prewarm_traces_keep_the_resume_result_when_the_activation_fails() {
        let (bridge, root) = temp_bridge("prewarm-activation-trace");
        let resume_started = Instant::now() - std::time::Duration::from_millis(500);
        let steps = PrewarmSteps {
            resume_started: (now_ms(), resume_started),
            resume_finished: resume_started + std::time::Duration::from_millis(10),
            resume_completion: "completed",
            activation: Some((now_ms(), Instant::now(), "failed")),
            result: Err(ChadexError::new("x", "y", "z")),
        };
        bridge.record_prewarm_steps(now_ms(), Instant::now(), &steps);
        let traces = bridge.performance.lifecycle_snapshot(10);
        let resume = traces.iter().find(|t| t.phase == "runtime_resume").unwrap();
        assert_eq!(resume.total_us, 10_000, "resume time excludes the lock wait and activation");
        let seen: Vec<(&str, &str, &str)> = traces
            .iter()
            .map(|t| (t.operation.as_str(), t.phase.as_str(), t.completion.as_str()))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("prewarm", "runtime_resume", "completed"),
                ("prewarm", "project_activation", "failed"),
                ("prewarm", "total", "failed"),
                ("launch", "helper_start_to_runtime_ready", "completed"),
            ]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn realign_only_activates_the_current_selection_in_a_ready_runtime() {
        let mut ready = prewarm_runtime_snapshot();
        ready.readiness.runtime_ready = true;
        let selected = selected_project("/selected");

        let stale = realign_activation_path(&ready, Some(&selected), "/elsewhere").unwrap_err();
        assert_eq!(stale.code, "project_realign_stale");
        assert_eq!(
            realign_activation_path(&ready, None, "/selected").unwrap_err().code,
            "project_realign_stale"
        );
        assert_eq!(
            realign_activation_path(&ready, Some(&selected), "/selected").unwrap(),
            Some("/selected")
        );
        assert_eq!(
            realign_activation_path(&prewarm_runtime_snapshot(), Some(&selected), "/selected").unwrap(),
            None,
            "a runtime that is not ready is left alone"
        );
        let aligned = selected_project("/project");
        assert_eq!(realign_activation_path(&ready, Some(&aligned), "/project").unwrap(), None);
    }

    #[test]
    fn realign_joins_a_running_prewarm_instead_of_cancelling_it() {
        assert_eq!(prewarm_interaction("realignLocalProject"), PrewarmInteraction::Join);
    }

    #[test]
    fn prewarm_resumes_only_a_configured_autostart_runtime_with_a_target() {
        assert_eq!(
            prewarm_decision(&prewarm_runtime_snapshot(), true),
            PrewarmDecision::Resume
        );
    }

    #[test]
    fn prewarm_skips_when_runtime_was_never_configured() {
        let mut snapshot = prewarm_runtime_snapshot();
        snapshot.runtime_configured = false;
        assert_eq!(
            prewarm_decision(&snapshot, true),
            PrewarmDecision::NotConfigured
        );
    }

    #[test]
    fn prewarm_skips_after_an_explicit_stop() {
        let mut snapshot = prewarm_runtime_snapshot();
        snapshot.runtime_autostart = false;
        assert_eq!(
            prewarm_decision(&snapshot, true),
            PrewarmDecision::ExplicitlyStopped
        );
    }

    #[test]
    fn prewarm_skips_when_another_operation_is_in_flight() {
        let mut snapshot = prewarm_runtime_snapshot();
        snapshot.current_operation = Some(runtime_operation(false));
        assert_eq!(
            prewarm_decision(&snapshot, true),
            PrewarmDecision::OperationInFlight
        );
    }

    #[test]
    fn prewarm_skips_without_a_target_project_or_runtime_project() {
        assert_eq!(
            prewarm_decision(&prewarm_runtime_snapshot(), false),
            PrewarmDecision::NoTargetProject
        );
        let mut snapshot = prewarm_runtime_snapshot();
        snapshot.project = None;
        assert_eq!(
            prewarm_decision(&snapshot, true),
            PrewarmDecision::NoRuntimeProject
        );
    }

    #[test]
    fn prewarm_skips_an_already_ready_runtime() {
        let mut snapshot = prewarm_runtime_snapshot();
        snapshot.readiness.runtime_ready = true;
        assert_eq!(
            prewarm_decision(&snapshot, true),
            PrewarmDecision::AlreadyReady
        );
    }

    #[test]
    fn background_operation_is_never_presented_as_user_work() {
        let mut snapshot = prewarm_runtime_snapshot();
        snapshot.current_operation = Some(runtime_operation(true));
        assert_eq!(visible_operation(&snapshot), None);

        snapshot.current_operation = Some(runtime_operation(false));
        let visible = visible_operation(&snapshot).expect("user operations stay visible");
        assert_eq!(visible.kind, "runtime_resume");
        assert!(visible.cancellable);
    }

    #[test]
    fn only_contending_requests_join_or_cancel_the_prewarm() {
        for method in ["connectChatGPT", "startTunnel", "configureLocalSetup", "resumeService"] {
            assert_eq!(prewarm_interaction(method), PrewarmInteraction::Join, "{method}");
        }
        for method in ["stopLocalService", "stopTunnel", "disconnectAI", "switchLocalProject"] {
            assert_eq!(prewarm_interaction(method), PrewarmInteraction::Cancel, "{method}");
        }
        for method in [
            "getStatus",
            "prewarmRuntime",
            "activateProject",
            "clearCredential",
            "provideCredential",
            "cancelTask",
            "cancelOperation",
            "queryActivities",
        ] {
            assert_eq!(prewarm_interaction(method), PrewarmInteraction::Ignore, "{method}");
        }
    }

    #[tokio::test]
    async fn joining_waits_for_the_prewarm_and_returns_immediately_when_idle() {
        let tracker = Arc::new(PrewarmTracker::new());
        tokio::time::timeout(std::time::Duration::from_millis(200), tracker.join())
            .await
            .expect("join with no prewarm must not wait");

        let run = tracker.begin().expect("first prewarm starts");
        assert!(tracker.begin().is_none(), "only one prewarm at a time");
        let waiter = {
            let tracker = Arc::clone(&tracker);
            tokio::spawn(async move { tracker.join().await })
        };
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(!waiter.is_finished(), "join must wait for the prewarm");
        drop(run);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("join resumes when the prewarm finishes")
            .unwrap();
        assert!(tracker.begin().is_some(), "slot is reusable afterwards");
    }

    #[tokio::test]
    async fn cancelling_stops_the_prewarm_and_proceeds_without_waiting_it_out() {
        let tracker = PrewarmTracker::new();
        let run = tracker.begin().unwrap();
        let cancels = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // The prewarm unwinds once the second cancellation attempt lands.
        let worker_cancels = Arc::clone(&cancels);
        let worker = async move {
            while worker_cancels.load(std::sync::atomic::Ordering::SeqCst) < 2 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            assert!(run.cancel_requested());
            drop(run);
        };
        let started = std::time::Instant::now();
        let counter = Arc::clone(&cancels);
        let cancel = tracker.cancel(
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(20),
            move || {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::future::ready(())
            },
        );
        tokio::join!(worker, cancel);
        assert!(cancels.load(std::sync::atomic::Ordering::SeqCst) >= 2);
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        assert!(tracker.receiver().is_none());
    }

    #[tokio::test]
    async fn cancelling_is_bounded_and_a_noop_without_a_prewarm() {
        let tracker = PrewarmTracker::new();
        tracker
            .cancel(
                std::time::Duration::from_millis(50),
                std::time::Duration::from_millis(10),
                || std::future::ready(()),
            )
            .await;

        let _stuck = tracker.begin().unwrap();
        let started = std::time::Instant::now();
        tracker
            .cancel(
                std::time::Duration::from_millis(100),
                std::time::Duration::from_millis(10),
                || std::future::ready(()),
            )
            .await;
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[tokio::test]
    async fn aborted_prewarm_releases_joiners() {
        let tracker = Arc::new(PrewarmTracker::new());
        let task = {
            let tracker = Arc::clone(&tracker);
            tokio::spawn(async move {
                let _run = tracker.begin().unwrap();
                std::future::pending::<()>().await;
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert!(tracker.receiver().is_some());
        task.abort();
        let _ = task.await;
        tokio::time::timeout(std::time::Duration::from_millis(500), tracker.join())
            .await
            .expect("an aborted prewarm must not strand joiners");
    }

    #[tokio::test]
    async fn prewarm_on_an_unconfigured_bridge_skips_without_an_operation() {
        let root = std::env::temp_dir().join(format!(
            "chadex-bridge-prewarm-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let bridge = Bridge::with_dirs(root.join("data"), root.join("resources")).unwrap();
        let snapshot = bridge.prewarm_runtime().await.unwrap();
        assert!(snapshot.current_operation.is_none());
        assert_ne!(snapshot.phase, ConnectionPhase::Preparing);
        assert!(bridge.prewarm.receiver().is_none());
        let traces = bridge.performance.lifecycle_snapshot(100);
        let seen: Vec<(&str, &str, &str)> = traces
            .iter()
            .map(|t| (t.operation.as_str(), t.phase.as_str(), t.completion.as_str()))
            .collect();
        assert_eq!(
            seen,
            vec![
                ("prewarm", "total", "skipped:not_configured"),
                ("launch", "helper_start_to_runtime_ready", "skipped:not_configured"),
            ]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    fn temp_bridge(tag: &str) -> (Bridge, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "chadex-bridge-{tag}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let bridge = Bridge::with_dirs(root.join("data"), root.join("resources")).unwrap();
        (bridge, root)
    }

    #[test]
    fn prewarm_skip_reasons_use_a_fixed_vocabulary() {
        assert_eq!(PrewarmDecision::ExplicitlyStopped.skip_completion(), "skipped:explicit_stop");
        assert_eq!(PrewarmDecision::AlreadyReady.skip_completion(), "skipped:already_ready");
        assert_eq!(PrewarmDecision::NoTargetProject.skip_completion(), "skipped:no_project");
        assert_eq!(
            PrewarmDecision::OperationInFlight.skip_completion(),
            "skipped:operation_in_flight"
        );
    }

    #[test]
    fn prewarm_records_resume_total_and_launch_traces_for_each_outcome() {
        let (bridge, root) = temp_bridge("prewarm-trace");
        for completion in ["completed", "cancelled", "failed"] {
            bridge.record_prewarm(now_ms(), Instant::now(), Some((now_ms(), Instant::now())), completion);
        }
        let traces = bridge.performance.lifecycle_snapshot(100);
        assert_eq!(traces.len(), 9);
        for (index, completion) in ["completed", "cancelled", "failed"].iter().enumerate() {
            let group = &traces[index * 3..index * 3 + 3];
            let phases: Vec<(&str, &str)> =
                group.iter().map(|t| (t.operation.as_str(), t.phase.as_str())).collect();
            assert_eq!(
                phases,
                vec![
                    ("prewarm", "runtime_resume"),
                    ("prewarm", "total"),
                    ("launch", "helper_start_to_runtime_ready"),
                ]
            );
            assert!(group.iter().all(|t| t.completion == *completion));
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn join_prewarm_reports_wait_only_when_a_prewarm_is_running() {
        let (bridge, root) = temp_bridge("join-wait");
        assert!(bridge.join_prewarm().await.is_none());
        let run = bridge.prewarm.begin().expect("prewarm starts");
        let (joined, ()) = tokio::join!(bridge.join_prewarm(), async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            drop(run);
        });
        let (started_at_ms, started) = joined.expect("a running prewarm is joined");
        bridge.push_lifecycle("connect", "prewarm_join_wait", started_at_ms, started, "completed");
        let traces = bridge.performance.lifecycle_snapshot(10);
        let last = traces.last().expect("join wait trace");
        assert_eq!((last.operation.as_str(), last.phase.as_str()), ("connect", "prewarm_join_wait"));
        assert!(last.total_us >= 15_000);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn skill_discovery_records_a_total_trace() {
        let (bridge, root) = temp_bridge("skill-discovery");
        let result = bridge.discover_external_skill_sources().await;
        let traces = bridge.performance.lifecycle_snapshot(10);
        assert_eq!(traces.len(), 1);
        assert_eq!(traces[0].operation, "skill_discovery");
        assert_eq!(traces[0].phase, "total");
        assert_eq!(traces[0].completion, if result.is_ok() { "completed" } else { "error" });
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn get_status_mascot_jobs_requires_boolean_opt_in() {
        for params in [Value::Null, json!({}), json!({"include_mascot_jobs": false}),
            json!({"include_mascot_jobs": "true"})] {
            assert!(!requested_mascot_jobs(&params));
        }
        assert!(requested_mascot_jobs(&json!({"include_mascot_jobs": true})));
    }

    #[test]
    fn mascot_jobs_switching_project_or_epoch_discards_observed_result() {
        let observed = || ScopedMascotJobs { project_path: "/project-a".into(), epoch: 7, jobs: vec![] };
        assert_eq!(observed().for_target(Some("/project-a"), 7), Some(vec![]));
        assert_eq!(observed().for_target(Some("/project-b"), 7), None);
        assert_eq!(observed().for_target(Some("/project-a"), 8), None);
        assert_eq!(observed().for_target(None, 7), None);
    }

    #[test]
    fn task_progress_hides_stale_completed_active_projection() {
        let value = json!({
            "status": "completed",
            "workspace": { "state": "active" },
            "recovery": { "available": true }
        });
        assert!(!Bridge::task_progress_is_presentable(&value));
    }

    #[test]
    fn task_progress_presents_only_fresh_terminal_results() {
        for status in ["completed", "failed", "failed_validation", "cancelled"] {
            assert!(Bridge::task_progress_is_presentable(&json!({
                "status": status, "finished_at_ms": now_ms()
            })));
            assert!(!Bridge::task_progress_is_presentable(&json!({
                "status": status, "finished_at_ms": now_ms().saturating_sub(9_000)
            })));
            assert!(!Bridge::task_progress_is_presentable(&json!({
                "status": status, "finished_at_ms": now_ms() + 10_000
            })));
        }
    }

    #[test]
    fn task_progress_keeps_active_and_preserved_recovery_states() {
        let active = json!({
            "status": "running",
            "workspace": { "state": "active" },
            "recovery": { "available": false }
        });
        let interrupted = json!({
            "status": "interrupted",
            "workspace": { "state": "preserved" },
            "recovery": { "available": true }
        });
        let blocked = json!({
            "status": "blocked",
            "workspace": { "state": "preserved" },
            "recovery": { "available": true }
        });
        assert!(Bridge::task_progress_is_presentable(&active));
        assert!(Bridge::task_progress_is_presentable(&interrupted));
        assert!(Bridge::task_progress_is_presentable(&blocked));
    }

    #[test]
    fn task_progress_requires_the_selected_project_source_path() {
        let value = json!({
            "source_path": "/tmp/project-a",
            "status": "running",
            "workspace": { "state": "active" },
            "recovery": { "available": false }
        });
        assert!(Bridge::task_progress_matches_path(&value, "/tmp/project-a"));
        assert!(!Bridge::task_progress_matches_path(
            &value,
            "/tmp/project-b"
        ));
        assert!(!Bridge::task_progress_matches_path(
            &json!({"status":"running"}),
            "/tmp/project-a"
        ));
    }

    #[tokio::test]
    async fn bounded_ndjson_reader_accepts_limit_and_rejects_oversized_frame() {
        let exact = vec![b'x'; 32];
        let mut framed = exact.clone();
        framed.push(b'\n');
        let mut reader = BufReader::new(framed.as_slice());
        assert_eq!(
            read_bounded_ndjson_line(&mut reader, 32).await.unwrap(),
            Some(String::from_utf8(exact).unwrap())
        );

        let mut oversized = vec![b'x'; 33];
        oversized.push(b'\n');
        let mut reader = BufReader::new(oversized.as_slice());
        let error = read_bounded_ndjson_line(&mut reader, 32).await.unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exceeds maximum size"));
    }

    #[tokio::test]
    async fn completed_request_tasks_are_reaped_and_inflight_is_bounded() {
        let mut tasks = JoinSet::new();
        for _ in 0..10_000 {
            reap_finished_request_tasks(&mut tasks);
            if tasks.len() >= MAX_IN_FLIGHT_REQUESTS {
                if let Some(result) = tasks.join_next().await {
                    log_request_task_result(result);
                }
            }
            tasks.spawn(async {});
            assert!(tasks.len() <= MAX_IN_FLIGHT_REQUESTS);
        }
        while let Some(result) = tasks.join_next().await {
            log_request_task_result(result);
        }
        assert_eq!(tasks.len(), 0);
    }

    #[test]
    fn json_response_result_keeps_direct_wire_shape() {
        let response = Response {
            protocol_version: PROTOCOL_VERSION,
            request_id: "instructions-wire-shape".to_string(),
            result: Some(ResponseResult::Json(json!({
                "status": "available",
                "sources": [{"path": "AGENTS.md"}]
            }))),
            error: None,
        };
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["result"]["status"], "available");
        assert_eq!(value["result"]["sources"][0]["path"], "AGENTS.md");
        assert!(value["result"].get("Json").is_none());
    }

    #[test]
    fn response_result_keeps_existing_wire_shape_without_intermediate_value() {
        let snapshot = BackendSnapshot {
            phase: ConnectionPhase::Stopped,
            graphify: GraphifyStatus::unavailable(),
            selected_project: None,
            tunnel_ready: false,
            chat_gpt_connected: false,
            chat_gpt_verified_for_selected_project: false,
            last_verified_at_ms: None,
            current_operation: None,
            task_progress: None,
            mascot_jobs: None,
            runtime_status: None,
            tunnel_status: None,
            error: None,
            activity_sequence: 3,
            state_revision: 7,
        };
        let response = Response {
            protocol_version: PROTOCOL_VERSION,
            request_id: "wire-shape".to_string(),
            result: Some(ResponseResult::Snapshot(snapshot)),
            error: None,
        };
        let value = serde_json::to_value(response).unwrap();
        assert_eq!(value["result"]["phase"], "stopped");
        assert_eq!(value["result"]["activity_sequence"], 3);
        assert_eq!(value["result"]["state_revision"], 7);
        assert!(value["result"].get("Snapshot").is_none());
        let legacy: BackendSnapshot = serde_json::from_value(value["result"].clone()).unwrap();
        assert_eq!(legacy.runtime_status, None);
        assert_eq!(legacy.tunnel_status, None);
    }

    #[test]
    fn snapshot_revision_changes_only_when_semantic_state_changes() {
        let mut state = SnapshotRevisionState::default();
        let base = BackendSnapshot {
            phase: ConnectionPhase::Stopped,
            graphify: GraphifyStatus::unavailable(),
            selected_project: None,
            tunnel_ready: false,
            chat_gpt_connected: false,
            chat_gpt_verified_for_selected_project: false,
            last_verified_at_ms: None,
            current_operation: None,
            task_progress: None,
            mascot_jobs: None,
            runtime_status: Some(RuntimeStatus {
                runtime_configured: true,
                runtime_ready: false,
                needs_attention: true,
                summary: "Runtime needs attention".to_string(),
                next_action: Some("Start the Server".to_string()),
                summary_kind: "runtime_needs_attention".to_string(),
                server: "stopped".to_string(),
                runner: "stopped".to_string(),
                exposure: "local_ready".to_string(),
                project: "ready".to_string(),
            }),
            tunnel_status: None,
            error: None,
            activity_sequence: 0,
            state_revision: 0,
        };
        let first = state.assign_revision(base.clone());
        let repeated = state.assign_revision(base.clone());
        let mut changed = base;
        let readiness = changed.runtime_status.as_mut().unwrap();
        readiness.runtime_ready = true;
        readiness.needs_attention = false;
        readiness.summary = "Runtime ready".to_string();
        readiness.next_action = None;
        readiness.summary_kind = "runtime_ready".to_string();
        readiness.server = "ready".to_string();
        readiness.runner = "ready".to_string();
        let changed = state.assign_revision(changed);

        assert_eq!(first.state_revision, 1);
        assert_eq!(repeated.state_revision, 1);
        assert_eq!(changed.state_revision, 2);
        let mut jobs_changed = changed.clone();
        jobs_changed.mascot_jobs = Some(vec![]);
        let empty = state.assign_revision(jobs_changed.clone());
        assert_eq!(empty.state_revision, 3);
        assert_eq!(serde_json::to_value(&empty).unwrap()["mascot_jobs"], json!([]));
        jobs_changed.mascot_jobs = Some(vec![RuntimeMascotJob {
            job_id: "job_active".into(), status: "running".into(),
            started_at_ms: Some(1000), finished_at_ms: None, exit_code: None,
        }]);
        let active = state.assign_revision(jobs_changed.clone());
        assert_eq!(active.state_revision, 4);
        jobs_changed.mascot_jobs = None;
        let unknown = state.assign_revision(jobs_changed);
        assert_eq!(unknown.state_revision, 5);
        assert!(serde_json::to_value(&unknown).unwrap()["mascot_jobs"].is_null());
        // Keep the pre-W3 tunnel/phase revision coverage as well as the new
        // readiness projection. Additive fields cannot replace existing gates.
        let mut tunnel_changed = unknown;
        tunnel_changed.tunnel_ready = true;
        tunnel_changed.phase = ConnectionPhase::WaitingForChatGptVerification;
        let connected = state.assign_revision(tunnel_changed.clone());
        assert_eq!(connected.state_revision, 6);
        assert_eq!(state.assign_revision(tunnel_changed.clone()).state_revision, 6);
        tunnel_changed.tunnel_status = Some(TunnelStatus { configured: true, state: TunnelState::Ready });
        assert_eq!(state.assign_revision(tunnel_changed).state_revision, 7);
    }

    #[tokio::test]
    async fn project_operation_lock_wait_is_bounded() {
        let lock = Mutex::new(());
        let held = lock.lock().await;
        let started = std::time::Instant::now();
        let error = lock_project_operation(&lock, std::time::Duration::from_millis(30))
            .await
            .expect_err("a held lock must not be waited for forever");
        assert_eq!(error.code, "project_switch_busy");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
        drop(held);
        assert!(lock_project_operation(&lock, std::time::Duration::from_millis(30)).await.is_ok());
    }

    #[tokio::test]
    async fn bounded_activation_cancels_a_slow_activation_and_waits_for_it_to_unwind() {
        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let unwound = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let activation = {
            let cancelled = Arc::clone(&cancelled);
            let unwound = Arc::clone(&unwound);
            async move {
                // Like the coordinator: runs until cancelled, then unwinds.
                while !cancelled.load(std::sync::atomic::Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                }
                unwound.store(true, std::sync::atomic::Ordering::SeqCst);
                Err::<RuntimeSnapshot, _>(ChadexError::new(
                    "desktop_operation_cancelled",
                    "cancelled",
                    "retry",
                ))
            }
        };
        let started = std::time::Instant::now();
        let error = bounded_activation(activation, std::time::Duration::from_millis(30), || {
            cancelled.store(true, std::sync::atomic::Ordering::SeqCst);
        })
        .await
        .expect_err("a cancelled activation reports the timeout");
        assert_eq!(error.code, "project_activation_timed_out");
        assert!(unwound.load(std::sync::atomic::Ordering::SeqCst), "the activation is awaited, never dropped");
        assert!(started.elapsed() < std::time::Duration::from_secs(2));
    }

    #[tokio::test]
    async fn bounded_activation_keeps_fast_results_and_errors() {
        let cancel_called = std::sync::atomic::AtomicBool::new(false);
        let ok = bounded_activation(
            async { Ok::<_, ChadexError>(prewarm_runtime_snapshot()) },
            std::time::Duration::from_secs(5),
            || cancel_called.store(true, std::sync::atomic::Ordering::SeqCst),
        )
        .await
        .expect("a fast activation succeeds");
        assert_eq!(ok, prewarm_runtime_snapshot());
        let error = bounded_activation(
            async {
                Err::<RuntimeSnapshot, _>(ChadexError::new("project_not_readable", "unreadable", "choose another"))
            },
            std::time::Duration::from_secs(5),
            || cancel_called.store(true, std::sync::atomic::Ordering::SeqCst),
        )
        .await
        .expect_err("activation errors pass through");
        assert_eq!(error.code, "project_not_readable", "a real failure keeps its own reason");
        assert!(!cancel_called.load(std::sync::atomic::Ordering::SeqCst));
    }
}

#[cfg(test)]
mod computer_overlay_bridge_tests {
    use super::*;
    use std::time::Duration;

    fn request(method: &str, params: Value) -> Request {
        Request {
            protocol_version: PROTOCOL_VERSION,
            request_id: "overlay-test".to_string(),
            method: method.to_string(),
            params,
        }
    }

    fn temp_bridge(tag: &str) -> (Arc<Bridge>, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "chadex-overlay-{tag}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let bridge = Bridge::with_dirs(root.join("data"), root.join("resources")).unwrap();
        (Arc::new(bridge), root)
    }

    async fn call(bridge: &Arc<Bridge>, method: &str, params: Value) -> Value {
        let response = handle_request(Arc::clone(bridge), request(method, params)).await;
        serde_json::to_value(&response).unwrap()
    }

    #[tokio::test]
    async fn set_overlay_events_requires_a_boolean() {
        let (bridge, root) = temp_bridge("params");
        for params in [
            json!({}),
            json!({"enabled": "true"}),
            json!({"enabled": 1}),
            json!({"enabled": null}),
            json!(null),
        ] {
            let response = call(&bridge, "setComputerOverlayEvents", params.clone()).await;
            assert_eq!(response["error"]["code"], "invalid_params", "{params}");
            assert!(!bridge.overlay.is_enabled(), "{params}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn set_overlay_events_toggles_and_reports_channel_state() {
        let (bridge, root) = temp_bridge("toggle");
        let mut events = bridge.take_overlay_events().unwrap();
        assert!(bridge.take_overlay_events().is_none(), "the receiver is single-owner");

        let on = call(&bridge, "setComputerOverlayEvents", json!({"enabled": true})).await;
        assert_eq!(on["result"]["enabled"], true);
        let expected_channel = if cfg!(target_os = "macos") { "detached" } else { "unsupported" };
        assert_eq!(on["result"]["runner_channel"], expected_channel);
        assert_eq!(on["result"]["counters"]["forwarded"], 0);
        assert!(events.try_recv().is_err(), "enabling alone sends no frame");

        let off = call(&bridge, "setComputerOverlayEvents", json!({"enabled": false})).await;
        assert_eq!(off["result"]["enabled"], false);
        let frame = events.try_recv().expect("clear(disabled)");
        assert_eq!(frame["phase"], "clear");
        assert_eq!(frame["reason"], "disabled");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn get_overlay_status_is_read_only_and_numbers_only() {
        let (bridge, root) = temp_bridge("status");
        let mut events = bridge.take_overlay_events().unwrap();

        let off = call(&bridge, "getComputerOverlayStatus", json!({})).await;
        assert_eq!(off["result"]["enabled"], false);
        assert_eq!(off["result"]["counters"]["forwarded"], 0);

        let _ = call(&bridge, "setComputerOverlayEvents", json!({"enabled": true})).await;
        let on = call(&bridge, "getComputerOverlayStatus", json!({"enabled": false})).await;
        assert_eq!(on["result"]["enabled"], true, "params are ignored; the query never toggles");
        let expected_channel = if cfg!(target_os = "macos") { "detached" } else { "unsupported" };
        assert_eq!(on["result"]["runner_channel"], expected_channel);
        assert!(events.try_recv().is_err(), "querying sends no frame");

        let mut keys: Vec<_> = on["result"].as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["counters", "enabled", "runner_channel"]);
        for value in on["result"]["counters"].as_object().unwrap().values() {
            assert!(value.is_u64(), "counters are plain numbers: {value}");
        }
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn no_event_frames_exist_unless_the_app_enabled_them() {
        let (bridge, root) = temp_bridge("silent");
        let mut events = bridge.take_overlay_events().unwrap();
        // Kill-switch style calls while disabled must not produce frames either.
        let _ = call(&bridge, "stopComputerControl", json!({})).await;
        let _ = call(&bridge, "setComputerControlMode", json!({"mode": "read_only"})).await;
        assert!(events.try_recv().is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn stop_and_read_only_clear_the_overlay_and_resume_does_not() {
        let (bridge, root) = temp_bridge("killswitch");
        let mut events = bridge.take_overlay_events().unwrap();
        let _ = call(&bridge, "setComputerOverlayEvents", json!({"enabled": true})).await;

        let _ = call(&bridge, "stopComputerControl", json!({})).await;
        let frame = events.try_recv().expect("clear(stopped) after Stop");
        assert_eq!(frame, json!({"v": 1, "phase": "clear", "reason": "stopped"}));

        let _ = call(&bridge, "setComputerControlMode", json!({"mode": "read_only"})).await;
        assert_eq!(events.try_recv().expect("clear after read_only")["reason"], "stopped");

        let _ = call(&bridge, "setComputerControlMode", json!({"mode": "ask_before_control"})).await;
        let _ = call(&bridge, "resumeComputerControl", json!({})).await;
        assert!(events.try_recv().is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn an_unread_overlay_channel_never_slows_other_requests() {
        let (bridge, root) = temp_bridge("ingress");
        // Nobody drains `overlay_events`, like an App that stopped reading stdout.
        let _ = call(&bridge, "setComputerOverlayEvents", json!({"enabled": true})).await;
        for _ in 0..500 {
            bridge.overlay.clear(computer_overlay::ClearReason::Stopped);
        }
        for method in ["getComputerSafety", "getStatus"] {
            let response = tokio::time::timeout(
                Duration::from_secs(5),
                call(&bridge, method, json!({})),
            )
            .await
            .unwrap_or_else(|_| panic!("{method} must answer while overlay frames are backed up"));
            assert!(response.get("result").is_some(), "{method}: {response}");
        }
        let counters = bridge.overlay.counters();
        assert!(counters.dropped_backpressure >= 400, "{counters:?}");
        assert!(counters.forwarded <= computer_overlay::OUT_CAPACITY as u64);
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn event_writer_emits_one_ndjson_frame_per_event_and_stops_with_the_channel() {
        let (client, server) = tokio::io::duplex(4096);
        let (tx, rx) = mpsc::channel(8);
        let writer = tokio::spawn(write_overlay_events(Arc::new(Mutex::new(server)), rx));
        tx.send(json!({"v": 1, "phase": "clear", "reason": "stopped"})).await.unwrap();
        tx.send(json!({"v": 1, "phase": "finished", "seq": 2})).await.unwrap();
        drop(tx);
        writer.await.unwrap();

        let mut lines = BufReader::new(client).lines();
        let first: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(first["protocol_version"], 1);
        assert_eq!(first["event"], "computer_overlay");
        assert!(first.get("request_id").is_none(), "event frames carry no request_id");
        assert_eq!(first["data"]["reason"], "stopped");
        let second: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
        assert_eq!(second["data"]["phase"], "finished");
        assert!(lines.next_line().await.unwrap().is_none());
    }
}
