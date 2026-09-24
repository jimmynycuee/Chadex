use crate::chadex_core::activity as core_activity;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

const HISTORY_LIMIT: usize = 200;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivityLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivityEventKind {
    ProcessStarted,
    ProcessExited,
    ProcessObservationFailed,
    ProcessStopping,
    ProcessStopped,
    LocalSetupPreparing,
    LocalRuntimeReady,
    RemoteConnecting,
    RemoteConnected,
    QuickShareStarting,
    QuickShareReady,
    QuickShareStopped,
    RegularTunnelStarting,
    RegularTunnelReady,
    RegularTunnelStopped,
    RuntimeStopped,
    StateRecovered,
    ProjectActivated,
    OperationStarted,
    OperationCancelRequested,
    OperationCancelled,
    OperationFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActivityEntry {
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub source: String,
    pub level: ActivityLevel,
    pub event_kind: ActivityEventKind,
    pub message: String,
}

#[derive(Clone, Default)]
pub struct ActivityLog {
    shared: Arc<Mutex<ActivityState>>,
}

#[derive(Default)]
struct ActivityState {
    sequence: u64,
    entries: VecDeque<ActivityEntry>,
}

impl ActivityLog {
    pub fn push(
        &self,
        event_kind: ActivityEventKind,
        source: impl Into<String>,
        level: ActivityLevel,
        message: impl AsRef<str>,
    ) {
        let mut state = self
            .shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        state.sequence = state.sequence.saturating_add(1);
        let sequence = state.sequence;
        if state.entries.len() >= HISTORY_LIMIT {
            state.entries.pop_front();
        }
        state.entries.push_back(ActivityEntry {
            sequence,
            timestamp_ms: timestamp_ms(),
            source: source.into(),
            level,
            event_kind,
            message: sanitize_message(message.as_ref()),
        });
    }

    pub fn snapshot(&self) -> Vec<ActivityEntry> {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entries
            .iter()
            .cloned()
            .collect()
    }

    pub fn latest_sequence(&self) -> u64 {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .sequence
    }
}

pub fn sanitize_message(message: &str) -> String {
    core_activity::sanitize_message(message)
}

fn timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_keeps_only_the_latest_entries() {
        let log = ActivityLog::default();
        for index in 0..250 {
            log.push(
                ActivityEventKind::ProcessStarted,
                "test",
                ActivityLevel::Info,
                format!("message {index}"),
            );
        }

        let entries = log.snapshot();
        assert_eq!(entries.len(), HISTORY_LIMIT);
        assert_eq!(entries.first().unwrap().sequence, 51);
        assert_eq!(entries.last().unwrap().sequence, 250);
        assert_eq!(log.latest_sequence(), 250);
    }

    #[test]
    fn messages_use_chadex_redaction() {
        let safe = sanitize_message(
            "Authorization: Bearer abc wc_pair_secret wc_pat_secret wc_agent_secret webcodex_temporary_secret",
        );
        assert!(!safe.contains("secret"));
        assert!(!safe.contains("Bearer abc"));
    }
}
