use crate::chadex_core::runtime_compat::activity::{ActivityEventKind, ActivityLevel, ActivityLog};
use crate::chadex_core::runtime_compat::deadline::Deadline;
use crate::chadex_core::runtime_compat::error::{DesktopError, DesktopResult};
use crate::chadex_core::runtime_compat::models::{
    aggregate_readiness, ChatGptActivitySnapshot, DesktopOperationKind, DesktopStateSnapshot,
    Enrollment, Experience, Exposure, ExposureReadiness, ProjectReadiness, ProjectSelection,
    QuickShareState, ReadinessNextActionKind, ReadinessSummaryKind, RegularConnectionPreference,
    RegularTunnelState, RegularTunnelStatus, RunnerReadiness, RunnerTopology, RuntimeTopology,
    ServerReadiness, ServerTopology, StoredDesktopConfig, StoredRuntime, TunnelProxyConfig,
    TunnelProxyMode, TunnelProxySnapshot,
};
use crate::chadex_core::runtime_compat::operation::{
    cancelled_error, CancellationContext, CancellationSignal, OperationAdmission,
    OperationController,
};
use crate::chadex_core::runtime_compat::process::{MachineEventReceiver, ProcessKind, ProcessPhase, ProcessSupervisor};
use crate::chadex_core::runtime_compat::integration::{
    inspect_project_path, ProjectRuntimeIdentity, QuickShareReadyEvent, RegularTunnelReadyEvent,
    RuntimeIntegrationBridge,
};
use crate::chadex_core::runtime_compat::tunnel_config::{TunnelConfig, TunnelConfigRequest};
use serde_json::Value;
#[cfg(unix)]
use std::fs::File;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::Mutex;

use super::policy::*;
use super::settings::*;
use super::store::*;

type SharedSupervisor = Arc<Mutex<ProcessSupervisor>>;

