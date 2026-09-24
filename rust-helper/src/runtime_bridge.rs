use crate::chadex_core::activity::{sanitize_message, RuntimeActivityEntry};
use crate::chadex_core::graphify::GraphifyStatus;
use crate::chadex_core::performance::{
    duration_us, now_ms, LifecyclePerformanceTrace, McpPerformanceTrace, PerformanceTraceStore,
};
use crate::chadex_core::runtime::{
    ChadexRuntimeCore, RuntimeProject, RuntimeProxyMode, RuntimeSnapshot,
};
use crate::chadex_core::tunnel::{RuntimeTunnelTarget, TunnelManager, TunnelState};
use crate::chadex_core::verification::VerificationTracker;
use crate::chadex_core::ChadexError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::time::Instant;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;
use tokio::task::JoinSet;
use zeroize::Zeroizing;

const PROTOCOL_VERSION: u32 = 1;
const ACTIVITY_QUERY_MAX: usize = 200;
const PERFORMANCE_QUERY_MAX: usize = 100;
const TASK_PROGRESS_MAX_BYTES: u64 = 64 * 1024;
const MAX_IN_FLIGHT_REQUESTS: usize = 64;
const SHUTDOWN_REQUEST_DRAIN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
const MAX_REQUEST_FRAME_BYTES: usize = 256 * 1024;

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
    error: Option<ErrorPayload>,
    activity_sequence: u64,
    state_revision: u64,
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

struct Bridge {
    runtime: ChadexRuntimeCore,
    graphify: GraphifyStatus,
    tunnel: Arc<TunnelManager>,
    verification: Arc<VerificationTracker>,
    performance: Arc<PerformanceTraceStore>,
    target_project: RwLock<Option<ProjectInspection>>,
    task_state_dir: PathBuf,
    snapshot_state: StdMutex<SnapshotRevisionState>,
    project_switch_lifecycle: Mutex<()>,
}

impl Bridge {
    fn new() -> Result<Self, ErrorPayload> {
        let data_dir = chadex_data_dir()?;
        fs::create_dir_all(&data_dir).map_err(|error| {
            ErrorPayload::new(
                "data_directory_unavailable",
                "Chadex could not prepare its private data directory",
                "Check Application Support permissions and retry.",
            )
            .with_details(json!({ "io_kind": format!("{:?}", error.kind()) }))
        })?;
        let resource_dir = chadex_resource_dir()?;
        let task_state_dir = data_dir.join("phase9-tasks");
        let runtime =
            ChadexRuntimeCore::new(data_dir.clone(), resource_dir).map_err(ErrorPayload::from)?;
        let graphify = GraphifyStatus::detect();
        let verification = Arc::new(VerificationTracker::default());
        let performance = Arc::new(PerformanceTraceStore::default());
        let tunnel = Arc::new(TunnelManager::new(
            data_dir,
            Arc::clone(&verification),
            Arc::clone(&performance),
        ));
        Ok(Self {
            runtime,
            graphify,
            tunnel,
            verification,
            performance,
            target_project: RwLock::new(None),
            task_state_dir,
            snapshot_state: StdMutex::new(SnapshotRevisionState::default()),
            project_switch_lifecycle: Mutex::new(()),
        })
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
        active || recoverable
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
        let _switch = self.project_switch_lifecycle.lock().await;
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
                let activated = self
                    .runtime
                    .activate_local_project(path)
                    .await
                    .map_err(ErrorPayload::from)?;
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

    async fn refreshed_snapshot(&self) -> BackendSnapshot {
        self.tunnel.refresh().await;
        self.snapshot()
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
        let tunnel = self.tunnel.snapshot();
        let verification = self.verification.snapshot();
        let target = self
            .target_project()
            .or_else(|| desktop.project.clone().map(project_inspection));
        let target_matches_runtime = match (&target, &desktop.project) {
            (Some(target), Some(runtime)) => target.path == runtime.path,
            _ => false,
        };
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
        let current_operation = desktop
            .current_operation
            .as_ref()
            .map(|operation| OperationSnapshot {
                id: operation.id.clone(),
                kind: operation.kind.to_string(),
                phase: operation.phase.as_str().to_string(),
                started_at_ms: operation.started_at_ms,
                cancellable: operation.cancellable,
            })
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
    let stdout = Arc::new(Mutex::new(tokio::io::stdout()));
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
    bridge.tunnel.shutdown().await;
    bridge.runtime.shutdown().await;
    Ok(())
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
    let result: Result<ResponseResult, ErrorPayload> = match request.method.as_str() {
        "getStatus" => Ok(ResponseResult::Snapshot(bridge.refreshed_snapshot().await)),
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
        "activateProject" => match param_str(&request.params, "path") {
            Ok(path) => bridge
                .choose_target_project(path)
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
        "configureLocalSetup" | "resumeService" => bridge
            .ensure_runtime_for_target()
            .await
            .map(ResponseResult::Snapshot),
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

fn param_str<'a>(params: &'a Value, key: &str) -> Result<&'a str, ErrorPayload> {
    params.get(key).and_then(Value::as_str).ok_or_else(|| {
        ErrorPayload::new(
            "invalid_params",
            format!("Missing required parameter: {key}"),
            "Update the app and helper together, then retry.",
        )
    })
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
    if let Some(path) = std::env::var_os("CHADEX_DATA_DIR") {
        let path = PathBuf::from(path);
        if !path.as_os_str().is_empty() {
            return Ok(path);
        }
    }
    let home = std::env::var_os("HOME").ok_or_else(|| {
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
    fn secret_parameter_is_removed_from_request_object() {
        let mut params = json!({ "tunnel_id": "tunnel_one", "api_key": "secret-value" });
        let secret = Zeroizing::new(take_param_string(&mut params, "api_key").unwrap());
        assert_eq!(secret.as_str(), "secret-value");
        assert!(!serde_json::to_string(&params)
            .unwrap()
            .contains("secret-value"));
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
            error: None,
            activity_sequence: 0,
            state_revision: 42,
        };
        let value = serde_json::to_value(snapshot).unwrap();
        assert_eq!(value["phase"], "waiting_for_chatgpt_verification");
        assert_eq!(value["chat_gpt_connected"], true);
        assert_eq!(value["chat_gpt_verified_for_selected_project"], false);
        assert_eq!(value["state_revision"], 42);
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
            error: None,
            activity_sequence: 0,
            state_revision: 0,
        };
        let first = state.assign_revision(base.clone());
        let repeated = state.assign_revision(base.clone());
        let mut changed = base;
        changed.tunnel_ready = true;
        changed.phase = ConnectionPhase::WaitingForChatGptVerification;
        let changed = state.assign_revision(changed);

        assert_eq!(first.state_revision, 1);
        assert_eq!(repeated.state_revision, 1);
        assert_eq!(changed.state_revision, 2);
    }
}
