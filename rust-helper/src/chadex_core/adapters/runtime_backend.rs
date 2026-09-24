use super::runtime_translation::{map_compat_activity, map_compat_error, present_readiness};
use crate::chadex_core::activity::RuntimeActivityEntry;
use crate::chadex_core::runtime::{
    RuntimeOperation, RuntimeOperationPhase, RuntimeProject, RuntimeProxyMode, RuntimeReadiness,
    RuntimeSnapshot,
};
use crate::chadex_core::tunnel::RuntimeTunnelTarget;
use crate::chadex_core::{ChadexError, ChadexResult};
use crate::chadex_core::runtime_compat::error::{DesktopError, DesktopResult};
use crate::chadex_core::runtime_compat::models::{
    DesktopOperationPhase, DesktopStateSnapshot, ExposureReadiness, ProjectReadiness,
    ProjectSelection, ReadinessSummaryKind, RunnerReadiness, ServerReadiness, TunnelProxyMode,
};
use crate::chadex_core::runtime_compat::operation::{
    cancelled_error, CancellationContext, CancellationSignal,
};
use crate::chadex_core::runtime_compat::state::{
    ChadexProjectActivationObservation, ChadexProjectActivationTarget, ChadexRuntimeProbeTarget,
    RuntimeStateManager,
};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use url::Url;
use zeroize::Zeroizing;

const RUNTIME_PROBE_CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const RUNTIME_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const RUNTIME_PROBE_MAX_TOKEN_BYTES: u64 = 16 * 1024;
const RUNTIME_PROBE_MAX_RESPONSE_BYTES: usize = 128 * 1024;
const RUNTIME_ACTIVATION_CONFIG_MAX_BYTES: u64 = 256 * 1024;
const FRESH_RUNTIME_READY_WINDOW: Duration = Duration::from_millis(150);
const FRESH_RUNTIME_READY_POLL_INTERVAL: Duration = Duration::from_millis(10);

#[derive(Debug, Default, Deserialize)]
struct FastActivationPolicy {
    #[serde(default)]
    allowed_roots: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
struct FastActivationRunnerConfig {
    server_url: String,
    client_id: String,
    #[serde(default)]
    policy: FastActivationPolicy,
}

#[derive(Debug, Deserialize)]
struct OperatorToolResult {
    success: bool,
    #[serde(default)]
    output: Value,
    #[serde(default)]
    error: Option<String>,
}

pub(crate) struct RuntimeBackendAdapter {
    app: Arc<RuntimeStateManager>,
    probe_client: Client,
}

impl RuntimeBackendAdapter {
    pub(crate) fn new(data_dir: PathBuf, resource_dir: PathBuf) -> ChadexResult<Self> {
        let app = RuntimeStateManager::new_for_chadex_backend(data_dir, resource_dir)
            .map_err(map_desktop_error)?;
        let probe_client = Client::builder()
            .no_proxy()
            .connect_timeout(RUNTIME_PROBE_CONNECT_TIMEOUT)
            .timeout(RUNTIME_PROBE_TIMEOUT)
            .build()
            .map_err(|_| {
                ChadexError::new(
                    "runtime_probe_unavailable",
                    "Chadex could not prepare the local runtime observer",
                    "Restart Chadex and retry.",
                )
            })?;
        Ok(Self {
            app: Arc::new(app),
            probe_client,
        })
    }

    pub(crate) fn snapshot(&self) -> RuntimeSnapshot {
        map_snapshot(self.app.get_state())
    }

    pub(crate) fn activity(&self) -> Vec<RuntimeActivityEntry> {
        self.app.activity().into_iter().map(map_activity).collect()
    }

    pub(crate) async fn inspect_project(&self, path: &str) -> ChadexResult<RuntimeProject> {
        self.app
            .inspect_project(path)
            .await
            .map(map_project)
            .map_err(map_desktop_error)
    }

    pub(crate) async fn activate_local_project(&self, path: &str) -> ChadexResult<RuntimeSnapshot> {
        let client = self.probe_client.clone();
        self.app
            .activate_local_project_with_chadex_fast_path(path, move |target, cancellation| {
                let client = client.clone();
                async move { fast_activate_project(&client, &target, &cancellation).await }
            })
            .await
            .map(map_snapshot)
            .map_err(map_desktop_error)
    }

    pub(crate) async fn configure_local_setup(
        &self,
        project_path: Option<&str>,
    ) -> ChadexResult<RuntimeSnapshot> {
        let client = self.probe_client.clone();
        self.app
            .configure_local_setup_with_chadex_fast_path(
                project_path,
                move |target, cancellation| {
                    let client = client.clone();
                    async move {
                        wait_for_fresh_runtime_project(&client, &target, &cancellation).await
                    }
                },
            )
            .await
            .map(map_snapshot)
            .map_err(map_desktop_error)
    }

    pub(crate) async fn resume_saved_runtime(&self) -> ChadexResult<RuntimeSnapshot> {
        if let Some(snapshot) = self.try_fast_runtime_observation().await {
            return Ok(snapshot);
        }
        self.app
            .resume_saved_runtime()
            .await
            .map(map_snapshot)
            .map_err(map_desktop_error)
    }

