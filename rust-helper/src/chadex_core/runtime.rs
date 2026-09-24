use super::activity::RuntimeActivityEntry;
use super::backend::RuntimeBackendApi;
use super::tunnel::RuntimeTunnelTarget;
use super::ChadexResult;
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeProject {
    pub path: String,
    pub allowed_root: String,
    pub is_git_repository: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeOperationPhase {
    Running,
    Cancelling,
}

impl RuntimeOperationPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Cancelling => "cancelling",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeOperation {
    pub id: String,
    pub kind: &'static str,
    pub phase: RuntimeOperationPhase,
    pub started_at_ms: u64,
    pub cancellable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeReadiness {
    pub runtime_ready: bool,
    pub needs_attention: bool,
    pub summary: String,
    pub next_action: Option<String>,
    pub summary_kind: &'static str,
    pub server: &'static str,
    pub runner: &'static str,
    pub exposure: &'static str,
    pub project: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSnapshot {
    pub runtime_configured: bool,
    pub readiness: RuntimeReadiness,
    pub project: Option<RuntimeProject>,
    pub current_operation: Option<RuntimeOperation>,
    pub activity_sequence: u64,
    pub tunnel_proxy_effective_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeProxyMode {
    Auto,
    Direct,
    Custom,
}

pub struct ChadexRuntimeCore {
    backend: RuntimeBackendApi,
}

impl ChadexRuntimeCore {
    pub fn new(data_dir: PathBuf, resource_dir: PathBuf) -> ChadexResult<Self> {
        Ok(Self {
            backend: RuntimeBackendApi::new(data_dir, resource_dir)?,
        })
    }

    pub fn snapshot(&self) -> RuntimeSnapshot {
        self.backend.snapshot()
    }

    pub fn activity(&self) -> Vec<RuntimeActivityEntry> {
        self.backend.activity()
    }

    pub async fn inspect_project(&self, path: &str) -> ChadexResult<RuntimeProject> {
        self.backend.inspect_project(path).await
    }

    pub async fn activate_local_project(&self, path: &str) -> ChadexResult<RuntimeSnapshot> {
        self.backend.activate_local_project(path).await
    }

    pub async fn configure_local_setup(
        &self,
        project_path: Option<&str>,
    ) -> ChadexResult<RuntimeSnapshot> {
        self.backend.configure_local_setup(project_path).await
    }

    pub async fn resume_saved_runtime(&self) -> ChadexResult<RuntimeSnapshot> {
        self.backend.resume_saved_runtime().await
    }

    pub async fn refresh_runtime_status(&self) -> ChadexResult<RuntimeSnapshot> {
        self.backend.refresh_runtime_status().await
    }

    pub async fn stop_local_runtime(&self) -> ChadexResult<RuntimeSnapshot> {
        self.backend.stop_local_runtime().await
    }

    pub async fn tunnel_target(&self) -> ChadexResult<RuntimeTunnelTarget> {
        self.backend.tunnel_target().await
    }

    pub async fn update_tunnel_proxy(
        &self,
        mode: RuntimeProxyMode,
        custom_url: Option<&str>,
    ) -> ChadexResult<RuntimeSnapshot> {
        self.backend.update_tunnel_proxy(mode, custom_url).await
    }

    pub fn cancel_operation(&self, operation_id: &str) -> ChadexResult<RuntimeSnapshot> {
        self.backend.cancel_operation(operation_id)
    }

    pub async fn cancel_chadex_task(
        &self,
        project: &str,
        task_id: &str,
    ) -> ChadexResult<Value> {
        self.backend.cancel_chadex_task(project, task_id).await
    }

    pub async fn shutdown(&self) {
        self.backend.shutdown().await;
    }
}
