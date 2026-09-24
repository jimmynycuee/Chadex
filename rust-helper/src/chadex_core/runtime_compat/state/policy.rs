use super::settings::*;
use super::store::desktop_state_unavailable;
use crate::chadex_core::runtime_compat::deadline::Deadline;
use crate::chadex_core::runtime_compat::error::{DesktopError, DesktopResult};
use crate::chadex_core::runtime_compat::models::{
    DesktopStateSnapshot, Experience, Exposure, ExposureReadiness, ProjectSelection,
    ReadinessNextActionKind, ReadinessSummaryKind, RegularConnectionPreference, RegularTunnelState,
    RegularTunnelStatus, RuntimeTopology, ServerTopology, StoredDesktopConfig, TunnelProxyConfig,
    TunnelProxyMode, TunnelProxySnapshot,
};
use crate::chadex_core::runtime_compat::operation::{cancelled_error, CancellationContext};
use crate::chadex_core::runtime_compat::integration::ProjectRuntimeIdentity;
use crate::chadex_core::runtime_compat::tunnel_config::TunnelConfig;
use serde_json::Value;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;
pub(super) fn startup_readiness_poll_interval(elapsed: Duration) -> Duration {
    if elapsed < STARTUP_FAST_POLL_WINDOW {
        STARTUP_FAST_POLL_INTERVAL
    } else if elapsed < STARTUP_MEDIUM_POLL_WINDOW {
        STARTUP_MEDIUM_POLL_INTERVAL
    } else {
        POLL_INTERVAL
    }
}

pub(super) async fn sleep_or_cancel_until(
    duration: Duration,
    cancellation: &CancellationContext,
    deadline: Deadline,
) -> DesktopResult<()> {
    cancellation.check()?;
    if deadline.is_elapsed() {
        return Ok(());
    }
    let wake_at = std::cmp::min(deadline.instant(), tokio::time::Instant::now() + duration);
    tokio::select! {
        biased;
        _ = cancellation.cancelled() => Err(cancelled_error()),
        _ = tokio::time::sleep_until(wake_at) => Ok(()),
    }
}

pub(super) fn readiness_timeout_error(
    code: &'static str,
    message: &'static str,
    action: &'static str,
) -> DesktopError {
    DesktopError::new(code, message, action)
        .with_details(serde_json::json!({ "category": "readiness_timeout" }))
}

// These are Desktop packaging defaults, not process-environment overrides.
// The Server loads its env file only into keys that are absent from the process
// environment, so the effective precedence remains built-in defaults < this
// config file < explicit environment variables.
pub(super) fn ensure_desktop_server_defaults(path: &Path) -> DesktopResult<()> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        desktop_state_unavailable("Desktop could not inspect its local Server configuration")
            .with_details(serde_json::json!({ "io_kind": format!("{:?}", error.kind()) }))
    })?;
    if !metadata.is_file() || metadata.len() > DESKTOP_SERVER_ENV_MAX_BYTES {
        return Err(desktop_state_unavailable(
            "Desktop local Server configuration is not a bounded regular file",
        ));
    }
    let content = std::fs::read_to_string(path).map_err(|error| {
        desktop_state_unavailable("Desktop could not read its local Server configuration")
            .with_details(serde_json::json!({ "io_kind": format!("{:?}", error.kind()) }))
    })?;
    let has_key = |key: &str| {
        content.lines().any(|line| {
            let line = line.trim();
            let line = line.strip_prefix("export ").unwrap_or(line).trim();
            line.split_once('=')
                .is_some_and(|(candidate, _)| candidate.trim() == key)
        })
    };
    let mut additions = Vec::new();
    if !has_key("WEBCODEX_MCP_COMPACT_SCHEMAS") {
        additions.push(format!(
            "WEBCODEX_MCP_COMPACT_SCHEMAS={DESKTOP_MCP_COMPACT_SCHEMAS}"
        ));
    }
    if additions.is_empty() {
        return Ok(());
    }
    let mut file = OpenOptions::new()
        .append(true)
        .open(path)
        .map_err(|error| {
            desktop_state_unavailable("Desktop could not update its local Server configuration")
                .with_details(serde_json::json!({ "io_kind": format!("{:?}", error.kind()) }))
        })?;
    if !content.is_empty() && !content.ends_with('\n') {
        file.write_all(b"\n").map_err(|error| {
            desktop_state_unavailable("Desktop could not update its local Server configuration")
                .with_details(serde_json::json!({ "io_kind": format!("{:?}", error.kind()) }))
        })?;
    }
    for addition in additions {
        writeln!(file, "{addition}").map_err(|error| {
            desktop_state_unavailable("Desktop could not update its local Server configuration")
                .with_details(serde_json::json!({ "io_kind": format!("{:?}", error.kind()) }))
        })?;
    }
    file.flush().map_err(|error| {
        desktop_state_unavailable("Desktop could not update its local Server configuration")
            .with_details(serde_json::json!({ "io_kind": format!("{:?}", error.kind()) }))
    })?;
    Ok(())
}