    pub(crate) async fn refresh_runtime_status(&self) -> ChadexResult<RuntimeSnapshot> {
        if let Some(snapshot) = self.try_fast_runtime_observation().await {
            return Ok(snapshot);
        }
        self.app
            .refresh_runtime_status()
            .await
            .map(map_snapshot)
            .map_err(map_desktop_error)
    }

    pub(crate) async fn stop_local_runtime(&self) -> ChadexResult<RuntimeSnapshot> {
        self.app
            .stop_local_runtime()
            .await
            .map(map_snapshot)
            .map_err(map_desktop_error)
    }

    pub(crate) async fn tunnel_target(&self) -> ChadexResult<RuntimeTunnelTarget> {
        self.app
            .chadex_runtime_tunnel_target()
            .await
            .map(|target| RuntimeTunnelTarget {
                server_url: target.server_url,
                server_env_file: target.server_env_file,
                bootstrap_token_env: "WEBCODEX_TOKEN".to_string(),
            })
            .map_err(map_desktop_error)
    }

    pub(crate) async fn update_tunnel_proxy(
        &self,
        mode: RuntimeProxyMode,
        custom_url: Option<&str>,
    ) -> ChadexResult<RuntimeSnapshot> {
        let mode = match mode {
            RuntimeProxyMode::Auto => TunnelProxyMode::Auto,
            RuntimeProxyMode::Direct => TunnelProxyMode::Direct,
            RuntimeProxyMode::Custom => TunnelProxyMode::Custom,
        };
        self.app
            .update_tunnel_proxy(mode, custom_url)
            .await
            .map(map_snapshot)
            .map_err(map_desktop_error)
    }

    pub(crate) fn cancel_operation(&self, operation_id: &str) -> ChadexResult<RuntimeSnapshot> {
        self.app
            .cancel_operation(operation_id)
            .map(map_snapshot)
            .map_err(map_desktop_error)
    }

    pub(crate) async fn cancel_chadex_task(
        &self,
        project: &str,
        task_id: &str,
    ) -> ChadexResult<Value> {
        let target = self
            .app
            .chadex_runtime_probe_target()
            .await
            .map_err(map_desktop_error)?
            .ok_or_else(|| {
                ChadexError::new(
                    "task_cancel_unavailable",
                    "The active Chadex runtime is unavailable",
                    "Restore the local Server and Runner before cancelling the task.",
                )
            })?;
        if target.runtime_project_id != project {
            return Err(ChadexError::new(
                "task_project_mismatch",
                "The task belongs to a different runtime project",
                "Refresh task progress before trying to cancel again.",
            ));
        }
        let token = read_probe_token(&target.user_token_file).await.ok_or_else(|| {
            ChadexError::new(
                "task_cancel_unavailable",
                "The local runtime credential is unavailable",
                "Restore the local runtime before cancelling the task.",
            )
        })?;
        let cancellation =
            CancellationContext::new(CancellationSignal::new(), CancellationSignal::new());
        let result = call_local_runtime_tool(
            &self.probe_client,
            &target.server_url,
            token.as_str(),
            "cancel_task",
            json!({"project": project, "task_id": task_id}),
            &cancellation,
        )
        .await
        .map_err(map_desktop_error)?
        .ok_or_else(|| {
            ChadexError::new(
                "task_cancel_unavailable",
                "The local runtime did not return a task cancellation result",
                "Refresh task progress and retry only if the task is still running.",
            )
        })?;
        if !result.success {
            let code = operator_error_code(&result).unwrap_or("task_cancel_failed");
            return Err(ChadexError::new(
                code,
                "The task cancellation request was not accepted",
                "Refresh task progress before deciding whether another cancellation is needed.",
            ));
        }
        Ok(result.output)
    }

    pub(crate) async fn shutdown(&self) {
        self.app.shutdown().await;
    }

    async fn try_fast_runtime_observation(&self) -> Option<RuntimeSnapshot> {
        let target = self.app.chadex_runtime_probe_target().await.ok()??;
        if !probe_exact_runtime_project(&self.probe_client, &target).await {
            return None;
        }
        self.app
            .chadex_apply_runtime_probe(&target)
            .await
            .ok()
            .map(map_snapshot)
    }
}

enum ActiveActivationAuthority {
    Registered(ChadexProjectActivationObservation),
    ExactRoot,
}

async fn wait_for_fresh_runtime_project(
    client: &Client,
    target: &ChadexProjectActivationTarget,
    cancellation: &CancellationContext,
) -> DesktopResult<Option<ChadexProjectActivationObservation>> {
    cancellation.check()?;
    let Some(token) = read_probe_token(&target.user_token_file).await else {
        return Ok(None);
    };
    let deadline = tokio::time::Instant::now() + FRESH_RUNTIME_READY_WINDOW;
    loop {
        cancellation.check()?;
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        let call = call_local_runtime_tool(
            client,
            &target.server_url,
            token.as_str(),
            "list_runners",
            json!({
                "client_id": target.runner_client_id,
                "include_projects": true,
                "summary_only": false
            }),
            cancellation,
        );
        let result = tokio::select! {
            _ = cancellation.cancelled() => return Err(cancelled_error()),
            result = tokio::time::timeout(remaining, call) => match result {
                Ok(result) => result,
                Err(_) => return Ok(None),
            },
        };
        match result {
            Ok(Some(result)) if result.success => {
                if let Some(ActiveActivationAuthority::Registered(observation)) =
                    active_activation_authority(&result.output, target)
                {
                    return Ok(Some(observation));
                }
            }
            Ok(_) => {}
            Err(error) if error.code == "desktop_operation_cancelled" => return Err(error),
            Err(_) => {}
        }

        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Ok(None);
        }
        tokio::select! {
            _ = cancellation.cancelled() => return Err(cancelled_error()),
            _ = tokio::time::sleep(FRESH_RUNTIME_READY_POLL_INTERVAL.min(remaining)) => {}
        }
    }
}

