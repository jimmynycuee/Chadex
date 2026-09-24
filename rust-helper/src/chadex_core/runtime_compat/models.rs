use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Experience {
    Full,
    QuickShare,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ServerTopology {
    Local,
    Remote { url: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunnerTopology {
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Exposure {
    None,
    ExistingHttps { url: String },
    Cloudflare,
    OpenAiTunnel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Enrollment {
    ManagedPairing,
    SharedKey,
    ExistingProfile { profile: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeTopology {
    pub experience: Experience,
    pub server: ServerTopology,
    pub runner: RunnerTopology,
    pub exposure: Exposure,
    pub enrollment: Enrollment,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServerReadiness {
    Stopped,
    Starting,
    Ready,
    Error,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunnerReadiness {
    Stopped,
    Connecting,
    Ready,
    Error,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExposureReadiness {
    Disabled,
    Starting,
    LocalReady,
    RemoteReady,
    Degraded,
    Error,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectReadiness {
    None,
    Configured,
    ReloadRequired,
    Ready,
    Error,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessSummaryKind {
    ReadyForChatGpt,
    RuntimeStopped,
    RuntimeStarting,
    ServiceNeedsAttention,
    RunnerDisconnected,
    ProjectNotReady,
    RuntimeReadyLocalOnly,
    TunnelReadyWaitingForChatGpt,
    ConnectionUnverified,
    QuickShareStopped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReadinessNextActionKind {
    StartOrReconnectService,
    StartRunner,
    AddOrReloadProject,
    ChooseConnection,
    CheckConnection,
    RestartQuickShare,
    RestoreClipboardHandoff,
    RestartSecureTunnel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadinessSnapshot {
    pub server: ServerReadiness,
    pub runner: RunnerReadiness,
    pub exposure: ExposureReadiness,
    pub project: ProjectReadiness,
    pub runtime_ready: bool,
    pub ready_for_chatgpt: bool,
    pub summary_kind: ReadinessSummaryKind,
    pub next_action_kind: Option<ReadinessNextActionKind>,
    pub summary: String,
    pub next_action: Option<String>,
}

struct ReadinessDecision {
    summary_kind: ReadinessSummaryKind,
    next_action_kind: Option<ReadinessNextActionKind>,
    summary: &'static str,
    next_action: Option<&'static str>,
}

impl ReadinessDecision {
    fn evaluate(
        server: &ServerReadiness,
        runner: &RunnerReadiness,
        exposure: &ExposureReadiness,
        project: &ProjectReadiness,
        ready_for_chatgpt: bool,
    ) -> Self {
        if ready_for_chatgpt {
            return Self::new(
                ReadinessSummaryKind::ReadyForChatGpt,
                None,
                "Ready to use with ChatGPT",
                None,
            );
        }

        if server == &ServerReadiness::Stopped && runner == &RunnerReadiness::Stopped {
            return Self::new(
                ReadinessSummaryKind::RuntimeStopped,
                Some(ReadinessNextActionKind::StartOrReconnectService),
                "Runtime stopped",
                Some("Start the runtime to continue."),
            );
        }

        if server == &ServerReadiness::Starting
            || (server == &ServerReadiness::Ready && runner == &RunnerReadiness::Connecting)
        {
            return Self::new(
                ReadinessSummaryKind::RuntimeStarting,
                None,
                "Runtime starting",
                None,
            );
        }

        if server != &ServerReadiness::Ready {
            return Self::new(
                ReadinessSummaryKind::ServiceNeedsAttention,
                Some(ReadinessNextActionKind::StartOrReconnectService),
                "WebCodex Service needs attention",
                Some("Start or reconnect the WebCodex Service."),
            );
        }

        if runner != &RunnerReadiness::Ready {
            return Self::new(
                ReadinessSummaryKind::RunnerDisconnected,
                Some(ReadinessNextActionKind::StartRunner),
                "Runner is not connected",
                Some("Start the Runner and wait for it to connect."),
            );
        }

        if project != &ProjectReadiness::Ready {
            return Self::new(
                ReadinessSummaryKind::ProjectNotReady,
                Some(ReadinessNextActionKind::AddOrReloadProject),
                "Project is not ready",
                Some("Prepare and activate the selected project."),
            );
        }

        if matches!(
            exposure,
            ExposureReadiness::Disabled | ExposureReadiness::LocalReady
        ) {
            return Self::new(
                ReadinessSummaryKind::RuntimeReadyLocalOnly,
                Some(ReadinessNextActionKind::ChooseConnection),
                "Runtime ready on this computer",
                Some("Choose a ChatGPT connection in Connection."),
            );
        }

        Self::new(
            ReadinessSummaryKind::ConnectionUnverified,
            Some(ReadinessNextActionKind::CheckConnection),
            "ChatGPT connection is not verified",
            Some("Check the ChatGPT connection status."),
        )
    }

    fn new(
        summary_kind: ReadinessSummaryKind,
        next_action_kind: Option<ReadinessNextActionKind>,
        summary: &'static str,
        next_action: Option<&'static str>,
    ) -> Self {
        Self {
            summary_kind,
            next_action_kind,
            summary,
            next_action,
        }
    }
}

impl Default for ReadinessSnapshot {
    fn default() -> Self {
        aggregate_readiness(
            ServerReadiness::Unknown,
            RunnerReadiness::Unknown,
            ExposureReadiness::Unknown,
            ProjectReadiness::None,
        )
    }
}

pub fn aggregate_readiness(
    server: ServerReadiness,
    runner: RunnerReadiness,
    exposure: ExposureReadiness,
    project: ProjectReadiness,
) -> ReadinessSnapshot {
    let runtime_ready = server == ServerReadiness::Ready
        && runner == RunnerReadiness::Ready
        && project == ProjectReadiness::Ready;
    let ready_for_chatgpt = runtime_ready && exposure == ExposureReadiness::RemoteReady;
    let decision =
        ReadinessDecision::evaluate(&server, &runner, &exposure, &project, ready_for_chatgpt);

    ReadinessSnapshot {
        server,
        runner,
        exposure,
        project,
        runtime_ready,
        ready_for_chatgpt,
        summary_kind: decision.summary_kind,
        next_action_kind: decision.next_action_kind,
        summary: decision.summary.to_string(),
        next_action: decision.next_action.map(str::to_string),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectSelection {
    pub path: String,
    pub allowed_root: String,
    pub is_git_repository: bool,
    pub runtime_project_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BinaryInfo {
    pub directory: String,
    pub version: String,
    pub git_commit: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuickShareState {
    pub provider: String,
    pub project: String,
    pub mcp_url: Option<String>,
    pub clipboard_state: String,
    pub clipboard_contains: String,
    pub ready_for_chatgpt: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RegularTunnelStatus {
    Starting,
    Ready,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegularTunnelState {
    pub provider: String,
    pub status: RegularTunnelStatus,
    pub clipboard_state: String,
    pub clipboard_contains: String,
    pub ready_for_chatgpt: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DesktopOperationKind {
    LocalSetup,
    LocalProjectActivate,
    RemoteSetup,
    QuickShareStart,
    QuickShareStop,
    RegularTunnelStart,
    RegularTunnelStop,
    LocalRuntimeStop,
    RuntimeRefresh,
    RuntimeResume,
    TunnelProxyUpdate,
    TunnelConfigUpdate,
}

impl DesktopOperationKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalSetup => "local_setup",
            Self::LocalProjectActivate => "local_project_activate",
            Self::RemoteSetup => "remote_setup",
            Self::QuickShareStart => "quick_share_start",
            Self::QuickShareStop => "quick_share_stop",
            Self::RegularTunnelStart => "regular_tunnel_start",
            Self::RegularTunnelStop => "regular_tunnel_stop",
            Self::LocalRuntimeStop => "local_runtime_stop",
            Self::RuntimeRefresh => "runtime_refresh",
            Self::RuntimeResume => "runtime_resume",
            Self::TunnelProxyUpdate => "tunnel_proxy_update",
            Self::TunnelConfigUpdate => "tunnel_config_update",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DesktopOperationPhase {
    Running,
    Cancelling,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DesktopOperationSnapshot {
    pub id: String,
    pub kind: DesktopOperationKind,
    pub phase: DesktopOperationPhase,
    pub started_at_ms: u64,
    pub cancellable: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RegularConnectionPreference {
    #[default]
    NoChatGpt,
    OpenAiTunnel,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TunnelProxyMode {
    #[default]
    Auto,
    Direct,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct TunnelProxyConfig {
    #[serde(default)]
    pub mode: TunnelProxyMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TunnelProxySnapshot {
    pub mode: TunnelProxyMode,
    pub custom_url: Option<String>,
    pub effective_source: String,
    pub effective_url: Option<String>,
    pub detected_url: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TunnelConfigSource {
    #[default]
    Environment,
    File,
    Memory,
    Invalid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct OpenAiTunnelConfigSnapshot {
    pub tunnel_id_present: bool,
    pub api_key_present: bool,
    #[serde(default)]
    pub source: TunnelConfigSource,
    #[serde(default)]
    pub saved_tunnel_id: Option<String>,
    #[serde(default)]
    pub effective_tunnel_id: Option<String>,
}

impl OpenAiTunnelConfigSnapshot {
    pub fn is_configured(&self) -> bool {
        self.tunnel_id_present && self.api_key_present
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PowerShellRuntimeSnapshot {
    pub pwsh_available: bool,
    pub windows_powershell_available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ChatGptActivitySnapshot {
    pub observed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_meaningful_activity_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DesktopStateSnapshot {
    pub topology: Option<RuntimeTopology>,
    pub readiness: ReadinessSnapshot,
    pub project: Option<ProjectSelection>,
    pub binaries: Option<BinaryInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub powershell_runtime: Option<PowerShellRuntimeSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chatgpt_activity: Option<ChatGptActivitySnapshot>,
    pub quick_share: Option<QuickShareState>,
    pub regular_tunnel: Option<RegularTunnelState>,
    pub current_operation: Option<DesktopOperationSnapshot>,
    pub activity_sequence: u64,
    pub openai_tunnel_configured: bool,
    pub openai_tunnel_config: OpenAiTunnelConfigSnapshot,
    pub regular_tunnel_available: bool,
    pub runtime_autostart: bool,
    pub preferred_connection: RegularConnectionPreference,
    pub tunnel_proxy: TunnelProxySnapshot,
}

impl Default for DesktopStateSnapshot {
    fn default() -> Self {
        Self {
            topology: None,
            readiness: ReadinessSnapshot::default(),
            project: None,
            binaries: None,
            powershell_runtime: None,
            chatgpt_activity: None,
            quick_share: None,
            regular_tunnel: None,
            current_operation: None,
            activity_sequence: 0,
            openai_tunnel_configured: false,
            openai_tunnel_config: OpenAiTunnelConfigSnapshot::default(),
            regular_tunnel_available: false,
            runtime_autostart: false,
            preferred_connection: RegularConnectionPreference::NoChatGpt,
            tunnel_proxy: TunnelProxySnapshot {
                mode: TunnelProxyMode::Auto,
                custom_url: None,
                effective_source: "direct".to_string(),
                effective_url: None,
                detected_url: None,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct StoredDesktopConfig {
    pub topology: Option<RuntimeTopology>,
    pub project: Option<ProjectSelection>,
    pub runtime: Option<StoredRuntime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_autostart: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_connection: Option<RegularConnectionPreference>,
    #[serde(default)]
    pub tunnel_proxy: TunnelProxyConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredRuntime {
    pub server_url: String,
    pub server_env_file: Option<PathBuf>,
    pub runner_config: Option<PathBuf>,
    pub user_token_file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_client_id: Option<String>,
    pub project_id: Option<String>,
    pub runtime_project_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readiness_requires_service_runner_and_project() {
        let local = aggregate_readiness(
            ServerReadiness::Ready,
            RunnerReadiness::Ready,
            ExposureReadiness::LocalReady,
            ProjectReadiness::Ready,
        );
        assert!(local.runtime_ready);
        assert!(!local.ready_for_chatgpt);
        assert_eq!(
            local.summary_kind,
            ReadinessSummaryKind::RuntimeReadyLocalOnly
        );

        let remote = aggregate_readiness(
            ServerReadiness::Ready,
            RunnerReadiness::Ready,
            ExposureReadiness::RemoteReady,
            ProjectReadiness::Ready,
        );
        assert!(remote.ready_for_chatgpt);
    }

    #[test]
    fn readiness_keeps_stopped_starting_and_error_distinct() {
        let cases = [
            (
                ServerReadiness::Stopped,
                RunnerReadiness::Stopped,
                ReadinessSummaryKind::RuntimeStopped,
            ),
            (
                ServerReadiness::Starting,
                RunnerReadiness::Stopped,
                ReadinessSummaryKind::RuntimeStarting,
            ),
            (
                ServerReadiness::Error,
                RunnerReadiness::Stopped,
                ReadinessSummaryKind::ServiceNeedsAttention,
            ),
        ];

        for (server, runner, expected) in cases {
            let snapshot = aggregate_readiness(
                server,
                runner,
                ExposureReadiness::LocalReady,
                ProjectReadiness::Configured,
            );
            assert_eq!(snapshot.summary_kind, expected);
            assert!(!snapshot.runtime_ready);
        }
    }

    #[test]
    fn stored_runtime_contract_roundtrips() {
        let stored = StoredRuntime {
            server_url: "http://127.0.0.1:8765".to_string(),
            server_env_file: Some(PathBuf::from("/tmp/server.env")),
            runner_config: Some(PathBuf::from("/tmp/runner.toml")),
            user_token_file: Some(PathBuf::from("/tmp/token")),
            runner_client_id: Some("runner-a".to_string()),
            project_id: Some("repo".to_string()),
            runtime_project_id: Some("agent:runner-a:repo".to_string()),
        };
        let json = serde_json::to_string(&stored).unwrap();
        let decoded: StoredRuntime = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, stored);
    }
}