pub(super) fn machine_event_overflow_error(event: &Value) -> DesktopError {
    DesktopError::new(
        "machine_event_overflow",
        "Desktop could not retain every critical machine-readiness event",
        "Retry the operation and inspect Activity if the child keeps emitting excessive machine events.",
    )
    .with_details(serde_json::json!({
        "category": "machine_event_overflow",
        "dropped_critical": event
            .get("dropped_critical")
            .and_then(Value::as_u64)
            .unwrap_or(1),
    }))
}

pub(super) fn local_enrollment_directory(data_dir: &Path, config: &StoredDesktopConfig) -> PathBuf {
    // Two reusable slots bound local credential storage while keeping login's
    // --overwrite away from the currently committed recovery identity. Resolve
    // native path aliases (notably /var on macOS) before comparing paths.
    let root = data_dir
        .canonicalize()
        .unwrap_or_else(|_| data_dir.to_path_buf())
        .join("local-connections");
    let first = root.join("a");
    let saved_runner = config
        .runtime
        .as_ref()
        .and_then(|runtime| runtime.runner_config.as_ref());
    if saved_runner.is_some_and(|path| {
        path.canonicalize()
            .unwrap_or_else(|_| path.clone())
            .starts_with(&first)
    }) {
        root.join("b")
    } else {
        first
    }
}

pub(super) fn project_snapshot(config: &StoredDesktopConfig) -> Option<ProjectSelection> {
    let mut project = config.project.clone()?;
    if identity_from_config(config).is_none() {
        project.runtime_project_id = None;
    }
    Some(project)
}

pub(super) fn stored_runner_client_id(config: &StoredDesktopConfig) -> Option<String> {
    let runtime = config.runtime.as_ref()?;
    if let Some(client_id) = runtime
        .runner_client_id
        .as_deref()
        .map(str::trim)
        .filter(|client_id| !client_id.is_empty())
    {
        return Some(client_id.to_string());
    }
    // Pre-migration Desktop state did not persist client_id separately. Recover
    // it from the exact runtime Project identity instead of accepting whatever
    // client_id happens to be present in runner.toml during the first upgrade.
    let project_id = runtime.project_id.as_deref()?.trim();
    let runtime_project_id = runtime.runtime_project_id.as_deref()?.trim();
    if project_id.is_empty() || runtime_project_id.is_empty() {
        return None;
    }
    let suffix = format!(":{project_id}");
    runtime_project_id
        .strip_prefix("agent:")?
        .strip_suffix(&suffix)
        .map(str::trim)
        .filter(|client_id| !client_id.is_empty())
        .map(str::to_string)
}

pub(super) fn identity_from_config(config: &StoredDesktopConfig) -> Option<ProjectRuntimeIdentity> {
    let runtime = config.runtime.as_ref()?;
    let project = config.project.as_ref()?;
    let runner_config = runtime.runner_config.clone()?;
    let user_token_file = runtime.user_token_file.clone()?;
    let project_id = runtime.project_id.clone()?.trim().to_string();
    let runtime_project_id = runtime.runtime_project_id.clone()?.trim().to_string();
    if project_id.is_empty()
        || runtime_project_id.is_empty()
        || !runner_config.is_file()
        || !user_token_file.is_file()
    {
        return None;
    }
    Some(ProjectRuntimeIdentity {
        project_id,
        runtime_project_id,
        project_path: project.path.clone(),
        runner_config,
        user_token_file,
        server_url: runtime.server_url.clone(),
    })
}