async fn fast_activate_project(
    client: &Client,
    target: &ChadexProjectActivationTarget,
    cancellation: &CancellationContext,
) -> DesktopResult<Option<ChadexProjectActivationObservation>> {
    cancellation.check()?;
    if !runner_config_has_exact_project_root(target).await {
        return Ok(None);
    }
    let Some(token) = read_probe_token(&target.user_token_file).await else {
        return Ok(None);
    };
    let Some(authority) =
        observe_active_activation_authority(client, target, token.as_str(), cancellation).await?
    else {
        return Ok(None);
    };
    if let ActiveActivationAuthority::Registered(observation) = authority {
        return Ok(Some(observation));
    }

    let response = match post_local_operator_json(
        client,
        &target.server_url,
        "/api/projects/resolve-or-register",
        token.as_str(),
        json!({
            "client_id": target.runner_client_id,
            "path": target.project.path,
        }),
        cancellation,
    )
    .await
    {
        Ok(Some(value)) => value,
        Ok(None) => return Ok(None),
        Err(error) if error.code == "desktop_operation_cancelled" => return Err(error),
        Err(_) => {
            return reconcile_fast_activation(client, target, token.as_str(), cancellation).await
        }
    };

    let result = match serde_json::from_value::<OperatorToolResult>(response) {
        Ok(result) => result,
        Err(_) => return Ok(None),
    };
    if result.success {
        return activation_observation(&result.output, target)
            .map(Some)
            .ok_or_else(project_activation_reconcile_error);
    }
    let code = operator_error_code(&result);
    if matches!(
        code,
        Some(
            "operation_indeterminate" | "project_projection_reconcile_required" | "outcome_unknown"
        )
    ) || result.output.get("execution_state").and_then(Value::as_str) == Some("outcome_unknown")
        || result.output.get("state_changed").and_then(Value::as_bool) == Some(true)
    {
        return reconcile_fast_activation(client, target, token.as_str(), cancellation).await;
    }
    Ok(None)
}

async fn observe_active_activation_authority(
    client: &Client,
    target: &ChadexProjectActivationTarget,
    token: &str,
    cancellation: &CancellationContext,
) -> DesktopResult<Option<ActiveActivationAuthority>> {
    let result = match call_local_runtime_tool(
        client,
        &target.server_url,
        token,
        "list_runners",
        json!({
            "client_id": target.runner_client_id,
            "include_projects": true,
            "summary_only": false
        }),
        cancellation,
    )
    .await
    {
        Ok(Some(result)) if result.success => result,
        Ok(_) => return Ok(None),
        Err(error) if error.code == "desktop_operation_cancelled" => return Err(error),
        Err(_) => return Ok(None),
    };
    Ok(active_activation_authority(&result.output, target))
}

fn active_activation_authority(
    output: &Value,
    target: &ChadexProjectActivationTarget,
) -> Option<ActiveActivationAuthority> {
    let agents = output.get("agents")?.as_array()?;
    if agents.len() != 1 {
        return None;
    }
    let agent = &agents[0];
    if agent.get("client_id")?.as_str()? != target.runner_client_id
        || agent.get("connected")?.as_bool()? != true
        || agent.get("status")?.as_str()? != "online"
    {
        return None;
    }
    let roots = agent.pointer("/policy/allowed_roots")?.as_array()?;
    let exact_root = roots.iter().filter_map(Value::as_str).any(|root| {
        chadex_runtime_runner_config::paths::paths_equal(Path::new(root), Path::new(&target.project.path))
    });
    if !exact_root {
        return None;
    }
    if let Some(projects) = agent.get("projects").and_then(Value::as_array) {
        for project in projects {
            if project.get("disabled").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let project_id = project.get("id").and_then(Value::as_str)?;
            let project_path = project.get("path").and_then(Value::as_str)?;
            if !chadex_runtime_runner_config::paths::paths_equal(
                Path::new(project_path),
                Path::new(&target.project.path),
            ) {
                continue;
            }
            return Some(ActiveActivationAuthority::Registered(
                ChadexProjectActivationObservation {
                    project_id: project_id.to_string(),
                    runtime_project_id: format!("agent:{}:{}", target.runner_client_id, project_id),
                    project_path: project_path.to_string(),
                },
            ));
        }
    }
    Some(ActiveActivationAuthority::ExactRoot)
}

