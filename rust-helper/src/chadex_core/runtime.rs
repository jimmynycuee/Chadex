use super::activity::RuntimeActivityEntry;
use super::backend::RuntimeBackendApi;
use super::tunnel::RuntimeTunnelTarget;
use super::ChadexResult;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeProject {
    pub path: String,
    pub allowed_root: String,
    pub is_git_repository: bool,
}

/// Bounded lifecycle facts only; no command, output, or inferred execution purpose.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeMascotJob {
    pub job_id: String,
    pub status: String,
    pub started_at_ms: Option<u64>,
    pub finished_at_ms: Option<u64>,
    pub exit_code: Option<i32>,
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
    /// Launch warm-up work the user did not request; never shown as user work.
    pub background: bool,
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
    pub runtime_autostart: bool,
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

    pub async fn resume_saved_runtime_background(&self) -> ChadexResult<RuntimeSnapshot> {
        self.backend.resume_saved_runtime_background().await
    }

    pub async fn cancel_background_operation(&self) {
        self.backend.cancel_background_operation().await
    }

    pub async fn refresh_runtime_status(&self) -> ChadexResult<RuntimeSnapshot> {
        self.backend.refresh_runtime_status().await
    }

    pub async fn observe_mascot_jobs(&self, project_path: &str) -> Option<Vec<RuntimeMascotJob>> {
        self.backend.observe_mascot_jobs(project_path).await
    }

    pub async fn skill_catalog(&self, project_path: &str) -> ChadexResult<Value> {
        self.backend.skill_catalog(project_path).await
    }

    pub async fn external_skill_roots(&self) -> ChadexResult<Value> {
        self.backend.external_skill_roots().await
    }

    pub async fn set_external_skill_roots(
        &self,
        roots: Vec<std::path::PathBuf>,
        script_roots: Vec<std::path::PathBuf>,
        expected_revision: String,
        verify_project_path: Option<String>,
    ) -> ChadexResult<Value> {
        self.backend
            .set_external_skill_roots(roots, script_roots, expected_revision, verify_project_path)
            .await
    }

    pub async fn skill_inventory(&self, project_path: &str) -> ChadexResult<Value> {
        self.backend.skill_inventory(project_path).await
    }

    pub async fn skill_definition(
        &self,
        project_path: &str,
        skill_id: &str,
        definition_revision: &str,
        package_revision: Option<&str>,
    ) -> ChadexResult<Value> {
        self.backend
            .skill_definition(
                project_path,
                skill_id,
                definition_revision,
                package_revision,
            )
            .await
    }

    pub async fn create_project_skill(
        &self,
        project_path: &str,
        skill_key: &str,
        content: &str,
    ) -> ChadexResult<Value> {
        self.backend
            .create_project_skill(project_path, skill_key, content)
            .await
    }

    pub async fn install_skill(
        &self,
        project_path: &str,
        skill_key: &str,
        artifact_path: &str,
    ) -> ChadexResult<Value> {
        self.backend
            .install_skill(project_path, skill_key, artifact_path)
            .await
    }

    pub async fn activate_skill(
        &self,
        project_path: &str,
        skill_key: &str,
        package_revision: &str,
        state_revision: &str,
    ) -> ChadexResult<Value> {
        self.backend
            .activate_skill(project_path, skill_key, package_revision, state_revision)
            .await
    }

    pub async fn deactivate_skill(
        &self,
        project_path: &str,
        skill_key: &str,
        state_revision: &str,
    ) -> ChadexResult<Value> {
        self.backend
            .deactivate_skill(project_path, skill_key, state_revision)
            .await
    }

    pub async fn memory_catalog(&self, project_path: &str) -> ChadexResult<Value> {
        self.backend.memory_catalog(project_path).await
    }

    pub async fn memory_read(
        &self,
        project_path: &str,
        memory_key: &str,
        expected_revision: Option<&str>,
    ) -> ChadexResult<Value> {
        self.backend
            .memory_read(project_path, memory_key, expected_revision)
            .await
    }

    pub async fn memory_set(
        &self,
        project_path: &str,
        memory_key: &str,
        summary: &str,
        body: &str,
        priority: &str,
        bootstrap: bool,
        tags: &[String],
        expected_revision: Option<&str>,
    ) -> ChadexResult<Value> {
        self.backend
            .memory_set(
                project_path,
                memory_key,
                summary,
                body,
                priority,
                bootstrap,
                tags,
                expected_revision,
            )
            .await
    }

    pub async fn memory_delete(
        &self,
        project_path: &str,
        memory_key: &str,
        expected_revision: &str,
    ) -> ChadexResult<Value> {
        self.backend
            .memory_delete(project_path, memory_key, expected_revision)
            .await
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