pub(super) fn reserve_loopback_address() -> DesktopResult<String> {
    let listener = TcpListener::bind(("127.0.0.1", 0)).map_err(|_| {
        DesktopError::new(
            "local_port_unavailable",
            "Desktop could not reserve a loopback port for WebCodex",
            "Check local networking and retry.",
        )
    })?;
    let address = listener.local_addr().map_err(|_| {
        DesktopError::new(
            "local_port_unavailable",
            "Desktop could not inspect the reserved loopback port",
            "Retry setup.",
        )
    })?;
    Ok(address.to_string())
}

pub(super) fn regular_tunnel_exposure(
    state: &mut Option<RegularTunnelState>,
    process_active: bool,
) -> Option<ExposureReadiness> {
    let state = state.as_mut()?;
    if !process_active {
        state.status = RegularTunnelStatus::Error;
        state.ready_for_chatgpt = false;
        return Some(ExposureReadiness::Error);
    }
    Some(match state.status {
        RegularTunnelStatus::Starting => ExposureReadiness::Starting,
        RegularTunnelStatus::Ready if state.ready_for_chatgpt => ExposureReadiness::LocalReady,
        RegularTunnelStatus::Ready => ExposureReadiness::Degraded,
        RegularTunnelStatus::Error => ExposureReadiness::Error,
    })
}

pub(super) fn apply_regular_tunnel_next_action(
    snapshot: &mut DesktopStateSnapshot,
    exposure: &ExposureReadiness,
) {
    if !snapshot.readiness.runtime_ready {
        return;
    }
    match exposure {
        ExposureReadiness::Error => {
            snapshot.readiness.next_action_kind =
                Some(ReadinessNextActionKind::RestartSecureTunnel);
            snapshot.readiness.next_action = Some("Restart the secure tunnel.".to_string());
        }
        ExposureReadiness::LocalReady => {
            snapshot.readiness.summary_kind = ReadinessSummaryKind::TunnelReadyWaitingForChatGpt;
            snapshot.readiness.summary =
                "OpenAI Secure Tunnel is ready; waiting for ChatGPT to connect".to_string();
            snapshot.readiness.next_action_kind = Some(ReadinessNextActionKind::CheckConnection);
            snapshot.readiness.next_action = Some(
                "Connect the Tunnel in ChatGPT, then verify it with one real project read."
                    .to_string(),
            );
        }
        ExposureReadiness::Degraded => {
            snapshot.readiness.next_action_kind =
                Some(ReadinessNextActionKind::RestoreClipboardHandoff);
            snapshot.readiness.next_action = Some(
                "Restore clipboard access, then restart the secure tunnel handoff.".to_string(),
            );
        }
        _ => {}
    }
}

pub(super) fn effective_topology(
    configured: Option<&RuntimeTopology>,
    regular_tunnel_selected: bool,
) -> Option<RuntimeTopology> {
    let mut topology = configured.cloned()?;
    if regular_tunnel_selected
        && topology.experience == Experience::Full
        && matches!(topology.server, ServerTopology::Local)
    {
        topology.exposure = Exposure::OpenAiTunnel;
    }
    Some(topology)
}