async fn runner_config_has_exact_project_root(target: &ChadexProjectActivationTarget) -> bool {
    let metadata = match tokio::fs::symlink_metadata(&target.runner_config).await {
        Ok(metadata) => metadata,
        Err(_) => return false,
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > RUNTIME_ACTIVATION_CONFIG_MAX_BYTES
    {
        return false;
    }
    let content = match tokio::fs::read_to_string(&target.runner_config).await {
        Ok(content) => content,
        Err(_) => return false,
    };
    let config = match toml::from_str::<FastActivationRunnerConfig>(&content) {
        Ok(config) => config,
        Err(_) => return false,
    };
    if config.client_id != target.runner_client_id
        || local_runtime_url(&config.server_url, "/mcp")
            != local_runtime_url(&target.server_url, "/mcp")
    {
        return false;
    }
    let project = Path::new(&target.project.path);
    chadex_runtime_runner_config::paths::canonicalize_usable_allowed_roots(&config.policy.allowed_roots)
        .iter()
        .any(|root| chadex_runtime_runner_config::paths::paths_equal(root, project))
}

async fn call_local_runtime_tool(
    client: &Client,
    server_url: &str,
    token: &str,
    tool: &str,
    arguments: Value,
    cancellation: &CancellationContext,
) -> DesktopResult<Option<OperatorToolResult>> {
    cancellation.check()?;
    let Some(url) = local_mcp_url(server_url) else {
        return Ok(None);
    };
    let body = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "call_runtime_tool",
            "arguments": {
                "tool": tool,
                "arguments": arguments
            }
        }
    }))
    .map_err(|_| project_activation_reconcile_error())?;
    let request = client
        .post(url)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send();
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err(cancelled_error()),
        response = request => response,
    }
    .map_err(|_| project_activation_reconcile_error())?;
    cancellation.check()?;
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|length| length > RUNTIME_PROBE_MAX_RESPONSE_BYTES as u64)
    {
        return Ok(None);
    }
    let Some(value) = bounded_json_response(response).await else {
        return Ok(None);
    };
    if value.get("error").is_some() {
        return Ok(None);
    }
    let Some(structured) = value.pointer("/result/structuredContent") else {
        return Ok(None);
    };
    Ok(serde_json::from_value(structured.clone()).ok())
}

async fn post_local_operator_json(
    client: &Client,
    server_url: &str,
    endpoint: &str,
    token: &str,
    body: Value,
    cancellation: &CancellationContext,
) -> DesktopResult<Option<Value>> {
    cancellation.check()?;
    let Some(url) = local_runtime_url(server_url, endpoint) else {
        return Ok(None);
    };
    let body = serde_json::to_vec(&body).map_err(|_| {
        DesktopError::new(
            "project_activation_reconcile_required",
            "Chadex could not encode the project activation request",
            "Observe the current runtime state before retrying the project switch.",
        )
    })?;
    let request = client
        .post(url)
        .bearer_auth(token)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send();
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err(cancelled_error()),
        response = request => response,
    }
    .map_err(|_| {
        DesktopError::new(
            "project_activation_transport_uncertain",
            "The project activation response could not be observed",
            "Chadex will reconcile the exact project before allowing another activation.",
        )
    })?;
    cancellation.check()?;
    if matches!(response.status().as_u16(), 404 | 405) {
        return Ok(None);
    }
    if response
        .content_length()
        .is_some_and(|length| length > RUNTIME_PROBE_MAX_RESPONSE_BYTES as u64)
    {
        return Err(project_activation_reconcile_error());
    }
    let status = response.status();
    let value = bounded_json_response(response)
        .await
        .ok_or_else(project_activation_reconcile_error)?;
    if !status.is_success() && value.get("success").is_none() {
        return Ok(None);
    }
    Ok(Some(value))
}

async fn reconcile_fast_activation(
    client: &Client,
    target: &ChadexProjectActivationTarget,
    token: &str,
    cancellation: &CancellationContext,
) -> DesktopResult<Option<ChadexProjectActivationObservation>> {
    let value = post_local_operator_json(
        client,
        &target.server_url,
        "/api/projects/list",
        token,
        json!({"client_id": target.runner_client_id, "limit": 100}),
        cancellation,
    )
    .await?
    .ok_or_else(project_activation_reconcile_error)?;
    let result = serde_json::from_value::<OperatorToolResult>(value)
        .map_err(|_| project_activation_reconcile_error())?;
    if !result.success {
        return Err(project_activation_reconcile_error());
    }
    let Some(projects) = result.output.get("projects").and_then(Value::as_array) else {
        return Err(project_activation_reconcile_error());
    };
    for project in projects {
        if let Some(observation) = activation_observation(project, target) {
            return Ok(Some(observation));
        }
    }
    Err(project_activation_reconcile_error())
}

