use crate::chadex_core::activity::RuntimeActivityEntry;
use crate::chadex_core::runtime_compat::activity::{
    ActivityEntry, ActivityEventKind, ActivityLevel,
};
use crate::chadex_core::runtime_compat::error::DesktopError;
use crate::chadex_core::runtime_compat::models::ReadinessSummaryKind;
use crate::chadex_core::ChadexError;
use serde_json::Value;

pub(crate) struct RuntimeReadinessPresentation {
    pub summary: String,
    pub next_action: Option<String>,
}

pub(crate) fn map_compat_error(error: DesktopError) -> ChadexError {
    ChadexError {
        code: error.code,
        message: present_compat_text(&error.message),
        recovery: present_compat_text(&error.next_action),
        details: error.details.map(present_compat_value),
    }
}

pub(crate) fn map_compat_activity(entry: ActivityEntry) -> RuntimeActivityEntry {
    let message = match entry.event_kind {
        ActivityEventKind::LocalSetupPreparing => "Preparing the Chadex local runtime".to_string(),
        ActivityEventKind::LocalRuntimeReady => "Chadex local runtime is ready".to_string(),
        ActivityEventKind::RemoteConnecting => {
            "Connecting this computer to the existing runtime Server".to_string()
        }
        ActivityEventKind::RemoteConnected => {
            "This computer is connected to the existing runtime Server".to_string()
        }
        _ => present_compat_text(&entry.message),
    };

    RuntimeActivityEntry {
        sequence: entry.sequence,
        timestamp_ms: entry.timestamp_ms,
        source: if entry.source == "desktop" {
            "chadex".to_string()
        } else {
            present_compat_text(&entry.source)
        },
        level: match entry.level {
            ActivityLevel::Info => "info",
            ActivityLevel::Warning => "warning",
            ActivityLevel::Error => "error",
        },
        event_kind: match entry.event_kind {
            ActivityEventKind::ProcessStarted => "process_started",
            ActivityEventKind::ProcessExited => "process_exited",
            ActivityEventKind::ProcessObservationFailed => "process_observation_failed",
            ActivityEventKind::ProcessStopping => "process_stopping",
            ActivityEventKind::ProcessStopped => "process_stopped",
            ActivityEventKind::LocalSetupPreparing => "local_setup_preparing",
            ActivityEventKind::LocalRuntimeReady => "local_runtime_ready",
            ActivityEventKind::RemoteConnecting => "remote_connecting",
            ActivityEventKind::RemoteConnected => "remote_connected",
            ActivityEventKind::QuickShareStarting => "quick_share_starting",
            ActivityEventKind::QuickShareReady => "quick_share_ready",
            ActivityEventKind::QuickShareStopped => "quick_share_stopped",
            ActivityEventKind::RegularTunnelStarting => "regular_tunnel_starting",
            ActivityEventKind::RegularTunnelReady => "regular_tunnel_ready",
            ActivityEventKind::RegularTunnelStopped => "regular_tunnel_stopped",
            ActivityEventKind::RuntimeStopped => "runtime_stopped",
            ActivityEventKind::StateRecovered => "state_recovered",
            ActivityEventKind::ProjectActivated => "project_activated",
            ActivityEventKind::OperationStarted => "operation_started",
            ActivityEventKind::OperationCancelRequested => "operation_cancel_requested",
            ActivityEventKind::OperationCancelled => "operation_cancelled",
            ActivityEventKind::OperationFailed => "operation_failed",
        },
        message,
    }
}