pub(super) fn exposure_readiness(topology: Option<&RuntimeTopology>) -> ExposureReadiness {
    match topology.map(|topology| &topology.exposure) {
        Some(Exposure::None) => ExposureReadiness::LocalReady,
        // An HTTPS origin is a configured route, not evidence that the MCP
        // endpoint plus ChatGPT authentication/handoff is externally usable.
        Some(Exposure::ExistingHttps { .. }) => ExposureReadiness::Unknown,
        Some(Exposure::Cloudflare | Exposure::OpenAiTunnel) => ExposureReadiness::Unknown,
        None => ExposureReadiness::Unknown,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct EffectiveTunnelProxy {
    pub(super) url: Option<String>,
    pub(super) source: &'static str,
    pub(super) detected_url: Option<String>,
}

pub(super) fn validate_tunnel_proxy_url(value: &str) -> DesktopResult<String> {
    crate::chadex_core::runtime_compat::platform::normalize_proxy_server(value).ok_or_else(|| {
        DesktopError::new(
            "tunnel_proxy_invalid",
            "The Tunnel proxy must be an HTTP or HTTPS proxy URL without embedded credentials",
            "Use a value such as http://127.0.0.1:7890, or choose automatic proxy detection.",
        )
    })
}

pub(super) fn environment_tunnel_proxy() -> Option<String> {
    ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"]
        .iter()
        .find_map(|name| {
            std::env::var(name)
                .ok()
                .and_then(|value| crate::chadex_core::runtime_compat::platform::normalize_proxy_server(&value))
        })
}

pub(super) fn effective_tunnel_proxy(
    config: &TunnelProxyConfig,
) -> DesktopResult<EffectiveTunnelProxy> {
    let system = crate::chadex_core::runtime_compat::platform::system_http_proxy_candidate();
    let detected_url = system.as_ref().map(|candidate| candidate.url.clone());
    match config.mode {
        TunnelProxyMode::Direct => Ok(EffectiveTunnelProxy {
            url: None,
            source: "direct",
            detected_url,
        }),
        TunnelProxyMode::Custom => Ok(EffectiveTunnelProxy {
            url: Some(validate_tunnel_proxy_url(
                config.custom_url.as_deref().unwrap_or(""),
            )?),
            source: "custom",
            detected_url,
        }),
        TunnelProxyMode::Auto => {
            if let Some(url) = environment_tunnel_proxy() {
                return Ok(EffectiveTunnelProxy {
                    url: Some(url),
                    source: "environment",
                    detected_url,
                });
            }
            if let Some(candidate) = system {
                if candidate.enabled || crate::chadex_core::runtime_compat::platform::proxy_is_loopback(&candidate.url) {
                    return Ok(EffectiveTunnelProxy {
                        url: Some(candidate.url),
                        source: if candidate.enabled {
                            "windows_system"
                        } else {
                            "windows_loopback_candidate"
                        },
                        detected_url,
                    });
                }
            }
            Ok(EffectiveTunnelProxy {
                url: None,
                source: "direct",
                detected_url,
            })
        }
    }
}

pub(super) fn runtime_autostart(config: &StoredDesktopConfig) -> bool {
    config.runtime_autostart.unwrap_or_else(|| {
        config.runtime.is_some()
            && config
                .topology
                .as_ref()
                .is_some_and(|topology| topology.experience == Experience::Full)
    })
}

pub(super) fn default_management_project_dir(data_dir: &Path, resource_dir: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let _ = data_dir;
        // The NSIS package is current-user scoped. Use the actual installation
        // directory as the default management Project so a fresh Desktop is
        // immediately manageable without asking the user to choose an unrelated
        // source checkout first. This intentionally grants the Project the same
        // install-directory authority the user has requested for Desktop
        // configuration and maintenance.
        return resource_dir.to_path_buf();
    }
    #[cfg(not(windows))]
    {
        // A macOS resource directory lives inside the signed app bundle and is
        // not a mutable workspace. Keep the same management-project semantics
        // in the per-user Desktop data directory there.
        let _ = resource_dir;
        data_dir.join("workspace")
    }
}

pub(super) fn preferred_connection(config: &StoredDesktopConfig) -> RegularConnectionPreference {
    config.preferred_connection.unwrap_or_default()
}

pub(super) fn apply_config_projection(
    snapshot: &mut DesktopStateSnapshot,
    config: &StoredDesktopConfig,
) {
    snapshot.runtime_autostart = runtime_autostart(config);
    snapshot.preferred_connection = preferred_connection(config);
    snapshot.tunnel_proxy = match effective_tunnel_proxy(&config.tunnel_proxy) {
        Ok(proxy) => TunnelProxySnapshot {
            mode: config.tunnel_proxy.mode,
            custom_url: config.tunnel_proxy.custom_url.clone(),
            effective_source: proxy.source.to_string(),
            effective_url: proxy.url,
            detected_url: proxy.detected_url,
        },
        Err(_) => TunnelProxySnapshot {
            mode: config.tunnel_proxy.mode,
            custom_url: config.tunnel_proxy.custom_url.clone(),
            effective_source: "invalid_custom".to_string(),
            effective_url: None,
            detected_url: crate::chadex_core::runtime_compat::platform::system_http_proxy_candidate()
                .map(|candidate| candidate.url),
        },
    };
}

pub(super) fn apply_openai_tunnel_configuration(
    snapshot: &mut DesktopStateSnapshot,
    config: &TunnelConfig,
) {
    let configuration = config.snapshot();
    snapshot.openai_tunnel_configured = configuration.is_configured();
    snapshot.openai_tunnel_config = configuration;
}

pub(super) fn same_server(left: &str, right: &str) -> bool {
    left.trim_end_matches('/')
        .eq_ignore_ascii_case(right.trim_end_matches('/'))
}

#[cfg(test)]
pub(super) fn same_project(left: &str, right: &str) -> bool {
    if cfg!(windows) {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}