fn project_activation_reconcile_error() -> DesktopError {
    DesktopError::new(
        "project_activation_reconcile_required",
        "Project activation may have completed, but the exact project could not be confirmed",
        "Observe the current runtime state before retrying the project switch.",
    )
}

fn activation_observation(
    output: &Value,
    target: &ChadexProjectActivationTarget,
) -> Option<ChadexProjectActivationObservation> {
    let runtime_project_id = output.get("id")?.as_str()?.trim();
    let prefix = format!("agent:{}:", target.runner_client_id);
    let runtime_project_suffix = runtime_project_id.strip_prefix(&prefix)?;
    let project_id = output
        .get("agent_project_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(runtime_project_suffix);
    let client_id = output.get("client_id")?.as_str()?;
    let project_path = output.get("path")?.as_str()?;
    if runtime_project_id.is_empty()
        || project_id.is_empty()
        || project_id != runtime_project_suffix
        || client_id != target.runner_client_id
        || !chadex_runtime_runner_config::paths::paths_equal(
            Path::new(project_path),
            Path::new(&target.project.path),
        )
    {
        return None;
    }
    Some(ChadexProjectActivationObservation {
        project_id: project_id.to_string(),
        runtime_project_id: runtime_project_id.to_string(),
        project_path: project_path.to_string(),
    })
}

fn operator_error_code(result: &OperatorToolResult) -> Option<&str> {
    result
        .output
        .get("error_code")
        .and_then(Value::as_str)
        .or_else(|| result.output.get("error_kind").and_then(Value::as_str))
        .or(result.error.as_deref())
}

async fn probe_exact_runtime_project(client: &Client, target: &ChadexRuntimeProbeTarget) -> bool {
    let Some(url) = local_mcp_url(&target.server_url) else {
        return false;
    };
    let Some(token) = read_probe_token(&target.user_token_file).await else {
        return false;
    };
    let payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "call_runtime_tool",
            "arguments": {
                "tool": "list_projects",
                "arguments": {
                    "client_id": target.runner_client_id,
                    "project": target.runtime_project_id,
                    "limit": 1,
                    "summary_only": false
                }
            }
        }
    });
    let body = match serde_json::to_vec(&payload) {
        Ok(body) => body,
        Err(_) => return false,
    };
    let response = match client
        .post(url)
        .bearer_auth(token.as_str())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => response,
        _ => return false,
    };
    if response
        .content_length()
        .is_some_and(|length| length > RUNTIME_PROBE_MAX_RESPONSE_BYTES as u64)
    {
        return false;
    }
    let Some(value) = bounded_json_response(response).await else {
        return false;
    };
    exact_runtime_project_is_ready(&value, target)
}

fn local_mcp_url(server_url: &str) -> Option<Url> {
    local_runtime_url(server_url, "/mcp")
}

fn local_runtime_url(server_url: &str, endpoint: &str) -> Option<Url> {
    if !endpoint.starts_with('/') || endpoint.contains('?') || endpoint.contains('#') {
        return None;
    }
    let mut url = Url::parse(server_url).ok()?;
    if url.scheme() != "http"
        || url.port().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return None;
    }
    let host = url
        .host_str()?
        .trim_start_matches('[')
        .trim_end_matches(']');
    if !host.eq_ignore_ascii_case("localhost")
        && !host
            .parse::<std::net::IpAddr>()
            .ok()
            .is_some_and(|ip| ip.is_loopback())
    {
        return None;
    }
    url.set_path(endpoint);
    Some(url)
}

async fn read_probe_token(path: &Path) -> Option<Zeroizing<String>> {
    let metadata = tokio::fs::symlink_metadata(path).await.ok()?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > RUNTIME_PROBE_MAX_TOKEN_BYTES
    {
        return None;
    }
    let token = tokio::fs::read_to_string(path).await.ok()?;
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(Zeroizing::new(token.to_string()))
}

async fn bounded_json_response(mut response: reqwest::Response) -> Option<Value> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if body.len().saturating_add(chunk.len()) > RUNTIME_PROBE_MAX_RESPONSE_BYTES {
            return None;
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).ok()
}

fn exact_runtime_project_is_ready(value: &Value, target: &ChadexRuntimeProbeTarget) -> bool {
    if value.get("error").is_some() {
        return false;
    }
    let Some(structured) = value.pointer("/result/structuredContent") else {
        return false;
    };
    if structured.get("success").and_then(Value::as_bool) != Some(true) {
        return false;
    }
    let Some(output) = structured.get("output") else {
        return false;
    };
    if output.get("count").and_then(Value::as_u64) != Some(1)
        || output.get("matched_count").and_then(Value::as_u64) != Some(1)
        || output.get("truncated").and_then(Value::as_bool) != Some(false)
    {
        return false;
    }
    let Some(project) = output
        .get("projects")
        .and_then(Value::as_array)
        .and_then(|projects| projects.first())
    else {
        return false;
    };
    project.get("id").and_then(Value::as_str) == Some(target.runtime_project_id.as_str())
        && project.get("client_id").and_then(Value::as_str)
            == Some(target.runner_client_id.as_str())
        && project.get("path").and_then(Value::as_str) == Some(target.project_path.as_str())
        && project.get("connected").and_then(Value::as_bool) == Some(true)
        && project.get("agent_status").and_then(Value::as_str) == Some("online")
}

