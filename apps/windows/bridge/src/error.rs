use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable, secret-free errors. Never contains paths, parameters, stderr,
/// response bodies, or the helper's free-form message/recovery/details.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct BridgeError {
    pub code: ErrorCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub helper_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub received_protocol: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    HelperMissing,
    RuntimeDirectoryMissing,
    DataDirectoryUnavailable,
    SpawnFailed,
    TransportStartFailed,
    NotRunning,
    TooManyPendingRequests,
    RequestTooLarge,
    ResponseTooLarge,
    InvalidResponse,
    ProtocolMismatch,
    Backend,
    RequestTimeout,
    WriteFailed,
    ReadFailed,
    Disconnected,
    HelperDied,
    ProcessWaitFailed,
    ShutdownForced,
    ShutdownFailed,
}

impl BridgeError {
    pub(crate) fn new(code: ErrorCode) -> Self {
        Self {
            code,
            helper_code: None,
            received_protocol: None,
            exit_code: None,
        }
    }

    pub(crate) fn backend(code: &str) -> Self {
        let mut error = Self::new(ErrorCode::Backend);
        // Finite W2-compatible codes; even a malicious helper cannot insert
        // arbitrary credentials into a serialized error/health snapshot.
        let safe = match code {
            "invalid_params"
            | "method_not_found"
            | "protocol_version_incompatible"
            | "project_not_selected"
            | "project_activation_incomplete"
            | "data_directory_unavailable"
            | "home_directory_unavailable"
            | "resource_directory_unavailable"
            | "invalid_arguments"
            | "local_port_unavailable"
            | "binary_missing"
            | "binary_directory_missing"
            | "binary_directory_invalid"
            | "binary_version_mismatch"
            | "binary_version_unverifiable"
            | "bundled_runtime_missing"
            | "runtime_binary_unreadable"
            | "runtime_bundle_changed"
            | "binary_probe_failed"
            | "project_activation_capability_unavailable"
            | "project_activation_restart_required"
            | "project_activation_reconcile_required"
            | "project_activation_config_conflict"
            | "runner_config_concurrent_change"
            | "webcodex_contract_invalid"
            | "webcodex_command_failed"
            | "webcodex_command_input_failed"
            | "webcodex_command_start_failed"
            | "webcodex_command_wait_failed"
            | "webcodex_command_timeout"
            | "command_exit_nonzero"
            | "timeout"
            | "outcome_unknown"
            | "capability_unavailable"
            | "unsupported_resource"
            | "unsupported_executable_type"
            | "spawn_failed"
            | "permission_denied"
            | "agent_offline"
            | "session_guard_denied"
            | "session_closed"
            | "runtime_error"
            | "tool_failure" => code,
            _ => "unclassified",
        };
        error.helper_code = Some(safe.to_owned());
        error
    }
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Debug contains only finite codes and scalar metadata.
        write!(f, "Chadex bridge: {:?}", self.code)
    }
}

impl std::error::Error for BridgeError {}
