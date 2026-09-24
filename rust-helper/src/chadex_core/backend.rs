use super::activity::RuntimeActivityEntry;
use super::adapters::runtime_backend::RuntimeBackendAdapter;
use super::runtime::{RuntimeProject, RuntimeProxyMode, RuntimeSnapshot};
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

    pub(crate) async fn configure_local_setup(
        &self,
        project_path: Option<&str>,
    ) -> ChadexResult<RuntimeSnapshot> {
        self.adapter.configure_local_setup(project_path).await
    }

    pub(crate) async fn resume_saved_runtime(&self) -> ChadexResult<RuntimeSnapshot> {
        self.adapter.resume_saved_runtime().await
    }

    pub(crate) async fn refresh_runtime_status(&self) -> ChadexResult<RuntimeSnapshot> {
        self.adapter.refresh_runtime_status().await
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