fn map_desktop_error(error: DesktopError) -> ChadexError {
    map_compat_error(error)
}

fn map_activity(
    entry: crate::chadex_core::runtime_compat::activity::ActivityEntry,
) -> RuntimeActivityEntry {
    map_compat_activity(entry)
}

fn map_project(project: ProjectSelection) -> RuntimeProject {
    RuntimeProject {
        path: project.path,
        allowed_root: project.allowed_root,
        is_git_repository: project.is_git_repository,
    }
}

fn map_snapshot(snapshot: DesktopStateSnapshot) -> RuntimeSnapshot {
    let presentation = present_readiness(&snapshot.readiness.summary_kind);
    let readiness = RuntimeReadiness {
        runtime_ready: snapshot.readiness.runtime_ready,
        needs_attention: matches!(
            snapshot.readiness.summary_kind,
            ReadinessSummaryKind::ServiceNeedsAttention | ReadinessSummaryKind::RunnerDisconnected
        ),
        summary: presentation.summary,
        next_action: presentation.next_action,
        summary_kind: readiness_summary_kind(snapshot.readiness.summary_kind),
        server: server_readiness(snapshot.readiness.server),
        runner: runner_readiness(snapshot.readiness.runner),
        exposure: exposure_readiness(snapshot.readiness.exposure),
        project: project_readiness(snapshot.readiness.project),
    };
    let current_operation = snapshot
        .current_operation
        .map(|operation| RuntimeOperation {
            id: operation.id,
            kind: operation.kind.as_str(),
            phase: match operation.phase {
                DesktopOperationPhase::Running => RuntimeOperationPhase::Running,
                DesktopOperationPhase::Cancelling => RuntimeOperationPhase::Cancelling,
            },
            started_at_ms: operation.started_at_ms,
            cancellable: operation.cancellable,
        });
    RuntimeSnapshot {
        runtime_configured: snapshot.topology.is_some(),
        readiness,
        project: snapshot.project.map(map_project),
        current_operation,
        activity_sequence: snapshot.activity_sequence,
        tunnel_proxy_effective_url: snapshot.tunnel_proxy.effective_url,
    }
}

fn readiness_summary_kind(kind: ReadinessSummaryKind) -> &'static str {
    match kind {
        ReadinessSummaryKind::ReadyForChatGpt => "ready_for_chat_gpt",
        ReadinessSummaryKind::RuntimeStopped => "runtime_stopped",
        ReadinessSummaryKind::RuntimeStarting => "runtime_starting",
        ReadinessSummaryKind::ServiceNeedsAttention => "service_needs_attention",
        ReadinessSummaryKind::RunnerDisconnected => "runner_disconnected",
        ReadinessSummaryKind::ProjectNotReady => "project_not_ready",
        ReadinessSummaryKind::RuntimeReadyLocalOnly => "runtime_ready_local_only",
        ReadinessSummaryKind::TunnelReadyWaitingForChatGpt => "tunnel_ready_waiting_for_chat_gpt",
        ReadinessSummaryKind::ConnectionUnverified => "connection_unverified",
        ReadinessSummaryKind::QuickShareStopped => "quick_share_stopped",
    }
}

fn server_readiness(value: ServerReadiness) -> &'static str {
    match value {
        ServerReadiness::Stopped => "stopped",
        ServerReadiness::Starting => "starting",
        ServerReadiness::Ready => "ready",
        ServerReadiness::Error => "error",
        ServerReadiness::Unknown => "unknown",
    }
}

fn runner_readiness(value: RunnerReadiness) -> &'static str {
    match value {
        RunnerReadiness::Stopped => "stopped",
        RunnerReadiness::Connecting => "connecting",
        RunnerReadiness::Ready => "ready",
        RunnerReadiness::Error => "error",
        RunnerReadiness::Unknown => "unknown",
    }
}

fn exposure_readiness(value: ExposureReadiness) -> &'static str {
    match value {
        ExposureReadiness::Disabled => "disabled",
        ExposureReadiness::Starting => "starting",
        ExposureReadiness::LocalReady => "local_ready",
        ExposureReadiness::RemoteReady => "remote_ready",
        ExposureReadiness::Degraded => "degraded",
        ExposureReadiness::Error => "error",
        ExposureReadiness::Unknown => "unknown",
    }
}

