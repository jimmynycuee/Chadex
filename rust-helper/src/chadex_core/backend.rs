use super::activity::RuntimeActivityEntry;
use super::adapters::runtime_backend::RuntimeBackendAdapter;
use super::runtime::{RuntimeMascotJob, RuntimeProject, RuntimeProxyMode, RuntimeSnapshot};
use super::tunnel::RuntimeTunnelTarget;
use super::ChadexResult;
use serde_json::Value;
use std::path::PathBuf;

pub(crate) struct RuntimeBackendApi {
    adapter: RuntimeBackendAdapter,
}

impl RuntimeBackendApi {
    pub(crate) fn new(data_dir: PathBuf, resource_dir: PathBuf) -> ChadexResult<Self> {
        Ok(Self {
            adapter: RuntimeBackendAdapter::new(data_dir, resource_dir)?,
        })
    }

    pub(crate) fn snapshot(&self) -> RuntimeSnapshot {
        self.adapter.snapshot()
    }

    pub(crate) fn activity(&self) -> Vec<RuntimeActivityEntry> {
        self.adapter.activity()
    }

    pub(crate) async fn inspect_project(&self, path: &str) -> ChadexResult<RuntimeProject> {
        self.adapter.inspect_project(path).await
    }

    pub(crate) async fn activate_local_project(&self, path: &str) -> ChadexResult<RuntimeSnapshot> {
        self.adapter.activate_local_project(path).await
    }

    pub(crate) async fn activate_local_project_background(
        &self,
        path: &str,
    ) -> ChadexResult<RuntimeSnapshot> {
        self.adapter.activate_local_project_background(path).await
    }

    pub(crate) async fn configure_local_setup(
        &self,
        project_path: Option<&str>,
    ) -> ChadexResult<RuntimeSnapshot> {
        self.adapter.configure_local_setup(project_path).await
    }

    pub(crate) async fn resume_saved_runtime(&self) -> ChadexResult<RuntimeSnapshot> {
        self.adapter.resume_saved_runtime().await
    }

    pub(crate) async fn resume_saved_runtime_background(&self) -> ChadexResult<RuntimeSnapshot> {
        self.adapter.resume_saved_runtime_background().await
    }

    pub(crate) async fn cancel_background_operation(&self) {
        self.adapter.cancel_background_operation().await
    }

    pub(crate) async fn refresh_runtime_status(&self) -> ChadexResult<RuntimeSnapshot> {
        self.adapter.refresh_runtime_status().await
    }

    pub(crate) async fn observe_mascot_jobs(&self, project_path: &str) -> Option<Vec<RuntimeMascotJob>> {
        self.adapter.observe_mascot_jobs(project_path).await
    }

    pub(crate) async fn skill_catalog(&self, project_path: &str) -> ChadexResult<Value> {
        self.adapter.skill_catalog(project_path).await
    }

    pub(crate) async fn external_skill_roots(&self) -> ChadexResult<Value> {
        self.adapter.external_skill_roots().await
    }

    pub(crate) async fn set_external_skill_roots(
        &self,
        roots: Vec<std::path::PathBuf>,
        script_roots: Vec<std::path::PathBuf>,
        expected_revision: String,
        verify_project_path: Option<String>,
    ) -> ChadexResult<Value> {
        self.adapter
            .set_external_skill_roots(roots, script_roots, expected_revision, verify_project_path)
            .await
    }

    pub(crate) async fn skill_inventory(&self, project_path: &str) -> ChadexResult<Value> {
        self.adapter.skill_inventory(project_path).await
    }

    pub(crate) async fn skill_definition(
        &self,
        project_path: &str,
        skill_id: &str,
        definition_revision: &str,
        package_revision: Option<&str>,
    ) -> ChadexResult<Value> {
        self.adapter
            .skill_definition(
                project_path,
                skill_id,
                definition_revision,
                package_revision,
            )
            .await
    }

    pub(crate) async fn create_project_skill(
        &self,
        project_path: &str,
        skill_key: &str,
        content: &str,
    ) -> ChadexResult<Value> {
        self.adapter
            .create_project_skill(project_path, skill_key, content)
            .await
    }

    pub(crate) async fn install_skill(
        &self,
        project_path: &str,
        skill_key: &str,
        artifact_path: &str,
    ) -> ChadexResult<Value> {
        self.adapter
            .install_skill(project_path, skill_key, artifact_path)
            .await
    }

    pub(crate) async fn activate_skill(
        &self,
        project_path: &str,
        skill_key: &str,
        package_revision: &str,
        state_revision: &str,
    ) -> ChadexResult<Value> {
        self.adapter
            .activate_skill(project_path, skill_key, package_revision, state_revision)
            .await
    }

    pub(crate) async fn deactivate_skill(
        &self,
        project_path: &str,
        skill_key: &str,
        state_revision: &str,
    ) -> ChadexResult<Value> {
        self.adapter
            .deactivate_skill(project_path, skill_key, state_revision)
            .await
    }

    pub(crate) async fn memory_catalog(&self, project_path: &str) -> ChadexResult<Value> {
        self.adapter.memory_catalog(project_path).await
    }

    pub(crate) async fn memory_read(
        &self,
        project_path: &str,
        memory_key: &str,
        expected_revision: Option<&str>,
    ) -> ChadexResult<Value> {
        self.adapter
            .memory_read(project_path, memory_key, expected_revision)
            .await
    }

    pub(crate) async fn memory_set(
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
        self.adapter
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

    pub(crate) async fn memory_delete(
        &self,
        project_path: &str,
        memory_key: &str,
        expected_revision: &str,
    ) -> ChadexResult<Value> {
        self.adapter
            .memory_delete(project_path, memory_key, expected_revision)
            .await
    }

    pub(crate) async fn stop_local_runtime(&self) -> ChadexResult<RuntimeSnapshot> {
        self.adapter.stop_local_runtime().await
    }

    pub(crate) async fn tunnel_target(&self) -> ChadexResult<RuntimeTunnelTarget> {
        self.adapter.tunnel_target().await
    }

    pub(crate) async fn update_tunnel_proxy(
        &self,
        mode: RuntimeProxyMode,
        custom_url: Option<&str>,
    ) -> ChadexResult<RuntimeSnapshot> {
        self.adapter.update_tunnel_proxy(mode, custom_url).await
    }

    pub(crate) fn cancel_operation(&self, operation_id: &str) -> ChadexResult<RuntimeSnapshot> {
        self.adapter.cancel_operation(operation_id)
    }

    pub(crate) async fn cancel_chadex_task(
        &self,
        project: &str,
        task_id: &str,
    ) -> ChadexResult<Value> {
        self.adapter.cancel_chadex_task(project, task_id).await
    }

    pub(crate) async fn shutdown(&self) {
        self.adapter.shutdown().await;
    }
}