pub(crate) fn present_readiness(kind: &ReadinessSummaryKind) -> RuntimeReadinessPresentation {
    let (summary, next_action) = match kind {
        ReadinessSummaryKind::ReadyForChatGpt => ("Ready to use with ChatGPT", None),
        ReadinessSummaryKind::RuntimeStopped => (
            "Local runtime is stopped",
            Some("Start the local runtime to continue."),
        ),
        ReadinessSummaryKind::RuntimeStarting => ("Local runtime is starting", None),
        ReadinessSummaryKind::ServiceNeedsAttention => (
            "Chadex local service needs attention",
            Some("Start or reconnect the local service."),
        ),
        ReadinessSummaryKind::RunnerDisconnected => (
            "Runner is not connected",
            Some("Start the Runner and wait for it to connect."),
        ),
        ReadinessSummaryKind::ProjectNotReady => (
            "Project is not ready",
            Some("Prepare and activate the selected project."),
        ),
        ReadinessSummaryKind::RuntimeReadyLocalOnly => (
            "Runtime ready on this computer",
            Some("Choose a ChatGPT connection in Connection."),
        ),
        ReadinessSummaryKind::TunnelReadyWaitingForChatGpt => (
            "Secure connection is ready",
            Some("Finish verification from ChatGPT."),
        ),
        ReadinessSummaryKind::ConnectionUnverified => (
            "ChatGPT connection is not verified",
            Some("Check the ChatGPT connection status."),
        ),
        ReadinessSummaryKind::QuickShareStopped => (
            "Quick Share is stopped",
            Some("Restart Quick Share to continue."),
        ),
    };

    RuntimeReadinessPresentation {
        summary: summary.to_string(),
        next_action: next_action.map(str::to_string),
    }
}

fn present_compat_value(value: Value) -> Value {
    match value {
        Value::String(value) => Value::String(present_compat_text(&value)),
        Value::Array(values) => {
            Value::Array(values.into_iter().map(present_compat_value).collect())
        }
        Value::Object(values) => Value::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, present_compat_value(value)))
                .collect(),
        ),
        other => other,
    }
}

fn present_compat_text(value: &str) -> String {
    [
        (
            "WEBCODEX_DESKTOP_BIN_DIR",
            "runtime binary directory override",
        ),
        ("WebCodex Desktop", "Chadex"),
        ("WebCodex Service", "Chadex local service"),
        ("WebCodex Server", "runtime Server"),
        ("WebCodex runtime", "Chadex local runtime"),
        ("WebCodex binaries", "bundled runtime components"),
        ("WebCodex command", "runtime command"),
        ("Desktop-owned", "Chadex-owned"),
        ("WebCodex", "runtime"),
        ("webcodex", "runtime"),
        ("Desktop", "Chadex"),
    ]
    .into_iter()
    .fold(value.to_string(), |text, (from, to)| text.replace(from, to))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn compatibility_error_translation_hides_upstream_branding() {
        let error = DesktopError::new(
            "server_unreachable",
            "WebCodex Service did not become ready",
            "Reinstall WebCodex Desktop and retry.",
        )
        .with_details(json!({
            "binary": "webcodex-server",
            "hint": "Set WEBCODEX_DESKTOP_BIN_DIR explicitly."
        }));

        let translated = map_compat_error(error);
        let serialized = format!(
            "{} {:?}",
            translated.message,
            translated.details.expect("translated details")
        );
        assert!(!serialized.to_ascii_lowercase().contains("webcodex"));
        assert!(!translated
            .recovery
            .to_ascii_lowercase()
            .contains("webcodex"));
    }

    #[test]
    fn typed_readiness_translation_is_chadex_owned() {
        let presentation = present_readiness(&ReadinessSummaryKind::ServiceNeedsAttention);
        assert_eq!(presentation.summary, "Chadex local service needs attention");
        assert_eq!(
            presentation.next_action.as_deref(),
            Some("Start or reconnect the local service.")
        );
    }

    #[test]
    fn typed_activity_translation_replaces_compatibility_message_and_source() {
        let entry = ActivityEntry {
            sequence: 1,
            timestamp_ms: 2,
            source: "desktop".to_string(),
            level: ActivityLevel::Info,
            event_kind: ActivityEventKind::LocalSetupPreparing,
            message: "Preparing WebCodex on this computer".to_string(),
        };

        let translated = map_compat_activity(entry);
        assert_eq!(translated.source, "chadex");
        assert_eq!(translated.message, "Preparing the Chadex local runtime");
        assert!(!translated.message.contains("WebCodex"));
    }
}