fn project_readiness(value: ProjectReadiness) -> &'static str {
    match value {
        ProjectReadiness::None => "none",
        ProjectReadiness::Configured => "configured",
        ProjectReadiness::ReloadRequired => "reload_required",
        ProjectReadiness::Ready => "ready",
        ProjectReadiness::Error => "error",
        ProjectReadiness::Unknown => "unknown",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chadex_core::runtime_compat::models::{
        aggregate_readiness, ExposureReadiness, ProjectReadiness, RunnerReadiness, ServerReadiness,
    };
    use std::path::PathBuf;

    fn probe_target() -> ChadexRuntimeProbeTarget {
        ChadexRuntimeProbeTarget {
            server_url: "http://127.0.0.1:8765".to_string(),
            user_token_file: PathBuf::from("/tmp/chadex-probe-token"),
            runner_client_id: "runner-a".to_string(),
            runtime_project_id: "agent:runner-a:repo".to_string(),
            project_path: "/tmp/repo".to_string(),
        }
    }

    fn activation_target(
        project_path: String,
        runner_config: PathBuf,
    ) -> ChadexProjectActivationTarget {
        ChadexProjectActivationTarget {
            project: ProjectSelection {
                path: project_path.clone(),
                allowed_root: project_path,
                is_git_repository: true,
                runtime_project_id: None,
            },
            server_url: "http://127.0.0.1:8765".to_string(),
            user_token_file: PathBuf::from("/tmp/chadex-activation-token"),
            runner_config,
            runner_client_id: "runner-a".to_string(),
        }
    }

    fn ready_probe_response(target: &ChadexRuntimeProbeTarget) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "structuredContent": {
                    "success": true,
                    "output": {
                        "count": 1,
                        "matched_count": 1,
                        "truncated": false,
                        "projects": [{
                            "id": target.runtime_project_id,
                            "client_id": target.runner_client_id,
                            "path": target.project_path,
                            "connected": true,
                            "agent_status": "online"
                        }]
                    }
                }
            }
        })
    }

    #[test]
    fn adapter_projects_only_chadex_runtime_fields() {
        let mut desktop = DesktopStateSnapshot::default();
        desktop.readiness = aggregate_readiness(
            ServerReadiness::Ready,
            RunnerReadiness::Ready,
            ExposureReadiness::Disabled,
            ProjectReadiness::Ready,
        );
        desktop.activity_sequence = 7;
        desktop.tunnel_proxy.effective_url = Some("http://127.0.0.1:8888".to_string());

        let runtime = map_snapshot(desktop);
        assert!(!runtime.runtime_configured);
        assert!(runtime.readiness.runtime_ready);
        assert!(!runtime.readiness.needs_attention);
        assert_eq!(runtime.activity_sequence, 7);
        assert_eq!(
            runtime.tunnel_proxy_effective_url.as_deref(),
            Some("http://127.0.0.1:8888")
        );
    }

    #[test]
    fn runtime_probe_accepts_only_explicit_loopback_http_origin() {
        assert_eq!(
            local_mcp_url("http://127.0.0.1:8765")
                .map(|url| url.to_string())
                .as_deref(),
            Some("http://127.0.0.1:8765/mcp")
        );
        assert_eq!(
            local_mcp_url("http://localhost:8765/")
                .map(|url| url.to_string())
                .as_deref(),
            Some("http://localhost:8765/mcp")
        );
        assert_eq!(
            local_mcp_url("http://[::1]:8765")
                .map(|url| url.to_string())
                .as_deref(),
            Some("http://[::1]:8765/mcp")
        );

        for invalid in [
            "https://127.0.0.1:8765",
            "http://example.test:8765",
            "http://127.0.0.1",
            "http://user:secret@127.0.0.1:8765",
            "http://127.0.0.1:8765/mcp",
            "http://127.0.0.1:8765?token=secret",
            "http://127.0.0.1:8765#fragment",
        ] {
            assert!(local_mcp_url(invalid).is_none(), "accepted {invalid}");
        }
    }

    #[test]
    fn runtime_probe_requires_exact_online_project_identity() {
        let target = probe_target();
        let ready = ready_probe_response(&target);
        assert!(exact_runtime_project_is_ready(&ready, &target));

        for (pointer, replacement) in [
            (
                "/result/structuredContent/output/projects/0/id",
                json!("agent:runner-a:other"),
            ),
            (
                "/result/structuredContent/output/projects/0/client_id",
                json!("runner-b"),
            ),
            (
                "/result/structuredContent/output/projects/0/path",
                json!("/tmp/other"),
            ),
            (
                "/result/structuredContent/output/projects/0/connected",
                json!(false),
            ),
            (
                "/result/structuredContent/output/projects/0/agent_status",
                json!("offline"),
            ),
            ("/result/structuredContent/output/truncated", json!(true)),
            ("/result/structuredContent/output/matched_count", json!(2)),
        ] {
            let mut invalid = ready.clone();
            *invalid.pointer_mut(pointer).expect("fixture pointer") = replacement;
            assert!(
                !exact_runtime_project_is_ready(&invalid, &target),
                "accepted mismatch at {pointer}"
            );
        }

        let mut failed = ready.clone();
        failed["result"]["structuredContent"]["success"] = json!(false);
        assert!(!exact_runtime_project_is_ready(&failed, &target));

        let rpc_error = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {"code": -32000, "message": "denied"}
        });
        assert!(!exact_runtime_project_is_ready(&rpc_error, &target));
    }

    #[test]
    fn activation_observation_requires_exact_runtime_identity() {
        let target = activation_target("/tmp/repo".to_string(), PathBuf::from("/tmp/runner.toml"));
        let ready = json!({
            "id": "agent:runner-a:repo",
            "agent_project_id": "repo",
            "client_id": "runner-a",
            "path": "/tmp/repo"
        });
        let observation = activation_observation(&ready, &target).expect("exact identity");
        assert_eq!(observation.project_id, "repo");
        assert_eq!(observation.runtime_project_id, "agent:runner-a:repo");

        for (field, value) in [
            ("id", json!("agent:runner-b:repo")),
            ("agent_project_id", json!("other")),
            ("client_id", json!("runner-b")),
            ("path", json!("/tmp/other")),
        ] {
            let mut invalid = ready.clone();
            invalid[field] = value;
            assert!(
                activation_observation(&invalid, &target).is_none(),
                "accepted mismatched {field}"
            );
        }
    }

    #[test]
    fn fast_activation_requires_active_runner_exact_root_and_online_inventory() {
        let target = activation_target("/tmp/repo".to_string(), PathBuf::from("/tmp/runner.toml"));
        let ready = json!({
            "agents": [{
                "client_id": "runner-a",
                "connected": true,
                "status": "online",
                "policy": {"allowed_roots": ["/tmp/repo"]},
                "projects": [{
                    "id": "repo",
                    "path": "/tmp/repo",
                    "disabled": false
                }]
            }]
        });
        match active_activation_authority(&ready, &target) {
            Some(ActiveActivationAuthority::Registered(observation)) => {
                assert_eq!(observation.project_id, "repo");
                assert_eq!(observation.runtime_project_id, "agent:runner-a:repo");
            }
            _ => panic!("expected registered exact-root authority"),
        }

        let mut parent_only = ready.clone();
        parent_only["agents"][0]["policy"]["allowed_roots"] = json!(["/tmp"]);
        assert!(active_activation_authority(&parent_only, &target).is_none());

        let mut offline = ready.clone();
        offline["agents"][0]["connected"] = json!(false);
        assert!(active_activation_authority(&offline, &target).is_none());

        let mut not_registered = ready.clone();
        not_registered["agents"][0]["projects"] = json!([]);
        assert!(matches!(
            active_activation_authority(&not_registered, &target),
            Some(ActiveActivationAuthority::ExactRoot)
        ));
    }

    #[tokio::test]
    async fn fast_activation_requires_exact_persisted_project_root() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("parent");
        let project = parent.join("project");
        tokio::fs::create_dir_all(&project).await.unwrap();
        let parent = parent.canonicalize().unwrap();
        let project = project.canonicalize().unwrap();
        let runner_config = temp.path().join("runner.toml");

        let parent_only = format!(
            "server_url = \"http://127.0.0.1:8765\"\nclient_id = \"runner-a\"\n\n[policy]\nallowed_roots = [{}]\n",
            toml::Value::String(parent.to_string_lossy().into_owned())
        );
        tokio::fs::write(&runner_config, parent_only).await.unwrap();
        let target = activation_target(
            project.to_string_lossy().into_owned(),
            runner_config.clone(),
        );
        assert!(
            !runner_config_has_exact_project_root(&target).await,
            "a parent root must not satisfy explicit-project exact authority"
        );

        let exact = format!(
            "server_url = \"http://127.0.0.1:8765\"\nclient_id = \"runner-a\"\n\n[policy]\nallowed_roots = [{}]\n",
            toml::Value::String(project.to_string_lossy().into_owned())
        );
        tokio::fs::write(&runner_config, exact.as_bytes())
            .await
            .unwrap();
        assert!(runner_config_has_exact_project_root(&target).await);

        let wrong_client = exact.replace("client_id = \"runner-a\"", "client_id = \"runner-b\"");
        tokio::fs::write(&runner_config, wrong_client)
            .await
            .unwrap();
        assert!(!runner_config_has_exact_project_root(&target).await);
    }

    #[test]
    fn operator_runtime_url_is_loopback_only_and_path_bounded() {
        assert_eq!(
            local_runtime_url("http://127.0.0.1:8765", "/api/projects/resolve-or-register")
                .map(|url| url.to_string())
                .as_deref(),
            Some("http://127.0.0.1:8765/api/projects/resolve-or-register")
        );
        assert!(local_runtime_url("http://example.test:8765", "/api/projects/list").is_none());
        assert!(local_runtime_url("http://127.0.0.1:8765", "api/projects/list").is_none());
        assert!(local_runtime_url("http://127.0.0.1:8765", "/api/projects/list?wide=1").is_none());
    }

    #[tokio::test]
    async fn runtime_probe_token_reader_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        let token_path = temp.path().join("user-token");
        tokio::fs::write(&token_path, "secret-token\n")
            .await
            .unwrap();
        let token = read_probe_token(&token_path).await;
        assert_eq!(
            token.as_ref().map(|value| value.as_str()),
            Some("secret-token")
        );

        let symlink_path = temp.path().join("user-token-link");
        symlink(&token_path, &symlink_path).unwrap();
        assert!(read_probe_token(&symlink_path).await.is_none());
    }
}