#[derive(Debug, Clone)]
struct ChatGptActivityProbe {
    identity: ProjectRuntimeIdentity,
    cli: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ChadexRuntimeTunnelTarget {
    pub server_url: String,
    pub server_env_file: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChadexRuntimeProbeTarget {
    pub server_url: String,
    pub user_token_file: PathBuf,
    pub runner_client_id: String,
    pub runtime_project_id: String,
    pub project_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChadexProjectActivationTarget {
    pub project: ProjectSelection,
    pub server_url: String,
    pub user_token_file: PathBuf,
    pub runner_config: PathBuf,
    pub runner_client_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChadexProjectActivationObservation {
    pub project_id: String,
    pub runtime_project_id: String,
    pub project_path: String,
}

pub struct RuntimeStateManager {
    core: Mutex<Option<RuntimeCoordinator>>,
    published: Arc<RwLock<DesktopStateSnapshot>>,
    supervisor: SharedSupervisor,
    activity: ActivityLog,
    operations: OperationController,
    shutdown_signal: CancellationSignal,
    shutdown_started: AtomicBool,
}

impl RuntimeStateManager {
    pub fn new(data_dir: PathBuf, resource_dir: PathBuf) -> DesktopResult<Self> {
        let core = RuntimeCoordinator::new(data_dir, resource_dir)?;
        Self::from_core(core)
    }

    /// Temporary Chadex migration boundary. WebCodex remains only as the local
    /// MCP/file backend; its TunnelConfig is deliberately empty and inert.
    /// Chadex never provides Tunnel ID or API key to this RuntimeStateManager.
    pub fn new_for_chadex_backend(data_dir: PathBuf, resource_dir: PathBuf) -> DesktopResult<Self> {
        let core = RuntimeCoordinator::new_with_tunnel_config(
            data_dir,
            resource_dir,
            TunnelConfig::in_memory(),
        )?;
        Self::from_core(core)
    }

    fn from_core(core: RuntimeCoordinator) -> DesktopResult<Self> {
        let published = Arc::clone(&core.published);
        let supervisor = Arc::clone(&core.supervisor);
        let activity = core.activity.clone();
        Ok(Self {
            core: Mutex::new(Some(core)),
            published,
            supervisor,
            operations: OperationController::new(activity.clone()),
            activity,
            shutdown_signal: CancellationSignal::new(),
            shutdown_started: AtomicBool::new(false),
        })
    }

    pub fn get_state(&self) -> DesktopStateSnapshot {
        let mut snapshot = self
            .published
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if snapshot.regular_tunnel.is_some() {
            if let Ok(mut supervisor) = self.supervisor.try_lock() {
                let active =
                    supervisor
                        .snapshot(ProcessKind::RegularTunnel)
                        .is_some_and(|process| {
                            matches!(
                                process.phase,
                                ProcessPhase::Starting | ProcessPhase::Running
                            )
                        });
                if let Some(exposure) =
                    regular_tunnel_exposure(&mut snapshot.regular_tunnel, active)
                {
                    snapshot.readiness = aggregate_readiness(
                        snapshot.readiness.server.clone(),
                        snapshot.readiness.runner.clone(),
                        exposure.clone(),
                        snapshot.readiness.project.clone(),
                    );
                    apply_regular_tunnel_next_action(&mut snapshot, &exposure);
                }
            }
        }
        snapshot.current_operation = self.operations.current();
        snapshot.activity_sequence = self.activity.latest_sequence();
        if snapshot.openai_tunnel_config.source == crate::chadex_core::runtime_compat::models::TunnelConfigSource::Environment {
            snapshot.openai_tunnel_config = crate::chadex_core::runtime_compat::tunnel_config::environment_snapshot();
            snapshot.openai_tunnel_configured = snapshot.openai_tunnel_config.is_configured();
        }
        snapshot.regular_tunnel_available = true;
        snapshot.powershell_runtime = crate::chadex_core::runtime_compat::platform::powershell_runtime_snapshot();
        snapshot
    }

    pub fn activity(&self) -> Vec<crate::chadex_core::runtime_compat::activity::ActivityEntry> {
        self.activity.snapshot()
    }

    /// Temporary migration boundary for Chadex. The local MCP/file backend still
    /// comes from the vendored runtime, but Tunnel credentials and lifecycle do
    /// not. This method projects only the non-secret local endpoint and the path
    /// to the private server environment file. Chadex reads the bootstrap token
    /// inside Rust and never returns it over the helper protocol.
    pub async fn chadex_runtime_tunnel_target(&self) -> DesktopResult<ChadexRuntimeTunnelTarget> {
        let snapshot = self.get_state();
        if !snapshot.readiness.runtime_ready {
            return Err(DesktopError::new(
                "runtime_not_ready",
                "The local MCP runtime is not ready",
                "Restore the local service before starting the Secure MCP Tunnel.",
            ));
        }
        let slot = self.core.lock().await;
        let core = slot.as_ref().ok_or_else(|| {
            DesktopError::new(
                "runtime_not_ready",
                "The local MCP runtime state is unavailable",
                "Wait for the current operation to finish, then retry.",
            )
        })?;
        let runtime = core.config.runtime.as_ref().ok_or_else(|| {
            DesktopError::new(
                "runtime_not_ready",
                "The local MCP runtime identity is incomplete",
                "Run local setup again before starting the Secure MCP Tunnel.",
            )
        })?;
        let server_env_file = runtime
            .server_env_file
            .as_ref()
            .filter(|path| path.is_file())
            .cloned()
            .ok_or_else(|| {
                DesktopError::new(
                    "server_unavailable",
                    "The local MCP server configuration is unavailable",
                    "Restart the local service before starting the Secure MCP Tunnel.",
                )
            })?;
        Ok(ChadexRuntimeTunnelTarget {
            server_url: runtime.server_url.clone(),
            server_env_file,
        })
    }

    /// Chadex-owned runtime observation fast path. This exports only the exact
    /// saved identity required to ask the already-running local Server whether
    /// the configured Runner and Project are still live. It does not grant new
    /// authority and carries only a token-file path, never token contents.
    pub async fn chadex_runtime_probe_target(
        &self,
    ) -> DesktopResult<Option<ChadexRuntimeProbeTarget>> {
        if self.shutdown_signal.is_cancelled() || self.operations.current().is_some() {
            return Ok(None);
        }
        let slot = self.core.lock().await;
        let Some(core) = slot.as_ref() else {
            return Ok(None);
        };
        let local_full = core.config.topology.as_ref().is_some_and(|topology| {
            topology.experience == Experience::Full
                && matches!(topology.server, ServerTopology::Local)
        });
        if !local_full || !runtime_autostart(&core.config) {
            return Ok(None);
        }
        let Some(identity) = identity_from_config(&core.config) else {
            return Ok(None);
        };
        let Some(runner_client_id) = stored_runner_client_id(&core.config) else {
            return Ok(None);
        };
        Ok(Some(ChadexRuntimeProbeTarget {
            server_url: identity.server_url,
            user_token_file: identity.user_token_file,
            runner_client_id,
            runtime_project_id: identity.runtime_project_id,
            project_path: identity.project_path,
        }))
    }

    /// Publish a previously authenticated exact-project observation only if the
    /// saved runtime identity is still byte-for-byte the one that was probed.
    /// Any concurrent project/runtime change fails closed so Chadex can use the
    /// ordinary Desktop orchestration fallback instead.
    pub async fn chadex_apply_runtime_probe(
        &self,
        target: &ChadexRuntimeProbeTarget,
    ) -> DesktopResult<DesktopStateSnapshot> {
        if self.shutdown_signal.is_cancelled() || self.operations.current().is_some() {
            return Err(DesktopError::new(
                "runtime_probe_stale",
                "The local runtime changed while Chadex was checking readiness",
                "Retry the runtime refresh.",
            ));
        }
        let mut slot = self.core.lock().await;
        let core = slot.as_mut().ok_or_else(|| {
            DesktopError::new(
                "runtime_not_ready",
                "The local MCP runtime state is unavailable",
                "Retry the runtime refresh.",
            )
        })?;
        let identity = identity_from_config(&core.config).ok_or_else(|| {
            DesktopError::new(
                "runtime_probe_stale",
                "The saved local runtime identity is incomplete",
                "Retry the runtime refresh.",
            )
        })?;
        let runner_client_id = stored_runner_client_id(&core.config).ok_or_else(|| {
            DesktopError::new(
                "runtime_probe_stale",
                "The saved Runner identity is incomplete",
                "Retry the runtime refresh.",
            )
        })?;
        if identity.server_url != target.server_url
            || identity.user_token_file != target.user_token_file
            || runner_client_id != target.runner_client_id
            || identity.runtime_project_id != target.runtime_project_id
            || identity.project_path != target.project_path
        {
            return Err(DesktopError::new(
                "runtime_probe_stale",
                "The local runtime identity changed while Chadex was checking readiness",
                "Retry the runtime refresh.",
            ));
        }
        core.snapshot.chatgpt_activity = None;
        core.snapshot.topology = core.config.topology.clone();
        core.snapshot.project = project_snapshot(&core.config);
        core.snapshot.readiness = aggregate_readiness(
            ServerReadiness::Ready,
            RunnerReadiness::Ready,
            exposure_readiness(core.config.topology.as_ref()),
            ProjectReadiness::Ready,
        );
        Ok(core.publish_snapshot())
    }

    pub async fn inspect_project(&self, path: &str) -> DesktopResult<ProjectSelection> {
        inspect_project_path(path).await
    }

    pub async fn refresh_runtime_status(&self) -> DesktopResult<DesktopStateSnapshot> {
        let (operation, cancellation, mut core, baseline) = self
            .begin_operation(DesktopOperationKind::RuntimeRefresh, true)
            .await?;
        let result = core.refresh_runtime_status(&cancellation).await;
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }

    pub async fn observe_chatgpt_activity(&self) -> DesktopResult<DesktopStateSnapshot> {
        if self.shutdown_signal.is_cancelled() {
            return Err(cancelled_error());
        }
        if self.operations.current().is_some() {
            return Ok(self.get_state());
        }
        let cancellation =
            CancellationContext::new(CancellationSignal::new(), self.shutdown_signal.clone());
        let probe = {
            let slot = self.core.lock().await;
            let Some(core) = slot.as_ref() else {
                return Ok(self.get_state());
            };
            let Some(probe) = core.chatgpt_activity_probe() else {
                return Ok(self.get_state());
            };
            probe
        };
        let observation = RuntimeIntegrationBridge::chatgpt_activity_with_binary(
            &probe.cli,
            &probe.identity,
            &cancellation,
        )
        .await;
        cancellation.check()?;
        let Ok(last_meaningful_activity_at_ms) = observation else {
            return Ok(self.get_state());
        };
        if self.operations.current().is_some() {
            return Ok(self.get_state());
        }
        let mut slot = self.core.lock().await;
        let Some(core) = slot.as_mut() else {
            return Ok(self.get_state());
        };
        core.apply_chatgpt_activity_observation(&probe.identity, last_meaningful_activity_at_ms)
    }

    pub async fn resume_saved_runtime(&self) -> DesktopResult<DesktopStateSnapshot> {
        let (operation, cancellation, mut core, baseline) = self
            .begin_operation(DesktopOperationKind::RuntimeResume, true)
            .await?;
        let result = core.resume_saved_runtime(&cancellation).await;
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }

    pub async fn update_tunnel_config(
        &self,
        request: TunnelConfigRequest,
    ) -> DesktopResult<DesktopStateSnapshot> {
        let (operation, cancellation, mut core, baseline) = self
            .begin_operation(DesktopOperationKind::TunnelConfigUpdate, false)
            .await?;
        let result = async {
            cancellation.check()?;
            let path = core.data_dir.join("secrets").join("tunnel-config.json");
            let mut config = core.tunnel_config.clone();
            core.tunnel_config = tokio::task::spawn_blocking(move || {
                config.update(&path, request)?;
                Ok::<_, DesktopError>(config)
            })
            .await
            .map_err(|_| {
                DesktopError::new(
                    "tunnel_config_save_failed",
                    "Could not complete the configuration save",
                    "Check the saved configuration before retrying.",
                )
            })??;
            core.get_state().await
        }
        .await;
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }

    pub async fn update_tunnel_proxy(
        &self,
        mode: TunnelProxyMode,
        custom_url: Option<&str>,
    ) -> DesktopResult<DesktopStateSnapshot> {
        let (operation, cancellation, mut core, baseline) = self
            .begin_operation(DesktopOperationKind::TunnelProxyUpdate, false)
            .await?;
        let result = core
            .update_tunnel_proxy(mode, custom_url, &cancellation)
            .await;
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }

    pub async fn configure_local_setup(
        &self,
        project_path: Option<&str>,
    ) -> DesktopResult<DesktopStateSnapshot> {
        let (operation, cancellation, mut core, baseline) = self
            .begin_operation(DesktopOperationKind::LocalSetup, true)
            .await?;
        let result = core
            .configure_local_setup(project_path, &cancellation)
            .await;
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }

    pub(crate) async fn configure_local_setup_with_chadex_fast_path<F, Fut>(
        &self,
        project_path: Option<&str>,
        fast_path: F,
    ) -> DesktopResult<DesktopStateSnapshot>
    where
        F: FnMut(ChadexProjectActivationTarget, CancellationContext) -> Fut,
        Fut: Future<Output = DesktopResult<Option<ChadexProjectActivationObservation>>>,
    {
        let (operation, cancellation, mut core, baseline) = self
            .begin_operation(DesktopOperationKind::LocalSetup, true)
            .await?;
        let result = core
            .configure_local_setup_with_chadex_fast_path(project_path, &cancellation, fast_path)
            .await;
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }

    pub async fn activate_local_project(
        &self,
        project_path: &str,
    ) -> DesktopResult<DesktopStateSnapshot> {
        let (operation, cancellation, mut core, baseline) = self
            .begin_operation(DesktopOperationKind::LocalProjectActivate, true)
            .await?;
        let result = core
            .activate_local_project(project_path, &cancellation)
            .await;
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }

    pub(crate) async fn activate_local_project_with_chadex_fast_path<F, Fut>(
        &self,
        project_path: &str,
        fast_path: F,
    ) -> DesktopResult<DesktopStateSnapshot>
    where
        F: FnOnce(ChadexProjectActivationTarget, CancellationContext) -> Fut,
        Fut: Future<Output = DesktopResult<Option<ChadexProjectActivationObservation>>>,
    {
        let (operation, cancellation, mut core, baseline) = self
            .begin_operation(DesktopOperationKind::LocalProjectActivate, true)
            .await?;
        let result = async {
            let target = core
                .chadex_project_activation_target(project_path, &cancellation)
                .await?;
            if let Some(observation) = fast_path(target.clone(), cancellation.clone()).await? {
                core.apply_chadex_project_activation(target, observation, &cancellation)
                    .await
            } else {
                core.activate_local_project(project_path, &cancellation)
                    .await
            }
        }
        .await;
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }

    pub async fn configure_remote_setup(
        &self,
        server_url: &str,
        pairing_code: &str,
        project_path: &str,
    ) -> DesktopResult<DesktopStateSnapshot> {
        let (operation, cancellation, mut core, baseline) = self
            .begin_operation(DesktopOperationKind::RemoteSetup, true)
            .await?;
        let result = core
            .configure_remote_setup(server_url, pairing_code, project_path, &cancellation)
            .await;
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }

    pub async fn start_quick_share(
        &self,
        project_path: &str,
        provider: &str,
    ) -> DesktopResult<DesktopStateSnapshot> {
        let (operation, cancellation, mut core, baseline) = self
            .begin_operation(DesktopOperationKind::QuickShareStart, true)
            .await?;
        let result = core
            .start_quick_share(project_path, provider, &cancellation)
            .await;
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }

    pub async fn stop_quick_share(&self) -> DesktopResult<DesktopStateSnapshot> {
        let (operation, cancellation, mut core, baseline) = self
            .begin_operation(DesktopOperationKind::QuickShareStop, false)
            .await?;
        let result = core.stop_quick_share(&cancellation).await;
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }

    pub async fn start_regular_tunnel(&self) -> DesktopResult<DesktopStateSnapshot> {
        let (operation, cancellation, mut core, baseline) = self
            .begin_operation(DesktopOperationKind::RegularTunnelStart, true)
            .await?;
        let result = core.start_regular_tunnel(&cancellation).await;
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }

    pub async fn stop_regular_tunnel(&self) -> DesktopResult<DesktopStateSnapshot> {
        let (operation, cancellation, mut core, baseline) = self
            .begin_operation(DesktopOperationKind::RegularTunnelStop, false)
            .await?;
        let result = core.stop_regular_tunnel(&cancellation).await;
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }

    pub async fn stop_local_runtime(&self) -> DesktopResult<DesktopStateSnapshot> {
        let (operation, cancellation, mut core, baseline) = self
            .begin_operation(DesktopOperationKind::LocalRuntimeStop, false)
            .await?;
        let result = core.stop_local_runtime(&cancellation).await;
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }

    pub fn cancel_operation(&self, operation_id: &str) -> DesktopResult<DesktopStateSnapshot> {
        self.operations.cancel(operation_id)?;
        Ok(self.get_state())
    }

    pub async fn shutdown(&self) {
        if self.shutdown_started.swap(true, Ordering::SeqCst) {
            return;
        }
        self.shutdown_signal.cancel();
        self.operations.cancel_active_for_shutdown();
        self.supervisor.lock().await.stop_all().await;
        let _ = self
            .operations
            .wait_until_idle(tokio::time::Instant::now() + SHUTDOWN_OPERATION_WAIT)
            .await;
    }

    async fn begin_operation(
        &self,
        kind: DesktopOperationKind,
        cancellable: bool,
    ) -> DesktopResult<(
        OperationAdmission,
        CancellationContext,
        RuntimeCoordinator,
        ProcessBaseline,
    )> {
        if self.shutdown_signal.is_cancelled() {
            return Err(cancelled_error());
        }
        let operation = self.operations.admit(kind, cancellable)?;
        let cancellation =
            CancellationContext::new(operation.cancellation.clone(), self.shutdown_signal.clone());
        let baseline = self.capture_process_baseline().await;
        let core = {
            let mut slot = self.core.lock().await;
            slot.take()
        };
        let Some(core) = core else {
            let error = DesktopError::new(
                "desktop_operation_busy",
                "Desktop mutation state is already in use",
                "Wait for the current operation to finish.",
            );
            let result: DesktopResult<()> = Err(error.clone());
            self.operations.finish(&operation.id, &result);
            return Err(error);
        };
        Ok((operation, cancellation, core, baseline))
    }

    async fn finish_operation(
        &self,
        operation: OperationAdmission,
        cancellation: CancellationContext,
        mut core: RuntimeCoordinator,
        baseline: ProcessBaseline,
        mut result: DesktopResult<DesktopStateSnapshot>,
    ) -> DesktopResult<DesktopStateSnapshot> {
        if result.is_ok() && cancellation.is_cancelled() {
            result = Err(cancelled_error());
        }
        if result.is_err() {
            let cancelled = result
                .as_ref()
                .err()
                .is_some_and(|error| error.code == "desktop_operation_cancelled");
            let cleanup = self.cleanup_new_owned_processes(&baseline).await;
            core.reconcile_after_operation_failure(operation.kind, &baseline, cleanup, cancelled);
            core.publish_snapshot();
        }
        {
            let mut slot = self.core.lock().await;
            *slot = Some(core);
        }
        self.operations.finish(&operation.id, &result);
        match result {
            Ok(_) => Ok(self.get_state()),
            Err(error) => Err(error),
        }
    }

    async fn capture_process_baseline(&self) -> ProcessBaseline {
        let snapshot = self
            .published
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let mut supervisor = self.supervisor.lock().await;
        ProcessBaseline {
            local_server: process_is_active(supervisor.snapshot(ProcessKind::LocalServer)),
            local_runner: process_is_active(supervisor.snapshot(ProcessKind::LocalRunner)),
            quick_share: process_is_active(supervisor.snapshot(ProcessKind::QuickShare)),
            regular_tunnel: process_is_active(supervisor.snapshot(ProcessKind::RegularTunnel)),
            snapshot,
        }
    }

    async fn cleanup_new_owned_processes(&self, baseline: &ProcessBaseline) -> ProcessCleanup {
        let mut supervisor = self.supervisor.lock().await;
        let mut cleanup = ProcessCleanup::default();
        for (kind, existed) in [
            (ProcessKind::QuickShare, baseline.quick_share),
            (ProcessKind::RegularTunnel, baseline.regular_tunnel),
            (ProcessKind::LocalRunner, baseline.local_runner),
            (ProcessKind::LocalServer, baseline.local_server),
        ] {
            if !existed && supervisor.snapshot(kind).is_some() {
                supervisor.stop(kind).await;
                cleanup.mark_stopped(kind);
            }
        }
        cleanup
    }

    #[cfg(test)]
    async fn hold_test_operation(
        &self,
        started: tokio::sync::oneshot::Sender<String>,
        cleanup_release: tokio::sync::oneshot::Receiver<()>,
    ) -> DesktopResult<DesktopStateSnapshot> {
        let (operation, cancellation, core, baseline) = self
            .begin_operation(DesktopOperationKind::LocalSetup, true)
            .await?;
        let _ = started.send(operation.id.clone());
        cancellation.cancelled().await;
        let _ = cleanup_release.await;
        let result: DesktopResult<DesktopStateSnapshot> = Err(cancelled_error());
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }

    #[cfg(test)]
    async fn run_test_one_shot_operation(
        &self,
        executable: PathBuf,
        args: Vec<String>,
        payload: Vec<u8>,
        timeout: Duration,
    ) -> DesktopResult<DesktopStateSnapshot> {
        let (operation, cancellation, mut core, baseline) = self
            .begin_operation(DesktopOperationKind::RuntimeRefresh, true)
            .await?;
        let result = crate::chadex_core::runtime_compat::integration::run_test_bounded(
            &executable,
            &args,
            Some(&payload),
            &cancellation,
            timeout,
        )
        .await
        .map(|_| core.publish_snapshot());
        self.finish_operation(operation, cancellation, core, baseline, result)
            .await
    }
}

#[derive(Clone)]
struct ProcessBaseline {
    local_server: bool,
    local_runner: bool,
    quick_share: bool,
    regular_tunnel: bool,
    snapshot: DesktopStateSnapshot,
}

#[derive(Clone, Copy, Default)]
struct ProcessCleanup {
    local_server: bool,
    local_runner: bool,
    quick_share: bool,
    regular_tunnel: bool,
}

impl ProcessCleanup {
    fn mark_stopped(&mut self, kind: ProcessKind) {
        match kind {
            ProcessKind::LocalServer => self.local_server = true,
            ProcessKind::LocalRunner => self.local_runner = true,
            ProcessKind::QuickShare => self.quick_share = true,
            ProcessKind::RegularTunnel => self.regular_tunnel = true,
        }
    }
}

fn project_not_loaded_error() -> DesktopError {
    readiness_timeout_error(
        "project_not_loaded",
        "The selected project did not become ready on the current Runner",
        "Retry project activation after checking Runner and project diagnostics.",
    )
}

fn process_is_active(snapshot: Option<crate::chadex_core::runtime_compat::process::ProcessSnapshot>) -> bool {
    snapshot.is_some_and(|process| {
        matches!(
            process.phase,
            ProcessPhase::Starting | ProcessPhase::Running | ProcessPhase::Stopping
        )
    })
}

fn can_refresh_legacy_runner(snapshot: Option<crate::chadex_core::runtime_compat::process::ProcessSnapshot>) -> bool {
    snapshot.is_some_and(|process| {
        process.owned_by_desktop
            && matches!(
                process.phase,
                ProcessPhase::Starting | ProcessPhase::Running | ProcessPhase::Stopping
            )
    })
}

pub struct RuntimeCoordinator {
    data_dir: PathBuf,
    default_project_dir: PathBuf,
    config_path: PathBuf,
    config: StoredDesktopConfig,
    tunnel_config: TunnelConfig,
    snapshot: DesktopStateSnapshot,
    adapter: RuntimeIntegrationBridge,
    supervisor: SharedSupervisor,
    activity: ActivityLog,
    published: Arc<RwLock<DesktopStateSnapshot>>,
}

impl RuntimeCoordinator {
    fn new(data_dir: PathBuf, resource_dir: PathBuf) -> DesktopResult<Self> {
        let tunnel_config =
            TunnelConfig::load(&data_dir.join("secrets").join("tunnel-config.json"));
        Self::new_with_tunnel_config(data_dir, resource_dir, tunnel_config)
    }

    fn new_with_tunnel_config(
        data_dir: PathBuf,
        resource_dir: PathBuf,
        tunnel_config: TunnelConfig,
    ) -> DesktopResult<Self> {
        let activity = ActivityLog::default();
        let default_project_dir = default_management_project_dir(&data_dir, &resource_dir);
        let config_path = data_dir.join("desktop-state.json");
        let config = load_config(&config_path, &activity)?;
        let mut snapshot = DesktopStateSnapshot::default();
        snapshot.topology = config.topology.clone();
        snapshot.project = project_snapshot(&config);
        if config.topology.is_some() && config.runtime_autostart == Some(false) {
            snapshot.readiness = aggregate_readiness(
                ServerReadiness::Stopped,
                RunnerReadiness::Stopped,
                ExposureReadiness::Disabled,
                if config.project.is_some() {
                    ProjectReadiness::Configured
                } else {
                    ProjectReadiness::None
                },
            );
        }
        apply_openai_tunnel_configuration(&mut snapshot, &tunnel_config);
        snapshot.regular_tunnel_available = true;
        snapshot.powershell_runtime = crate::chadex_core::runtime_compat::platform::powershell_runtime_snapshot();
        apply_config_projection(&mut snapshot, &config);
        let published = Arc::new(RwLock::new(snapshot.clone()));
        let supervisor = Arc::new(Mutex::new(ProcessSupervisor::new(activity.clone())));
        let task_state_dir = data_dir.join("phase9-tasks");
        Ok(Self {
            data_dir,
            default_project_dir,
            config_path,
            config,
            tunnel_config,
            snapshot,
            adapter: RuntimeIntegrationBridge::new(
                Some(resource_dir.join("chadex-runtime")),
                task_state_dir,
            ),
            supervisor,
            activity,
            published,
        })
    }

    pub async fn get_state(&mut self) -> DesktopResult<DesktopStateSnapshot> {
        apply_openai_tunnel_configuration(&mut self.snapshot, &self.tunnel_config);
        self.snapshot.regular_tunnel_available = true;
        apply_config_projection(&mut self.snapshot, &self.config);
        if self.snapshot.regular_tunnel.is_some() {
            let active = self
                .process_snapshot(ProcessKind::RegularTunnel)
                .await
                .is_some_and(|process| {
                    matches!(
                        process.phase,
                        ProcessPhase::Starting | ProcessPhase::Running
                    )
                });
            if let Some(exposure) =
                regular_tunnel_exposure(&mut self.snapshot.regular_tunnel, active)
            {
                self.snapshot.readiness = aggregate_readiness(
                    self.snapshot.readiness.server.clone(),
                    self.snapshot.readiness.runner.clone(),
                    exposure.clone(),
                    self.snapshot.readiness.project.clone(),
                );
                apply_regular_tunnel_next_action(&mut self.snapshot, &exposure);
                self.snapshot.topology = effective_topology(
                    self.config.topology.as_ref(),
                    self.snapshot.regular_tunnel.is_some(),
                );
            }
        }
        Ok(self.publish_snapshot())
    }

    fn chatgpt_activity_probe(&self) -> Option<ChatGptActivityProbe> {
        if !self.snapshot.readiness.runtime_ready
            || self
                .snapshot
                .chatgpt_activity
                .as_ref()
                .is_some_and(|activity| activity.observed)
        {
            return None;
        }
        let identity = identity_from_config(&self.config)?;
        let cli = self.adapter.binaries().ok()?.cli.clone();
        Some(ChatGptActivityProbe { identity, cli })
    }

    fn apply_chatgpt_activity_observation(
        &mut self,
        expected_identity: &ProjectRuntimeIdentity,
        last_meaningful_activity_at_ms: Option<i64>,
    ) -> DesktopResult<DesktopStateSnapshot> {
        if !self.snapshot.readiness.runtime_ready
            || identity_from_config(&self.config).as_ref() != Some(expected_identity)
            || self
                .snapshot
                .chatgpt_activity
                .as_ref()
                .is_some_and(|activity| activity.observed)
        {
            return Ok(self.publish_snapshot());
        }
        self.snapshot.chatgpt_activity = Some(ChatGptActivitySnapshot {
            observed: last_meaningful_activity_at_ms.is_some(),
            last_meaningful_activity_at_ms,
        });
        Ok(self.publish_snapshot())
    }

    fn stage_project_scope(&mut self, project: ProjectSelection) {
        self.snapshot.project = Some(project);
        // ChatGPT activity is evidence for one exact runtime Project. Never carry
        // an observation from the previously displayed Project into a new setup
        // or Quick Share scope; the new Project must earn its own observation.
        self.snapshot.chatgpt_activity = None;
    }

    pub async fn refresh_runtime_status(
        &mut self,
        cancellation: &CancellationContext,
    ) -> DesktopResult<DesktopStateSnapshot> {
        cancellation.check()?;
        if self.snapshot.quick_share.is_some() {
            self.snapshot.chatgpt_activity = None;
            let active = self
                .process_snapshot(ProcessKind::QuickShare)
                .await
                .is_some_and(|process| {
                    matches!(
                        process.phase,
                        ProcessPhase::Starting | ProcessPhase::Running
                    )
                });
            if !active {
                self.snapshot.readiness = aggregate_readiness(
                    ServerReadiness::Stopped,
                    RunnerReadiness::Stopped,
                    ExposureReadiness::Error,
                    ProjectReadiness::Configured,
                );
                self.snapshot.readiness.summary_kind = ReadinessSummaryKind::QuickShareStopped;
                self.snapshot.readiness.next_action_kind =
                    Some(ReadinessNextActionKind::RestartQuickShare);
                self.snapshot.readiness.summary = "Quick Share stopped".to_string();
                self.snapshot.readiness.next_action = Some("Start Quick Share again.".to_string());
            }
            return self.get_state().await;
        }

        let Some(identity) = identity_from_config(&self.config) else {
            self.snapshot.chatgpt_activity = None;
            self.snapshot.topology = self.config.topology.clone();
            self.snapshot.project = project_snapshot(&self.config);
            return self.get_state().await;
        };
        self.adapter.ensure_binaries(cancellation).await.ok();
        cancellation.check()?;
        if let Ok(binaries) = self.adapter.binaries() {
            self.snapshot.binaries = Some(binaries.info());
        }
        let server = match self
            .adapter
            .server_status(
                Some(&identity.server_url),
                self.config
                    .runtime
                    .as_ref()
                    .and_then(|runtime| runtime.server_env_file.as_deref()),
                Some(&identity.user_token_file),
                cancellation,
            )
            .await
        {
            Ok(status) if status.http_reachable => ServerReadiness::Ready,
            Ok(_) => ServerReadiness::Error,
            Err(_) => ServerReadiness::Unknown,
        };
        cancellation.check()?;
        // A saved user stop remains stopped across refresh, while a reachable
        // external service is still observed normally.
        if !runtime_autostart(&self.config) && server != ServerReadiness::Ready {
            self.snapshot.chatgpt_activity = None;
            self.snapshot.readiness = aggregate_readiness(
                ServerReadiness::Stopped,
                RunnerReadiness::Stopped,
                ExposureReadiness::Disabled,
                ProjectReadiness::Configured,
            );
            return self.get_state().await;
        }
        let runner = match self.adapter.runner_ready(&identity, cancellation).await {
            Ok(true) => RunnerReadiness::Ready,
            Ok(false) => RunnerReadiness::Connecting,
            Err(_) => RunnerReadiness::Unknown,
        };
        cancellation.check()?;
        let project = match self.adapter.project_ready(&identity, cancellation).await {
            Ok(true) => ProjectReadiness::Ready,
            Ok(false) => ProjectReadiness::ReloadRequired,
            Err(_) => ProjectReadiness::Unknown,
        };
        cancellation.check()?;
        self.snapshot.chatgpt_activity =
            if server == ServerReadiness::Ready && project == ProjectReadiness::Ready {
                match self.adapter.chatgpt_activity(&identity, cancellation).await {
                    Ok(last_meaningful_activity_at_ms) => Some(ChatGptActivitySnapshot {
                        observed: last_meaningful_activity_at_ms.is_some(),
                        last_meaningful_activity_at_ms,
                    }),
                    Err(_) => None,
                }
            } else {
                None
            };
        cancellation.check()?;
        let tunnel_active = self
            .process_snapshot(ProcessKind::RegularTunnel)
            .await
            .is_some_and(|process| {
                matches!(
                    process.phase,
                    ProcessPhase::Starting | ProcessPhase::Running
                )
            });
        let regular_tunnel_expected = self.snapshot.regular_tunnel.is_some();
        let exposure = regular_tunnel_exposure(&mut self.snapshot.regular_tunnel, tunnel_active)
            .unwrap_or_else(|| exposure_readiness(self.config.topology.as_ref()));
        self.snapshot.readiness = aggregate_readiness(server, runner, exposure.clone(), project);
        if regular_tunnel_expected && exposure == ExposureReadiness::Error {
            self.snapshot.readiness.next_action_kind =
                Some(ReadinessNextActionKind::RestartSecureTunnel);
            self.snapshot.readiness.next_action = Some("Restart the secure tunnel.".to_string());
        } else if regular_tunnel_expected && exposure == ExposureReadiness::Degraded {
            self.snapshot.readiness.next_action_kind =
                Some(ReadinessNextActionKind::RestoreClipboardHandoff);
            self.snapshot.readiness.next_action = Some(
                "Restore clipboard access, then restart the secure tunnel handoff.".to_string(),
            );
        }
        self.snapshot.topology = effective_topology(
            self.config.topology.as_ref(),
            self.snapshot.regular_tunnel.is_some(),
        );
        self.snapshot.project = project_snapshot(&self.config);
        self.get_state().await
    }

    fn publish_snapshot(&mut self) -> DesktopStateSnapshot {
        self.snapshot.current_operation = None;
        self.snapshot.activity_sequence = self.activity.latest_sequence();
        apply_openai_tunnel_configuration(&mut self.snapshot, &self.tunnel_config);
        self.snapshot.regular_tunnel_available = true;
        self.snapshot.powershell_runtime = crate::chadex_core::runtime_compat::platform::powershell_runtime_snapshot();
        apply_config_projection(&mut self.snapshot, &self.config);
        let snapshot = self.snapshot.clone();
        *self
            .published
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot.clone();
        snapshot
    }

    fn reconcile_after_operation_failure(
        &mut self,
        kind: DesktopOperationKind,
        baseline: &ProcessBaseline,
        cleanup: ProcessCleanup,
        cancelled: bool,
    ) {
        let observed_binaries = self.snapshot.binaries.clone();
        match kind {
            DesktopOperationKind::RuntimeResume => {
                // Resume is a desired-state replay of an already committed setup.
                // If any step fails or is cancelled, newly owned processes have
                // already been reclaimed above; restore the last published view
                // rather than leaving a synthetic "starting" state behind.
                self.snapshot = baseline.snapshot.clone();
            }
            DesktopOperationKind::QuickShareStart => {
                // Quick Share is intentionally ephemeral. Any failed start has
                // already stopped (or will have cleanup stop) the newly owned
                // foreground process, so return the public topology to the
                // last committed runtime instead of leaving "starting" behind.
                self.snapshot = baseline.snapshot.clone();
            }
            DesktopOperationKind::RegularTunnelStart if cancelled => {
                // User cancellation is not a tunnel failure. Restore the last
                // observed full-runtime state after the exact owned tunnel is
                // reclaimed rather than publishing a synthetic tunnel error.
                self.snapshot = baseline.snapshot.clone();
            }
            DesktopOperationKind::RuntimeRefresh if cancelled => {
                // A cancelled observation must not partially overwrite the
                // last published control-plane state.
                self.snapshot = baseline.snapshot.clone();
            }
            DesktopOperationKind::LocalSetup => {
                let server = if cleanup.local_server {
                    ServerReadiness::Stopped
                } else if self.snapshot.readiness.server == ServerReadiness::Starting {
                    baseline.snapshot.readiness.server.clone()
                } else {
                    self.snapshot.readiness.server.clone()
                };
                let runner = if cleanup.local_runner {
                    RunnerReadiness::Stopped
                } else if self.snapshot.readiness.runner == RunnerReadiness::Connecting {
                    baseline.snapshot.readiness.runner.clone()
                } else {
                    self.snapshot.readiness.runner.clone()
                };
                let project = if cleanup.local_server || cleanup.local_runner {
                    self.snapshot
                        .project
                        .as_ref()
                        .map(|_| ProjectReadiness::Configured)
                        .unwrap_or(ProjectReadiness::None)
                } else {
                    self.snapshot.readiness.project.clone()
                };
                self.snapshot.readiness = aggregate_readiness(
                    server,
                    runner,
                    self.snapshot.readiness.exposure.clone(),
                    project,
                );
            }
            DesktopOperationKind::RemoteSetup => {
                let runner = if cleanup.local_runner {
                    RunnerReadiness::Stopped
                } else if self.snapshot.readiness.runner == RunnerReadiness::Connecting {
                    baseline.snapshot.readiness.runner.clone()
                } else {
                    self.snapshot.readiness.runner.clone()
                };
                let project = if cleanup.local_runner {
                    self.snapshot
                        .project
                        .as_ref()
                        .map(|_| ProjectReadiness::Configured)
                        .unwrap_or(ProjectReadiness::None)
                } else {
                    self.snapshot.readiness.project.clone()
                };
                self.snapshot.readiness = aggregate_readiness(
                    self.snapshot.readiness.server.clone(),
                    runner,
                    self.snapshot.readiness.exposure.clone(),
                    project,
                );
            }
            _ => {}
        }
        if observed_binaries.is_some() {
            self.snapshot.binaries = observed_binaries;
        }
    }

    pub async fn resume_saved_runtime(
        &mut self,
        cancellation: &CancellationContext,
    ) -> DesktopResult<DesktopStateSnapshot> {
        cancellation.check()?;
        let Some(topology) = self.config.topology.clone() else {
            return self.get_state().await;
        };
        if topology.experience != Experience::Full {
            return self.get_state().await;
        }
        let project_path = self
            .config
            .project
            .as_ref()
            .map(|project| project.path.clone());
        match topology.server {
            ServerTopology::Local => {
                self.configure_local_setup(project_path.as_deref(), cancellation)
                    .await
            }
            ServerTopology::Remote { url } => {
                let project_path = project_path.ok_or_else(|| {
                    DesktopError::new(
                        "project_not_ready",
                        "The saved remote Desktop runtime no longer has a project selection",
                        "Choose a project or change the Desktop runtime setup.",
                    )
                })?;
                self.configure_remote_setup(&url, "", &project_path, cancellation)
                    .await
            }
        }
    }

    pub async fn update_tunnel_proxy(
        &mut self,
        mode: TunnelProxyMode,
        custom_url: Option<&str>,
        cancellation: &CancellationContext,
    ) -> DesktopResult<DesktopStateSnapshot> {
        cancellation.check()?;
        let custom_url = match mode {
            TunnelProxyMode::Custom => Some(validate_tunnel_proxy_url(custom_url.unwrap_or(""))?),
            TunnelProxyMode::Auto | TunnelProxyMode::Direct => None,
        };
        self.config.tunnel_proxy = TunnelProxyConfig { mode, custom_url };
        self.save_config().await?;
        self.get_state().await
    }

    async fn chadex_project_activation_target(
        &mut self,
        project_path: &str,
        cancellation: &CancellationContext,
    ) -> DesktopResult<ChadexProjectActivationTarget> {
        cancellation.check()?;
        let local_full = self.config.topology.as_ref().is_some_and(|topology| {
            topology.experience == Experience::Full
                && matches!(topology.server, ServerTopology::Local)
        });
        if !local_full || self.snapshot.quick_share.is_some() {
            return Err(DesktopError::new(
                "unsupported_topology",
                "Projects can only be activated in place on a local Full Runtime",
                "Use the full runtime setup flow before activating another project.",
            ));
        }
        if !self.snapshot.readiness.runtime_ready {
            return Err(DesktopError::new(
                "runtime_not_ready",
                "The local Full Runtime is not ready for an in-place project activation",
                "Restore the local runtime, then choose the project again.",
            ));
        }
        let project = self.adapter.inspect_project(project_path).await?;
        cancellation.check()?;
        let identity = identity_from_config(&self.config).ok_or_else(|| {
            DesktopError::new(
                "runtime_not_ready",
                "The saved local Runner identity is incomplete",
                "Restore or reconfigure the local runtime before activating another project.",
            )
        })?;
        let runner_client_id = stored_runner_client_id(&self.config).ok_or_else(|| {
            DesktopError::new(
                "runner_offline",
                "The saved local Runner client identity is unavailable",
                "Restore or reconfigure the local runtime before activating another project.",
            )
        })?;
        Ok(ChadexProjectActivationTarget {
            project,
            server_url: identity.server_url,
            user_token_file: identity.user_token_file,
            runner_config: identity.runner_config,
            runner_client_id,
        })
    }

    async fn apply_chadex_project_activation(
        &mut self,
        target: ChadexProjectActivationTarget,
        observation: ChadexProjectActivationObservation,
        cancellation: &CancellationContext,
    ) -> DesktopResult<DesktopStateSnapshot> {
        cancellation.check()?;
        let identity = identity_from_config(&self.config).ok_or_else(|| {
            DesktopError::new(
                "project_activation_reconcile_required",
                "The saved local Runner identity changed during project activation",
                "Observe the current runtime state before retrying the project switch.",
            )
        })?;
        let runner_client_id = stored_runner_client_id(&self.config).ok_or_else(|| {
            DesktopError::new(
                "project_activation_reconcile_required",
                "The saved local Runner identity changed during project activation",
                "Observe the current runtime state before retrying the project switch.",
            )
        })?;
        if identity.server_url != target.server_url
            || identity.user_token_file != target.user_token_file
            || identity.runner_config != target.runner_config
            || runner_client_id != target.runner_client_id
            || observation.project_id.trim().is_empty()
            || observation.runtime_project_id.trim().is_empty()
            || !chadex_runtime_runner_config::paths::paths_equal(
                Path::new(&observation.project_path),
                Path::new(&target.project.path),
            )
        {
            return Err(DesktopError::new(
                "project_activation_reconcile_required",
                "The local runtime identity changed during project activation",
                "Observe the current runtime state before retrying the project switch.",
            ));
        }
        let activated_identity = ProjectRuntimeIdentity {
            project_id: observation.project_id,
            runtime_project_id: observation.runtime_project_id,
            project_path: observation.project_path,
            runner_config: target.runner_config,
            user_token_file: target.user_token_file,
            server_url: target.server_url,
        };
        cancellation.check()?;
        self.commit_local_project_activation(target.project, activated_identity)
            .await
    }

    pub async fn activate_local_project(
        &mut self,
        project_path: &str,
        cancellation: &CancellationContext,
    ) -> DesktopResult<DesktopStateSnapshot> {
        cancellation.check()?;
        let local_full = self.config.topology.as_ref().is_some_and(|topology| {
            topology.experience == Experience::Full
                && matches!(topology.server, ServerTopology::Local)
        });
        if !local_full || self.snapshot.quick_share.is_some() {
            return Err(DesktopError::new(
                "unsupported_topology",
                "Projects can only be activated in place on a local Full Runtime",
                "Use the full runtime setup flow before activating another project.",
            ));
        }
        if !self.snapshot.readiness.runtime_ready {
            return Err(DesktopError::new(
                "runtime_not_ready",
                "The local Full Runtime is not ready for an in-place project activation",
                "Restore the local runtime, then choose the project again.",
            ));
        }

        let project = self.adapter.inspect_project(project_path).await?;
        cancellation.check()?;
        let identity = identity_from_config(&self.config).ok_or_else(|| {
            DesktopError::new(
                "runtime_not_ready",
                "The saved local Runner identity is incomplete",
                "Restore or reconfigure the local runtime before activating another project.",
            )
        })?;
        let runner_client_id = stored_runner_client_id(&self.config).ok_or_else(|| {
            DesktopError::new(
                "runner_offline",
                "The saved local Runner client identity is unavailable",
                "Restore or reconfigure the local runtime before activating another project.",
            )
        })?;
        let runner = self
            .adapter
            .observe_runner_connection(&identity, Some(&runner_client_id), cancellation)
            .await?;
        cancellation.check()?;
        if !runner.online {
            return Err(DesktopError::new(
                "runner_offline",
                "The current local Runner is not online",
                "Restore the local runtime, then choose the project again.",
            ));
        }

        let identity = self
            .adapter
            .activate_project(&identity, &runner_client_id, &project, cancellation)
            .await?;
        self.wait_for_project(&identity, cancellation).await?;
        cancellation.check()?;

        self.commit_local_project_activation(project, identity)
            .await
    }

    async fn commit_local_project_activation(
        &mut self,
        project: ProjectSelection,
        identity: ProjectRuntimeIdentity,
    ) -> DesktopResult<DesktopStateSnapshot> {
        let previous_config = self.config.clone();
        let previous_snapshot = self.snapshot.clone();
        let mut committed_project = project;
        committed_project.runtime_project_id = Some(identity.runtime_project_id.clone());
        let runtime = self.config.runtime.as_mut().ok_or_else(|| {
            DesktopError::new(
                "runtime_not_ready",
                "The local runtime disappeared during project activation",
                "Restore the local runtime, then choose the project again.",
            )
        })?;
        runtime.project_id = Some(identity.project_id);
        runtime.runtime_project_id = Some(identity.runtime_project_id);
        self.config.project = Some(committed_project.clone());
        if let Err(error) = self.save_config().await {
            self.config = previous_config;
            self.snapshot = previous_snapshot;
            self.publish_snapshot();
            return Err(error);
        }

        self.activity.push(
            ActivityEventKind::ProjectActivated,
            "desktop",
            ActivityLevel::Info,
            std::path::Path::new(&committed_project.path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy(),
        );
        self.snapshot.project = Some(committed_project);
        self.snapshot.chatgpt_activity = None;
        self.snapshot.readiness = aggregate_readiness(
            self.snapshot.readiness.server.clone(),
            self.snapshot.readiness.runner.clone(),
            self.snapshot.readiness.exposure.clone(),
            ProjectReadiness::Ready,
        );
        self.get_state().await
    }

    pub async fn configure_local_setup(
        &mut self,
        project_path: Option<&str>,
        cancellation: &CancellationContext,
    ) -> DesktopResult<DesktopStateSnapshot> {
        self.configure_local_setup_inner(
            project_path,
            cancellation,
            |_target, _cancellation| async { Ok(None) },
        )
        .await
    }

    async fn configure_local_setup_with_chadex_fast_path<F, Fut>(
        &mut self,
        project_path: Option<&str>,
        cancellation: &CancellationContext,
        fast_path: F,
    ) -> DesktopResult<DesktopStateSnapshot>
    where
        F: FnMut(ChadexProjectActivationTarget, CancellationContext) -> Fut,
        Fut: Future<Output = DesktopResult<Option<ChadexProjectActivationObservation>>>,
    {
        self.configure_local_setup_inner(project_path, cancellation, fast_path)
            .await
    }

    async fn configure_local_setup_inner<F, Fut>(
        &mut self,
        project_path: Option<&str>,
        cancellation: &CancellationContext,
        mut fast_path: F,
    ) -> DesktopResult<DesktopStateSnapshot>
    where
        F: FnMut(ChadexProjectActivationTarget, CancellationContext) -> Fut,
        Fut: Future<Output = DesktopResult<Option<ChadexProjectActivationObservation>>>,
    {
        cancellation.check()?;
        let project = match project_path.map(str::trim).filter(|path| !path.is_empty()) {
            Some(path) => self.adapter.inspect_project(path).await?,
            None => {
                tokio::fs::create_dir_all(&self.default_project_dir)
                    .await
                    .map_err(|error| {
                        DesktopError::new(
                            "default_project_unavailable",
                            "Desktop could not prepare its default management project",
                            "Check that the WebCodex Desktop install directory is writable, or choose another project from Change runtime mode.",
                        )
                        .with_details(serde_json::json!({ "io_kind": format!("{:?}", error.kind()) }))
                    })?;
                let mut project = self
                    .adapter
                    .inspect_project(&self.default_project_dir.to_string_lossy())
                    .await?;
                // The automatically registered management project should not
                // also grant authority to register sibling installation folders.
                // Equality is a valid allowed-root boundary, so keep this
                // implicit setup scoped to the installation directory itself.
                project.allowed_root = project.path.clone();
                project
            }
        };
        cancellation.check()?;
        let binaries = self.adapter.ensure_binaries(cancellation).await?.clone();
        self.snapshot.binaries = Some(binaries.info());
        self.activity.push(
            ActivityEventKind::LocalSetupPreparing,
            "desktop",
            ActivityLevel::Info,
            "Preparing WebCodex on this computer",
        );
        self.snapshot.topology = Some(RuntimeTopology {
            experience: Experience::Full,
            server: ServerTopology::Local,
            runner: RunnerTopology::Local,
            exposure: Exposure::None,
            enrollment: Enrollment::ManagedPairing,
        });
        self.stage_project_scope(project.clone());
        self.snapshot.readiness = aggregate_readiness(
            ServerReadiness::Starting,
            RunnerReadiness::Stopped,
            ExposureReadiness::Disabled,
            ProjectReadiness::Configured,
        );
        self.publish_snapshot();

        let local_dir = self.data_dir.join("runtime").join("local");
        let env_file = local_dir.join("webcodex.env");
        let data_dir = local_dir.join("data");
        tokio::fs::create_dir_all(&local_dir).await.map_err(|_| {
            DesktopError::new(
                "desktop_state_unavailable",
                "Desktop could not create its local runtime directory",
                "Check local app-data permissions and retry.",
            )
        })?;
        cancellation.check()?;

        let server_url = if env_file.is_file() {
            ensure_desktop_server_defaults(&env_file)?;
            self.adapter
                .server_status(None, Some(&env_file), None, cancellation)
                .await?
                .probe_url
        } else {
            let listen = reserve_loopback_address()?;
            let status = self
                .adapter
                .init_local_server(&listen, &data_dir, &env_file, cancellation)
                .await?;
            ensure_desktop_server_defaults(&env_file)?;
            status.probe_url
        };
        let reusable_identity = identity_from_config(&self.config)
            .filter(|identity| same_server(&identity.server_url, &server_url));
        let saved_runner_client_id = stored_runner_client_id(&self.config);
        cancellation.check()?;

        let server_deadline = Deadline::after(SERVER_READY_TIMEOUT);
        let running = self
            .adapter
            .server_status_until(
                Some(&server_url),
                Some(&env_file),
                None,
                cancellation,
                server_deadline,
            )
            .await
            .is_ok_and(|status| status.http_reachable);
        cancellation.check()?;
        let server_started = if !running {
            if server_deadline.is_elapsed() {
                return Err(readiness_timeout_error(
                    "server_unreachable",
                    "WebCodex Service did not become ready",
                    "Check the local Service diagnostics and retry.",
                ));
            }
            let command = self.adapter.local_server_command(&env_file)?;
            self.spawn_owned(ProcessKind::LocalServer, command, false, cancellation)
                .await?;
            true
        } else {
            false
        };
        self.wait_for_server(
            &server_url,
            Some(&env_file),
            None,
            cancellation,
            server_deadline,
            server_started,
        )
        .await?;
        self.snapshot.readiness.server = ServerReadiness::Ready;
        self.publish_snapshot();

        let reusable_observation = match reusable_identity.as_ref() {
            Some(identity) => self
                .adapter
                .observe_runner_connection(
                    identity,
                    saved_runner_client_id.as_deref(),
                    cancellation,
                )
                .await
                .ok(),
            None => None,
        };
        let (mut identity, identity_replaced, runner_client_id) =
            match (reusable_identity, reusable_observation) {
                (Some(identity), Some(observation)) => (identity, false, observation.client_id),
                _ => {
                    // Login publishes with --overwrite. Keep the saved connection's
                    // files intact until activation and config persistence succeed.
                    let connections_dir = local_enrollment_directory(&self.data_dir, &self.config);
                    let pairing_code = self
                        .adapter
                        .create_local_pairing(&server_url, &env_file, cancellation)
                        .await?;
                    let identity = self
                        .adapter
                        .login_with_pairing(
                            &server_url,
                            &pairing_code,
                            &connections_dir,
                            &project,
                            cancellation,
                        )
                        .await?;
                    drop(pairing_code);
                    cancellation.check()?;
                    let observation = self
                        .adapter
                        .observe_runner_connection(&identity, None, cancellation)
                        .await?;
                    (identity, true, observation.client_id)
                }
            };

        let replacing_owned_runner = identity_replaced
            && process_is_active(self.process_snapshot(ProcessKind::LocalRunner).await);
        let runner_deadline = Deadline::after(RUNNER_READY_TIMEOUT);
        let activation: DesktopResult<bool> = async {
            if replacing_owned_runner {
                // A Desktop-owned Runner can only serve the exact config it was
                // started with. Replace that owned process transactionally while
                // keeping the local Server alive; never broad-kill unrelated Runners.
                self.stop_process_until(ProcessKind::LocalRunner, runner_deadline)
                    .await;
                if runner_deadline.is_elapsed() {
                    return Err(readiness_timeout_error(
                        "runner_offline",
                        "Desktop could not stop its previous Runner before changing projects",
                        "Retry project setup. Desktop will only replace the Runner it owns.",
                    ));
                }
                cancellation.check()?;
            }

            let runner_ready = if identity_replaced {
                // A fresh Desktop enrollment may reuse the same client_id while
                // changing the Runner token or project registry. An older Runner
                // with that client_id is not proof that this exact config is active.
                false
            } else {
                self.adapter
                    .runner_ready_until(&identity, cancellation, runner_deadline)
                    .await
                    .unwrap_or(false)
            };
            cancellation.check()?;
            let mut runner_started = if !runner_ready {
                if runner_deadline.is_elapsed() {
                    return Err(readiness_timeout_error(
                        "runner_offline",
                        "Runner did not become connected",
                        "Retry project setup to restart Desktop's Runner.",
                    ));
                }
                self.snapshot.readiness.runner = RunnerReadiness::Connecting;
                self.publish_snapshot();
                let command = self.adapter.local_runner_command(&identity.runner_config)?;
                self.spawn_owned(ProcessKind::LocalRunner, command, false, cancellation)
                    .await?;
                true
            } else {
                false
            };

            let fast_target = ChadexProjectActivationTarget {
                project: project.clone(),
                server_url: identity.server_url.clone(),
                user_token_file: identity.user_token_file.clone(),
                runner_config: identity.runner_config.clone(),
                runner_client_id: runner_client_id.clone(),
            };
            if let Some(observation) = fast_path(fast_target, cancellation.clone()).await? {
                let expected_runtime_project_id =
                    format!("agent:{runner_client_id}:{}", observation.project_id);
                if observation.project_id.trim().is_empty()
                    || observation.runtime_project_id != expected_runtime_project_id
                    || !chadex_runtime_runner_config::paths::paths_equal(
                        Path::new(&observation.project_path),
                        Path::new(&project.path),
                    )
                {
                    return Err(DesktopError::new(
                        "project_activation_reconcile_required",
                        "The active Runner returned a mismatched project identity",
                        "Observe the current runtime state before retrying local setup.",
                    ));
                }
                identity.project_id = observation.project_id;
                identity.runtime_project_id = observation.runtime_project_id;
                identity.project_path = observation.project_path;
                self.snapshot.readiness.runner = RunnerReadiness::Ready;
                self.publish_snapshot();
                cancellation.check()?;
                return Ok(runner_started);
            }

            self.wait_for_runner(&identity, cancellation, runner_deadline, runner_started)
                .await?;
            self.snapshot.readiness.runner = RunnerReadiness::Ready;
            self.publish_snapshot();
            identity = match self
                .adapter
                .activate_project(&identity, &runner_client_id, &project, cancellation)
                .await
            {
                Ok(identity) => identity,
                Err(error)
                    if matches!(
                        error.code.as_str(),
                        "project_activation_capability_unavailable"
                            | "project_activation_restart_required"
                    ) =>
                {
                    if !can_refresh_legacy_runner(
                        self.process_snapshot(ProcessKind::LocalRunner).await,
                    ) {
                        return Err(DesktopError::new(
                            "project_activation_legacy_runner",
                            "This Runner needs to be refreshed before the new project can be activated",
                            "Refresh the Runner, then select this project again.",
                        ));
                    }
                    let legacy_identity = self
                        .adapter
                        .legacy_register_project(
                            &identity,
                            &runner_client_id,
                            &project,
                            cancellation,
                        )
                        .await?;
                    let legacy_deadline = Deadline::after(RUNNER_READY_TIMEOUT);
                    self.stop_process_until(ProcessKind::LocalRunner, legacy_deadline)
                        .await;
                    if legacy_deadline.is_elapsed() {
                        return Err(readiness_timeout_error(
                            "runner_offline",
                            "Desktop could not refresh its legacy Runner",
                            "Retry project setup after checking Runner diagnostics.",
                        ));
                    }
                    let command = self
                        .adapter
                        .local_runner_command(&legacy_identity.runner_config)?;
                    self.spawn_owned(ProcessKind::LocalRunner, command, false, cancellation)
                        .await?;
                    self.wait_for_runner(&legacy_identity, cancellation, legacy_deadline, true)
                        .await?;
                    runner_started = true;
                    legacy_identity
                }
                Err(error) => return Err(error),
            };
            self.wait_for_project(&identity, cancellation).await?;
            cancellation.check()?;
            Ok(runner_started)
        }
        .await;
        let runner_started = match activation {
            Ok(runner_started) => runner_started,
            Err(error) => {
                if replacing_owned_runner {
                    self.stop_process_until(
                        ProcessKind::LocalRunner,
                        Deadline::after(READINESS_CLEANUP_SLACK),
                    )
                    .await;
                    self.snapshot.topology = self.config.topology.clone();
                    self.snapshot.project = project_snapshot(&self.config);
                    self.snapshot.readiness = aggregate_readiness(
                        ServerReadiness::Ready,
                        RunnerReadiness::Stopped,
                        exposure_readiness(self.config.topology.as_ref()),
                        self.config
                            .project
                            .as_ref()
                            .map(|_| ProjectReadiness::Configured)
                            .unwrap_or(ProjectReadiness::None),
                    );
                    self.publish_snapshot();
                }
                return Err(error);
            }
        };

        // Commit the visible/saved project only after the new Runner and exact
        // project have both reached readiness. Until this point the previous
        // stored project remains the recovery authority.
        let previous_config = self.config.clone();
        let mut committed_project = project.clone();
        committed_project.runtime_project_id = Some(identity.runtime_project_id.clone());
        self.config.topology = self.snapshot.topology.clone();
        self.config.project = Some(committed_project.clone());
        self.config.runtime_autostart = Some(true);
        self.config.runtime = Some(StoredRuntime {
            server_url: identity.server_url.clone(),
            server_env_file: Some(env_file.clone()),
            runner_config: Some(identity.runner_config.clone()),
            user_token_file: Some(identity.user_token_file.clone()),
            runner_client_id: Some(runner_client_id.clone()),
            project_id: Some(identity.project_id.clone()),
            runtime_project_id: Some(identity.runtime_project_id.clone()),
        });
        if let Err(error) = self.save_config().await {
            self.config = previous_config;
            self.snapshot.topology = self.config.topology.clone();
            self.snapshot.project = project_snapshot(&self.config);
            if runner_started || replacing_owned_runner {
                self.stop_process_until(
                    ProcessKind::LocalRunner,
                    Deadline::after(READINESS_CLEANUP_SLACK),
                )
                .await;
            }
            if server_started {
                self.stop_process_until(
                    ProcessKind::LocalServer,
                    Deadline::after(READINESS_CLEANUP_SLACK),
                )
                .await;
            }
            self.snapshot.readiness = aggregate_readiness(
                if server_started {
                    ServerReadiness::Stopped
                } else {
                    ServerReadiness::Ready
                },
                if runner_started || replacing_owned_runner {
                    RunnerReadiness::Stopped
                } else {
                    RunnerReadiness::Ready
                },
                ExposureReadiness::Disabled,
                self.config
                    .project
                    .as_ref()
                    .map(|_| ProjectReadiness::Configured)
                    .unwrap_or(ProjectReadiness::None),
            );
            self.publish_snapshot();
            return Err(error);
        }
        self.snapshot.project = Some(committed_project);
        self.snapshot.readiness = aggregate_readiness(
            ServerReadiness::Ready,
            RunnerReadiness::Ready,
            ExposureReadiness::LocalReady,
            ProjectReadiness::Ready,
        );
        self.activity.push(
            ActivityEventKind::LocalRuntimeReady,
            "desktop",
            ActivityLevel::Info,
            "Local WebCodex runtime is ready",
        );
        self.get_state().await
    }

    pub async fn configure_remote_setup(
        &mut self,
        server_url: &str,
        pairing_code: &str,
        project_path: &str,
        cancellation: &CancellationContext,
    ) -> DesktopResult<DesktopStateSnapshot> {
        cancellation.check()?;
        let server_url = crate::chadex_core::runtime_compat::integration::validate_server_url(server_url)?;
        let project = self.adapter.inspect_project(project_path).await?;
        cancellation.check()?;
        let binaries = self.adapter.ensure_binaries(cancellation).await?.clone();
        self.snapshot.binaries = Some(binaries.info());
        let exposure = if server_url.starts_with("https://") {
            Exposure::ExistingHttps {
                url: server_url.clone(),
            }
        } else {
            Exposure::None
        };
        let topology = RuntimeTopology {
            experience: Experience::Full,
            server: ServerTopology::Remote {
                url: server_url.clone(),
            },
            runner: RunnerTopology::Local,
            exposure,
            enrollment: Enrollment::ManagedPairing,
        };
        self.snapshot.topology = Some(topology.clone());
        self.stage_project_scope(project.clone());
        self.config.topology = Some(topology.clone());
        self.config.runtime_autostart = Some(true);
        self.config.preferred_connection = Some(RegularConnectionPreference::NoChatGpt);
        self.snapshot.readiness = aggregate_readiness(
            ServerReadiness::Starting,
            RunnerReadiness::Connecting,
            if server_url.starts_with("https://") {
                ExposureReadiness::Starting
            } else {
                ExposureReadiness::Degraded
            },
            ProjectReadiness::Configured,
        );
        self.activity.push(
            ActivityEventKind::RemoteConnecting,
            "desktop",
            ActivityLevel::Info,
            "Connecting this computer to the existing WebCodex Server",
        );
        self.publish_snapshot();

        let reusable_identity = identity_from_config(&self.config)
            .filter(|identity| same_server(&identity.server_url, &server_url));
        let saved_runner_client_id = stored_runner_client_id(&self.config);
        let reusable_observation = match reusable_identity.as_ref() {
            Some(identity) => self
                .adapter
                .observe_runner_connection(
                    identity,
                    saved_runner_client_id.as_deref(),
                    cancellation,
                )
                .await
                .ok(),
            None => None,
        };
        let (mut identity, identity_replaced, runner_client_id) = match (
            reusable_identity,
            reusable_observation,
        ) {
            (Some(identity), Some(observation)) => (identity, false, observation.client_id),
            _ => {
                if !pairing_code.starts_with("wc_pair_") {
                    return Err(DesktopError::new(
                            "pairing_code_invalid",
                            "The saved Runner identity is not reusable and no new WebCodex pairing code was provided",
                            "Refresh this Runner connection with a new wc_pair_… code.",
                        ));
                }
                let identity = self
                    .adapter
                    .login_with_pairing(
                        &server_url,
                        pairing_code,
                        &self.data_dir.join("connections"),
                        &project,
                        cancellation,
                    )
                    .await?;
                let observation = self
                    .adapter
                    .observe_runner_connection(&identity, None, cancellation)
                    .await?;
                // A remote pairing code is one-shot. Publish the newly
                // validated connection identity before Runner/project
                // activation so a later readiness failure can retry this
                // same credential instead of forcing another pairing.
                self.config.topology = Some(topology.clone());
                self.store_identity(
                    &project,
                    &identity,
                    None,
                    Some(observation.client_id.clone()),
                )
                .await?;
                cancellation.check()?;
                (identity, true, observation.client_id)
            }
        };

        let server_deadline = Deadline::after(SERVER_READY_TIMEOUT);
        let server_status = match self
            .adapter
            .server_status_until(
                Some(&server_url),
                None,
                Some(&identity.user_token_file),
                cancellation,
                server_deadline,
            )
            .await
        {
            Ok(status) => status,
            Err(error) => {
                cancellation.check()?;
                if server_deadline.is_elapsed() {
                    return Err(readiness_timeout_error(
                        "server_unreachable",
                        "The existing WebCodex Server did not respond before the readiness deadline",
                        "Check the Server URL and network path, then retry.",
                    ));
                }
                return Err(error);
            }
        };
        cancellation.check()?;
        if !server_status.http_reachable {
            return Err(DesktopError::new(
                "server_unreachable",
                "The existing WebCodex Server is not reachable",
                "Check the Server URL and network path, then retry.",
            ));
        }
        let runner_deadline = Deadline::after(RUNNER_READY_TIMEOUT);
        let replacing_owned_runner = identity_replaced
            && process_is_active(self.process_snapshot(ProcessKind::LocalRunner).await);
        if replacing_owned_runner {
            self.stop_process_until(ProcessKind::LocalRunner, runner_deadline)
                .await;
            cancellation.check()?;
        }
        let runner_ready = if identity_replaced {
            false
        } else {
            self.adapter
                .runner_ready_until(&identity, cancellation, runner_deadline)
                .await
                .unwrap_or(false)
        };
        cancellation.check()?;
        let runner_started = if !runner_ready {
            if runner_deadline.is_elapsed() {
                return Err(readiness_timeout_error(
                    "runner_offline",
                    "Runner did not become connected",
                    "Check Server reachability and Runner diagnostics, then retry.",
                ));
            }
            let command = self.adapter.local_runner_command(&identity.runner_config)?;
            self.spawn_owned(ProcessKind::LocalRunner, command, false, cancellation)
                .await?;
            true
        } else {
            false
        };
        self.wait_for_runner(&identity, cancellation, runner_deadline, runner_started)
            .await?;
        identity = match self
            .adapter
            .activate_project(&identity, &runner_client_id, &project, cancellation)
            .await
        {
            Ok(identity) => identity,
            Err(error)
                if matches!(
                    error.code.as_str(),
                    "project_activation_capability_unavailable"
                        | "project_activation_restart_required"
                ) =>
            {
                if !can_refresh_legacy_runner(self.process_snapshot(ProcessKind::LocalRunner).await)
                {
                    return Err(DesktopError::new(
                        "project_activation_legacy_runner",
                        "This Runner needs to be refreshed before the new project can be activated",
                        "Refresh the Runner, then select this project again.",
                    ));
                }
                let legacy_identity = self
                    .adapter
                    .legacy_register_project(&identity, &runner_client_id, &project, cancellation)
                    .await?;
                let legacy_deadline = Deadline::after(RUNNER_READY_TIMEOUT);
                self.stop_process_until(ProcessKind::LocalRunner, legacy_deadline)
                    .await;
                if legacy_deadline.is_elapsed() {
                    return Err(readiness_timeout_error(
                        "runner_offline",
                        "Desktop could not refresh its legacy Runner",
                        "Retry project setup after checking Runner diagnostics.",
                    ));
                }
                let command = self
                    .adapter
                    .local_runner_command(&legacy_identity.runner_config)?;
                self.spawn_owned(ProcessKind::LocalRunner, command, false, cancellation)
                    .await?;
                self.wait_for_runner(&legacy_identity, cancellation, legacy_deadline, true)
                    .await?;
                legacy_identity
            }
            Err(error) => return Err(error),
        };
        self.wait_for_project(&identity, cancellation).await?;
        cancellation.check()?;
        self.config.topology = Some(topology);
        self.store_identity(&project, &identity, None, Some(runner_client_id))
            .await?;
        self.snapshot.readiness = aggregate_readiness(
            ServerReadiness::Ready,
            RunnerReadiness::Ready,
            if server_url.starts_with("https://") {
                // HTTPS proves transport shape, not that ChatGPT can reach and
                // authenticate to the MCP endpoint. D1 has no canonical remote
                // MCP/handoff probe, so keep this explicitly unverified.
                ExposureReadiness::Unknown
            } else {
                ExposureReadiness::Degraded
            },
            ProjectReadiness::Ready,
        );
        self.activity.push(
            ActivityEventKind::RemoteConnected,
            "desktop",
            ActivityLevel::Info,
            "This computer is connected to the existing WebCodex Server",
        );
        self.get_state().await
    }

    pub async fn start_quick_share(
        &mut self,
        project_path: &str,
        provider: &str,
        cancellation: &CancellationContext,
    ) -> DesktopResult<DesktopStateSnapshot> {
        cancellation.check()?;
        let project = self.adapter.inspect_project(project_path).await?;
        cancellation.check()?;
        let binaries = self.adapter.ensure_binaries(cancellation).await?.clone();
        self.snapshot.binaries = Some(binaries.info());
        if self
            .process_snapshot(ProcessKind::QuickShare)
            .await
            .is_some_and(|process| {
                matches!(
                    process.phase,
                    ProcessPhase::Starting | ProcessPhase::Running
                )
            })
        {
            return Err(DesktopError::new(
                "quick_share_already_running",
                "Quick Share is already running",
                "Stop the current share before starting another one.",
            ));
        }
        let deadline = Deadline::after(QUICK_SHARE_READY_TIMEOUT);
        let tunnel_proxy = effective_tunnel_proxy(&self.config.tunnel_proxy)?;
        let mut command = self.adapter.quick_share_command(
            Path::new(&project.path),
            provider,
            tunnel_proxy.url.as_deref(),
        )?;
        if provider == "openai" {
            self.tunnel_config.apply_to_command(&mut command)?;
        }
        if deadline.is_elapsed() {
            return Err(readiness_timeout_error(
                "quick_share_not_ready",
                "Quick Share did not reach verified readiness",
                "Check Activity and Tunnel prerequisites, then retry.",
            ));
        }
        let mut events = self
            .spawn_owned(ProcessKind::QuickShare, command, true, cancellation)
            .await?
            .expect("machine stdout requested");
        self.snapshot.topology = Some(RuntimeTopology {
            experience: Experience::QuickShare,
            server: ServerTopology::Local,
            runner: RunnerTopology::Local,
            exposure: match provider {
                "cloudflare" => Exposure::Cloudflare,
                "openai" => Exposure::OpenAiTunnel,
                _ => Exposure::None,
            },
            enrollment: Enrollment::ExistingProfile {
                profile: "temporary_share".to_string(),
            },
        });
        self.stage_project_scope(project.clone());
        self.snapshot.readiness = aggregate_readiness(
            ServerReadiness::Starting,
            RunnerReadiness::Connecting,
            ExposureReadiness::Starting,
            ProjectReadiness::Configured,
        );
        self.activity.push(
            ActivityEventKind::QuickShareStarting,
            "quick_share",
            ActivityLevel::Info,
            "Starting the temporary Quick Share runtime",
        );
        self.publish_snapshot();
        let event_wait = async {
            while let Some(value) = events.recv().await {
                match value.get("event").and_then(Value::as_str) {
                    Some("ready") => return Ok(Some(value)),
                    Some("machine_event_overflow") => return Err(value),
                    _ => {}
                }
            }
            Ok(None)
        };
        let event_result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                self.stop_process_until(
                    ProcessKind::QuickShare,
                    Deadline::at(deadline.cleanup_deadline(READINESS_CLEANUP_SLACK)),
                ).await;
                return Err(cancelled_error());
            }
            result = tokio::time::timeout_at(deadline.instant(), event_wait) => {
                result
            }
        };
        let event_value = match event_result {
            Ok(Ok(Some(value))) => value,
            Ok(Err(overflow)) => {
                self.stop_process_until(
                    ProcessKind::QuickShare,
                    Deadline::at(deadline.cleanup_deadline(READINESS_CLEANUP_SLACK)),
                )
                .await;
                return Err(machine_event_overflow_error(&overflow));
            }
            Ok(Ok(None)) | Err(_) => {
                let logs = self.process_logs(ProcessKind::QuickShare).await;
                self.stop_process_until(
                    ProcessKind::QuickShare,
                    Deadline::at(deadline.cleanup_deadline(READINESS_CLEANUP_SLACK)),
                )
                .await;
                return Err(DesktopError::new(
                    "quick_share_not_ready",
                    "Quick Share did not reach verified readiness",
                    "Check Activity and Tunnel prerequisites, then retry.",
                )
                .with_details(serde_json::json!({
                    "category": "readiness_timeout",
                    "diagnostic_lines": logs,
                })));
            }
        };
        let event: QuickShareReadyEvent = match serde_json::from_value(event_value) {
            Ok(event) => event,
            Err(_) => {
                self.stop_process(ProcessKind::QuickShare).await;
                return Err(DesktopError::new(
                    "webcodex_contract_invalid",
                    "Quick Share returned an invalid readiness event",
                    "Verify that Desktop and WebCodex binaries come from the same source baseline.",
                ));
            }
        };
        if event.event != "ready"
            || event.schema_version != 1
            || event.experience != "quick_share"
            || event.project.trim().is_empty()
            || event.exposure.kind.trim().is_empty()
        {
            self.stop_process(ProcessKind::QuickShare).await;
            return Err(DesktopError::new(
                "webcodex_contract_invalid",
                "Quick Share readiness identity is incomplete",
                "Update Desktop and WebCodex together.",
            ));
        }
        cancellation.check()?;
        let clipboard_required = event.connection.clipboard_contains != "none";
        let handoff_available = !clipboard_required || event.connection.clipboard_state == "copied";
        let ready_for_chatgpt = event.ready_for_chatgpt && handoff_available;
        self.snapshot.quick_share = Some(QuickShareState {
            provider: provider.to_string(),
            project: project.path.clone(),
            mcp_url: event.connection.mcp_url,
            clipboard_state: event.connection.clipboard_state,
            clipboard_contains: event.connection.clipboard_contains,
            ready_for_chatgpt,
        });
        let exposure_readiness = match event.exposure.state.as_str() {
            "remote_ready" if handoff_available => ExposureReadiness::RemoteReady,
            "remote_ready" => ExposureReadiness::Degraded,
            "local_ready" => ExposureReadiness::LocalReady,
            _ => ExposureReadiness::Unknown,
        };
        self.snapshot.readiness = aggregate_readiness(
            ServerReadiness::Ready,
            RunnerReadiness::Ready,
            exposure_readiness,
            ProjectReadiness::Ready,
        );
        if !handoff_available {
            self.snapshot.readiness.next_action_kind =
                Some(ReadinessNextActionKind::RestoreClipboardHandoff);
            self.snapshot.readiness.next_action =
                Some("Clipboard handoff is unavailable; restart Quick Share after clipboard access is restored.".to_string());
        }
        self.activity.push(
            ActivityEventKind::QuickShareReady,
            "quick_share",
            ActivityLevel::Info,
            "Quick Share reached verified readiness",
        );
        self.get_state().await
    }

    pub async fn stop_quick_share(
        &mut self,
        cancellation: &CancellationContext,
    ) -> DesktopResult<DesktopStateSnapshot> {
        self.stop_process(ProcessKind::QuickShare).await;
        self.snapshot.quick_share = None;
        self.snapshot.topology = self.config.topology.clone();
        self.snapshot.project = self.config.project.clone();
        self.activity.push(
            ActivityEventKind::QuickShareStopped,
            "quick_share",
            ActivityLevel::Info,
            "Quick Share stopped",
        );
        if self.config.runtime.is_some() {
            self.refresh_runtime_status(cancellation).await
        } else {
            self.snapshot.readiness = DesktopStateSnapshot::default().readiness;
            self.get_state().await
        }
    }

    pub async fn start_regular_tunnel(
        &mut self,
        cancellation: &CancellationContext,
    ) -> DesktopResult<DesktopStateSnapshot> {
        cancellation.check()?;
        if self.snapshot.quick_share.is_some() {
            return Err(DesktopError::new(
                "unsupported_topology",
                "Regular ChatGPT Connection is unavailable while Quick Share is active",
                "Stop Quick Share and start the Local Full Runtime first.",
            ));
        }
        let topology = self.config.topology.clone().ok_or_else(|| {
            DesktopError::new(
                "runtime_not_ready",
                "Local Full Runtime has not been configured",
                "Set up WebCodex on this computer before starting the secure tunnel.",
            )
        })?;
        if topology.experience != Experience::Full
            || !matches!(topology.server, ServerTopology::Local)
        {
            return Err(DesktopError::new(
                "unsupported_topology",
                "Regular OpenAI Secure Tunnel is only started for a local WebCodex Server",
                "Manage external exposure on the remote Server instead.",
            ));
        }
        if !self.tunnel_config.snapshot().is_configured() {
            return Err(DesktopError::new(
                "tunnel_unavailable",
                "OpenAI Secure Tunnel is not configured",
                "Save the Tunnel ID and API key in Desktop settings, then retry.",
            ));
        }
        if self
            .process_snapshot(ProcessKind::RegularTunnel)
            .await
            .is_some_and(|process| {
                matches!(
                    process.phase,
                    ProcessPhase::Starting | ProcessPhase::Running
                )
            })
        {
            return Err(DesktopError::new(
                "regular_tunnel_already_running",
                "OpenAI Secure Tunnel is already running",
                "Stop the current secure tunnel before starting another one.",
            ));
        }

        let current = self.refresh_runtime_status(cancellation).await?;
        if !current.readiness.runtime_ready {
            return Err(DesktopError::new(
                "runtime_not_ready",
                "Local WebCodex runtime is not ready",
                "Restore the Server and Runner readiness before starting the secure tunnel.",
            ));
        }
        let runtime = self.config.runtime.clone().ok_or_else(|| {
            DesktopError::new(
                "runtime_not_ready",
                "Local runtime identity is incomplete",
                "Run Local Setup again.",
            )
        })?;
        let env_file = runtime
            .server_env_file
            .filter(|path| path.is_file())
            .ok_or_else(|| {
                DesktopError::new(
                    "server_unavailable",
                    "Local Server configuration is unavailable",
                    "Run Local Setup again.",
                )
            })?;
        let deadline = Deadline::after(REGULAR_TUNNEL_READY_TIMEOUT);
        let tunnel_proxy = effective_tunnel_proxy(&self.config.tunnel_proxy)?;
        let mut command = self
            .adapter
            .regular_tunnel_command(&env_file, tunnel_proxy.url.as_deref())?;
        self.tunnel_config.apply_to_command(&mut command)?;
        if deadline.is_elapsed() {
            return Err(readiness_timeout_error(
                "tunnel_unavailable",
                "OpenAI Secure Tunnel did not reach verified readiness",
                "Check Activity and the canonical Tunnel prerequisites, then retry.",
            ));
        }
        let mut events = self
            .spawn_owned(ProcessKind::RegularTunnel, command, true, cancellation)
            .await?
            .expect("regular tunnel machine stdout requested");
        self.snapshot.regular_tunnel = Some(RegularTunnelState {
            provider: "openai".to_string(),
            status: RegularTunnelStatus::Starting,
            clipboard_state: "pending".to_string(),
            clipboard_contains: "tunnel_id".to_string(),
            ready_for_chatgpt: false,
        });
        self.snapshot.topology = effective_topology(self.config.topology.as_ref(), true);
        self.snapshot.readiness = aggregate_readiness(
            ServerReadiness::Ready,
            RunnerReadiness::Ready,
            ExposureReadiness::Starting,
            current.readiness.project.clone(),
        );
        self.activity.push(
            ActivityEventKind::RegularTunnelStarting,
            "regular_tunnel",
            ActivityLevel::Info,
            "Starting the regular OpenAI Secure Tunnel",
        );
        self.publish_snapshot();
        let event_wait = async {
            while let Some(value) = events.recv().await {
                match value.get("event").and_then(Value::as_str) {
                    Some("ready") => return Ok(Some(value)),
                    Some("machine_event_overflow") => return Err(value),
                    _ => {}
                }
            }
            Ok(None)
        };
        let event_result = tokio::select! {
            biased;
            _ = cancellation.cancelled() => {
                self.stop_process_until(
                    ProcessKind::RegularTunnel,
                    Deadline::at(deadline.cleanup_deadline(READINESS_CLEANUP_SLACK)),
                ).await;
                return Err(cancelled_error());
            }
            result = tokio::time::timeout_at(deadline.instant(), event_wait) => {
                result
            }
        };
        let event_value = match event_result {
            Ok(Ok(Some(value))) => value,
            Ok(Err(overflow)) => {
                self.stop_process_until(
                    ProcessKind::RegularTunnel,
                    Deadline::at(deadline.cleanup_deadline(READINESS_CLEANUP_SLACK)),
                )
                .await;
                self.snapshot.regular_tunnel = None;
                return Err(machine_event_overflow_error(&overflow));
            }
            Ok(Ok(None)) | Err(_) => {
                let logs = self.process_logs(ProcessKind::RegularTunnel).await;
                self.stop_process_until(
                    ProcessKind::RegularTunnel,
                    Deadline::at(deadline.cleanup_deadline(READINESS_CLEANUP_SLACK)),
                )
                .await;
                self.snapshot.regular_tunnel = Some(RegularTunnelState {
                    provider: "openai".to_string(),
                    status: RegularTunnelStatus::Error,
                    clipboard_state: "unavailable".to_string(),
                    clipboard_contains: "tunnel_id".to_string(),
                    ready_for_chatgpt: false,
                });
                self.snapshot.readiness = aggregate_readiness(
                    ServerReadiness::Ready,
                    RunnerReadiness::Ready,
                    ExposureReadiness::Error,
                    current.readiness.project.clone(),
                );
                apply_regular_tunnel_next_action(&mut self.snapshot, &ExposureReadiness::Error);
                return Err(DesktopError::new(
                    "tunnel_unavailable",
                    "OpenAI Secure Tunnel did not reach verified readiness",
                    "Check Activity and the canonical Tunnel prerequisites, then retry.",
                )
                .with_details(serde_json::json!({
                    "category": "readiness_timeout",
                    "diagnostic_lines": logs,
                })));
            }
        };
        let event: RegularTunnelReadyEvent = match serde_json::from_value(event_value) {
            Ok(event) => event,
            Err(_) => {
                self.stop_process(ProcessKind::RegularTunnel).await;
                self.snapshot.regular_tunnel = None;
                return Err(DesktopError::new(
                    "webcodex_contract_invalid",
                    "Regular Tunnel returned an invalid readiness event",
                    "Verify that Desktop and WebCodex binaries come from the same source baseline.",
                ));
            }
        };
        if event.event != "ready"
            || event.schema_version != 1
            || event.provider != "openai"
            || event.connection.kind != "openai_tunnel"
            || event.connection.clipboard_contains != "tunnel_id"
        {
            self.stop_process(ProcessKind::RegularTunnel).await;
            self.snapshot.regular_tunnel = None;
            return Err(DesktopError::new(
                "webcodex_contract_invalid",
                "Regular Tunnel readiness identity is incomplete",
                "Update Desktop and WebCodex together.",
            ));
        }
        cancellation.check()?;
        // The schema-v1 ready event is emitted only after the daemon is ready.
        // Its ready_for_chatgpt flag additionally requires CLI clipboard success;
        // Desktop can also hand off the same non-secret ID through its copy field.
        let id_available = self.tunnel_config.snapshot().effective_tunnel_id.is_some();
        let handoff_available = event.connection.clipboard_state == "copied" || id_available;
        self.snapshot.regular_tunnel = Some(RegularTunnelState {
            provider: event.provider,
            status: RegularTunnelStatus::Ready,
            clipboard_state: event.connection.clipboard_state,
            clipboard_contains: event.connection.clipboard_contains,
            ready_for_chatgpt: (event.ready_for_chatgpt || id_available) && handoff_available,
        });
        self.config.preferred_connection = Some(RegularConnectionPreference::OpenAiTunnel);
        self.save_config().await?;
        // The daemon and clipboard handoff prove only Desktop-managed Tunnel
        // readiness. Actual ChatGPT use is observed independently from canonical
        // Window activity and projected through `chatgpt_activity` on refresh.
        let exposure = if handoff_available {
            ExposureReadiness::LocalReady
        } else {
            ExposureReadiness::Degraded
        };
        self.snapshot.readiness = aggregate_readiness(
            ServerReadiness::Ready,
            RunnerReadiness::Ready,
            exposure.clone(),
            current.readiness.project.clone(),
        );
        apply_regular_tunnel_next_action(&mut self.snapshot, &exposure);
        self.snapshot.topology = effective_topology(self.config.topology.as_ref(), true);
        self.snapshot.project = project_snapshot(&self.config);
        self.activity.push(
            ActivityEventKind::RegularTunnelReady,
            "regular_tunnel",
            ActivityLevel::Info,
            "Regular OpenAI Secure Tunnel reached local handoff readiness",
        );
        self.get_state().await
    }

    pub async fn stop_regular_tunnel(
        &mut self,
        cancellation: &CancellationContext,
    ) -> DesktopResult<DesktopStateSnapshot> {
        self.stop_process(ProcessKind::RegularTunnel).await;
        self.snapshot.regular_tunnel = None;
        self.snapshot.topology = self.config.topology.clone();
        self.config.preferred_connection = Some(RegularConnectionPreference::NoChatGpt);
        self.save_config().await?;
        self.activity.push(
            ActivityEventKind::RegularTunnelStopped,
            "regular_tunnel",
            ActivityLevel::Info,
            "Regular OpenAI Secure Tunnel stopped",
        );
        if self.config.runtime.is_some() {
            self.refresh_runtime_status(cancellation).await
        } else {
            self.get_state().await
        }
    }

    pub async fn stop_local_runtime(
        &mut self,
        _cancellation: &CancellationContext,
    ) -> DesktopResult<DesktopStateSnapshot> {
        self.stop_process(ProcessKind::RegularTunnel).await;
        self.snapshot.regular_tunnel = None;
        self.stop_process(ProcessKind::LocalRunner).await;
        self.config.runtime_autostart = Some(false);
        self.save_config().await?;
        self.stop_process(ProcessKind::LocalServer).await;
        self.snapshot.topology = self.config.topology.clone();
        let exposure = exposure_readiness(self.config.topology.as_ref());
        self.snapshot.readiness = aggregate_readiness(
            ServerReadiness::Stopped,
            RunnerReadiness::Stopped,
            exposure,
            self.config
                .project
                .as_ref()
                .map(|_| ProjectReadiness::Configured)
                .unwrap_or(ProjectReadiness::None),
        );
        self.activity.push(
            ActivityEventKind::RuntimeStopped,
            "desktop",
            ActivityLevel::Info,
            "Desktop-managed local runtime stopped",
        );
        self.get_state().await
    }

    async fn process_snapshot(&self, kind: ProcessKind) -> Option<crate::chadex_core::runtime_compat::process::ProcessSnapshot> {
        self.supervisor.lock().await.snapshot(kind)
    }

    async fn process_logs(&self, kind: ProcessKind) -> Vec<String> {
        self.supervisor.lock().await.logs(kind)
    }

    async fn spawn_owned(
        &self,
        kind: ProcessKind,
        command: std::process::Command,
        machine_stdout: bool,
        cancellation: &CancellationContext,
    ) -> DesktopResult<Option<MachineEventReceiver>> {
        cancellation.check()?;
        let mut supervisor = self.supervisor.lock().await;
        cancellation.check()?;
        supervisor.spawn_owned(kind, command, machine_stdout).await
    }

    async fn stop_process(&self, kind: ProcessKind) {
        self.supervisor.lock().await.stop(kind).await;
    }

    async fn stop_process_until(&self, kind: ProcessKind, deadline: Deadline) {
        self.supervisor
            .lock()
            .await
            .stop_until(kind, deadline)
            .await;
    }

    async fn wait_for_server(
        &mut self,
        server_url: &str,
        env_file: Option<&Path>,
        token_file: Option<&Path>,
        cancellation: &CancellationContext,
        deadline: Deadline,
        cleanup_owned_process: bool,
    ) -> DesktopResult<()> {
        let wait_started = tokio::time::Instant::now();
        loop {
            cancellation.check()?;
            if deadline.is_elapsed() {
                self.cleanup_readiness_process(
                    ProcessKind::LocalServer,
                    deadline,
                    cleanup_owned_process,
                )
                .await;
                return Err(readiness_timeout_error(
                    "server_unreachable",
                    "WebCodex Service did not become ready",
                    "Check the local Service diagnostics and retry.",
                ));
            }
            if let Some(process) = self.process_snapshot(ProcessKind::LocalServer).await {
                if matches!(process.phase, ProcessPhase::Exited | ProcessPhase::Failed) {
                    self.cleanup_readiness_process(
                        ProcessKind::LocalServer,
                        deadline,
                        cleanup_owned_process,
                    )
                    .await;
                    return Err(DesktopError::new(
                        "server_start_failed",
                        "The Desktop-owned WebCodex Server exited during startup",
                        "Open Activity for safe diagnostics and retry.",
                    ));
                }
            }
            if self
                .adapter
                .server_status_until(
                    Some(server_url),
                    env_file,
                    token_file,
                    cancellation,
                    deadline,
                )
                .await
                .is_ok_and(|status| status.http_reachable)
            {
                return Ok(());
            }
            cancellation.check()?;
            if deadline.is_elapsed() {
                self.cleanup_readiness_process(
                    ProcessKind::LocalServer,
                    deadline,
                    cleanup_owned_process,
                )
                .await;
                return Err(readiness_timeout_error(
                    "server_unreachable",
                    "WebCodex Service did not become ready",
                    "Check the local Service diagnostics and retry.",
                ));
            }
            sleep_or_cancel_until(
                startup_readiness_poll_interval(wait_started.elapsed()),
                cancellation,
                deadline,
            )
            .await?;
        }
    }

    async fn wait_for_runner(
        &mut self,
        identity: &ProjectRuntimeIdentity,
        cancellation: &CancellationContext,
        deadline: Deadline,
        cleanup_owned_process: bool,
    ) -> DesktopResult<()> {
        let wait_started = tokio::time::Instant::now();
        loop {
            cancellation.check()?;
            if deadline.is_elapsed() {
                self.cleanup_readiness_process(
                    ProcessKind::LocalRunner,
                    deadline,
                    cleanup_owned_process,
                )
                .await;
                return Err(readiness_timeout_error(
                    "runner_offline",
                    "Runner did not become connected",
                    "Check Server reachability and Runner diagnostics, then retry.",
                ));
            }
            if let Some(process) = self.process_snapshot(ProcessKind::LocalRunner).await {
                if matches!(process.phase, ProcessPhase::Exited | ProcessPhase::Failed) {
                    self.cleanup_readiness_process(
                        ProcessKind::LocalRunner,
                        deadline,
                        cleanup_owned_process,
                    )
                    .await;
                    return Err(DesktopError::new(
                        "runner_offline",
                        "The Desktop-owned Runner exited while connecting",
                        "Open Activity for safe diagnostics and retry.",
                    ));
                }
            }
            if self
                .adapter
                .runner_ready_until(identity, cancellation, deadline)
                .await
                .unwrap_or(false)
            {
                return Ok(());
            }
            cancellation.check()?;
            if deadline.is_elapsed() {
                self.cleanup_readiness_process(
                    ProcessKind::LocalRunner,
                    deadline,
                    cleanup_owned_process,
                )
                .await;
                return Err(readiness_timeout_error(
                    "runner_offline",
                    "Runner did not become connected",
                    "Check Server reachability and Runner diagnostics, then retry.",
                ));
            }
            sleep_or_cancel_until(
                startup_readiness_poll_interval(wait_started.elapsed()),
                cancellation,
                deadline,
            )
            .await?;
        }
    }

    async fn wait_for_project(
        &mut self,
        identity: &ProjectRuntimeIdentity,
        cancellation: &CancellationContext,
    ) -> DesktopResult<()> {
        let deadline = Deadline::after(PROJECT_READY_TIMEOUT);
        loop {
            cancellation.check()?;
            if deadline.is_elapsed() {
                return Err(project_not_loaded_error());
            }
            if self
                .adapter
                .project_ready_until(identity, cancellation, deadline)
                .await
                .unwrap_or(false)
            {
                return Ok(());
            }
            cancellation.check()?;
            if deadline.is_elapsed() {
                return Err(project_not_loaded_error());
            }
            sleep_or_cancel_until(POLL_INTERVAL, cancellation, deadline).await?;
        }
    }

    async fn cleanup_readiness_process(
        &self,
        kind: ProcessKind,
        deadline: Deadline,
        cleanup_owned_process: bool,
    ) {
        if cleanup_owned_process {
            self.stop_process_until(
                kind,
                Deadline::at(deadline.cleanup_deadline(READINESS_CLEANUP_SLACK)),
            )
            .await;
        }
    }

    async fn store_identity(
        &mut self,
        project: &ProjectSelection,
        identity: &ProjectRuntimeIdentity,
        server_env_file: Option<PathBuf>,
        runner_client_id: Option<String>,
    ) -> DesktopResult<()> {
        let mut project = project.clone();
        project.runtime_project_id = Some(identity.runtime_project_id.clone());
        self.config.project = Some(project.clone());
        self.config.runtime = Some(StoredRuntime {
            server_url: identity.server_url.clone(),
            server_env_file,
            runner_config: Some(identity.runner_config.clone()),
            user_token_file: Some(identity.user_token_file.clone()),
            runner_client_id,
            project_id: Some(identity.project_id.clone()),
            runtime_project_id: Some(identity.runtime_project_id.clone()),
        });
        self.snapshot.project = Some(project);
        self.save_config().await
    }

    async fn save_config(&self) -> DesktopResult<()> {
        tokio::fs::create_dir_all(&self.data_dir)
            .await
            .map_err(|_| {
                DesktopError::new(
                    "desktop_state_unavailable",
                    "Desktop cannot create its app-data directory",
                    "Check local filesystem permissions and retry.",
                )
            })?;
        let encoded = serde_json::to_vec_pretty(&self.config).map_err(|_| {
            DesktopError::new(
                "desktop_state_invalid",
                "Desktop could not encode its non-secret runtime state",
                "Retry the setup operation.",
            )
        })?;
        let config_path = self.config_path.clone();
        tokio::task::spawn_blocking(move || save_config_atomically(&config_path, &encoded))
            .await
            .map_err(|_| desktop_state_unavailable("Desktop state persistence worker stopped"))??;
        Ok(())
    }
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
