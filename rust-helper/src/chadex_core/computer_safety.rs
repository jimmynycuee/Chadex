use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{oneshot, Notify};

const MAX_PENDING_APPROVALS: usize = 16;
const MAX_AUDIT_EVENTS: usize = 64;
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ComputerControlMode {
    ReadOnly,
    AskBeforeControl,
    AllowSession,
}

impl Default for ComputerControlMode {
    fn default() -> Self {
        Self::AskBeforeControl
    }
}

impl ComputerControlMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "read_only" => Some(Self::ReadOnly),
            "ask_before_control" => Some(Self::AskBeforeControl),
            "allow_session" => Some(Self::AllowSession),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::AskBeforeControl => "ask_before_control",
            Self::AllowSession => "allow_session",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ComputerApprovalSummary {
    pub approval_id: String,
    pub action: String,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ComputerSafetyAuditEvent {
    pub sequence: u64,
    pub timestamp_ms: u64,
    pub event: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ComputerSafetySnapshot {
    pub mode: ComputerControlMode,
    pub stopped: bool,
    pub generation: u64,
    pub pending_approvals: Vec<ComputerApprovalSummary>,
    pub audit: Vec<ComputerSafetyAuditEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComputerDispatchPermit {
    generation: u64,
    action: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComputerAuthorization {
    Allowed(ComputerDispatchPermit),
    Denied {
        error_kind: &'static str,
        message: &'static str,
    },
}

struct PendingApproval {
    summary: ComputerApprovalSummary,
    generation: u64,
    response: oneshot::Sender<bool>,
}

struct ComputerSafetyState {
    mode: ComputerControlMode,
    stopped: bool,
    generation: u64,
    pending: HashMap<String, PendingApproval>,
    audit: VecDeque<ComputerSafetyAuditEvent>,
}

impl Default for ComputerSafetyState {
    fn default() -> Self {
        Self {
            mode: ComputerControlMode::AskBeforeControl,
            stopped: false,
            generation: 1,
            pending: HashMap::new(),
            audit: VecDeque::new(),
        }
    }
}

pub struct ComputerSafetyController {
    state: Mutex<ComputerSafetyState>,
    next_approval: AtomicU64,
    next_audit: AtomicU64,
    changed: Notify,
}

impl Default for ComputerSafetyController {
    fn default() -> Self {
        Self {
            state: Mutex::new(ComputerSafetyState::default()),
            next_approval: AtomicU64::new(1),
            next_audit: AtomicU64::new(1),
            changed: Notify::new(),
        }
    }
}

impl ComputerSafetyController {
    pub fn snapshot(&self) -> ComputerSafetySnapshot {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut pending_approvals = state
            .pending
            .values()
            .map(|pending| pending.summary.clone())
            .collect::<Vec<_>>();
        pending_approvals.sort_by_key(|approval| approval.created_at_ms);
        ComputerSafetySnapshot {
            mode: state.mode,
            stopped: state.stopped,
            generation: state.generation,
            pending_approvals,
            audit: state.audit.iter().cloned().collect(),
        }
    }

    pub fn begin_session(&self) {
        let responses = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let responses = drain_pending(&mut state);
            state.generation = state.generation.saturating_add(1);
            state.stopped = false;
            if state.mode == ComputerControlMode::AllowSession {
                state.mode = ComputerControlMode::AskBeforeControl;
            }
            let mode = state.mode.as_str().to_string();
            push_audit(
                &mut state,
                &self.next_audit,
                "session_started",
                None,
                Some(mode),
            );
            responses
        };
        deny_pending(responses);
        self.changed.notify_waiters();
    }

    pub fn set_mode(&self, mode: ComputerControlMode) -> ComputerSafetySnapshot {
        let responses = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let responses = drain_pending(&mut state);
            state.generation = state.generation.saturating_add(1);
            state.mode = mode;
            state.stopped = false;
            push_audit(
                &mut state,
                &self.next_audit,
                "mode_changed",
                None,
                Some(mode.as_str().to_string()),
            );
            responses
        };
        deny_pending(responses);
        self.changed.notify_waiters();
        self.snapshot()
    }

    pub fn stop(&self) -> ComputerSafetySnapshot {
        let responses = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let responses = drain_pending(&mut state);
            state.generation = state.generation.saturating_add(1);
            state.stopped = true;
            push_audit(
                &mut state,
                &self.next_audit,
                "emergency_stop",
                None,
                Some("user_stop".to_string()),
            );
            responses
        };
        deny_pending(responses);
        self.changed.notify_waiters();
        self.snapshot()
    }

    pub fn approve(&self, approval_id: &str) -> bool {
        self.resolve_approval(approval_id, true)
    }

    pub fn deny(&self, approval_id: &str) -> bool {
        self.resolve_approval(approval_id, false)
    }

    fn resolve_approval(&self, approval_id: &str, allowed: bool) -> bool {
        let pending = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some(pending) = state.pending.remove(approval_id) else {
                return false;
            };
            push_audit(
                &mut state,
                &self.next_audit,
                if allowed {
                    "approval_approved"
                } else {
                    "approval_denied"
                },
                Some(pending.summary.action.clone()),
                None,
            );
            pending
        };
        let _ = pending.response.send(allowed);
        self.changed.notify_waiters();
        true
    }

    pub async fn authorize(&self, action: &str) -> ComputerAuthorization {
        if !is_computer_control_action(action) {
            return ComputerAuthorization::Denied {
                error_kind: "computer_control_invalid_action",
                message: "Computer control action is invalid.",
            };
        }

        let (approval_id, generation, response) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.stopped {
                push_audit(
                    &mut state,
                    &self.next_audit,
                    "control_denied",
                    Some(action.to_string()),
                    Some("stopped".to_string()),
                );
                return ComputerAuthorization::Denied {
                    error_kind: "computer_control_stopped",
                    message: "Computer control is stopped by the user.",
                };
            }
            match state.mode {
                ComputerControlMode::ReadOnly => {
                    push_audit(
                        &mut state,
                        &self.next_audit,
                        "control_denied",
                        Some(action.to_string()),
                        Some("read_only".to_string()),
                    );
                    return ComputerAuthorization::Denied {
                        error_kind: "computer_control_read_only",
                        message: "Computer control is disabled in Read-only mode.",
                    };
                }
                ComputerControlMode::AllowSession => {
                    let generation = state.generation;
                    push_audit(
                        &mut state,
                        &self.next_audit,
                        "control_allowed",
                        Some(action.to_string()),
                        Some("allow_session".to_string()),
                    );
                    return ComputerAuthorization::Allowed(ComputerDispatchPermit {
                        generation,
                        action: action.to_string(),
                    });
                }
                ComputerControlMode::AskBeforeControl => {}
            }

            if state.pending.len() >= MAX_PENDING_APPROVALS {
                push_audit(
                    &mut state,
                    &self.next_audit,
                    "control_denied",
                    Some(action.to_string()),
                    Some("approval_queue_full".to_string()),
                );
                return ComputerAuthorization::Denied {
                    error_kind: "computer_control_approval_busy",
                    message: "Too many Computer control approvals are already pending.",
                };
            }

            let sequence = self.next_approval.fetch_add(1, Ordering::SeqCst);
            let approval_id = format!("computer_approval_{}_{}", now_ms(), sequence);
            let generation = state.generation;
            let (tx, rx) = oneshot::channel();
            state.pending.insert(
                approval_id.clone(),
                PendingApproval {
                    summary: ComputerApprovalSummary {
                        approval_id: approval_id.clone(),
                        action: action.to_string(),
                        created_at_ms: now_ms(),
                    },
                    generation,
                    response: tx,
                },
            );
            push_audit(
                &mut state,
                &self.next_audit,
                "approval_requested",
                Some(action.to_string()),
                None,
            );
            (approval_id, generation, rx)
        };
        self.changed.notify_waiters();

        let approved = match tokio::time::timeout(APPROVAL_TIMEOUT, response).await {
            Ok(Ok(approved)) => approved,
            _ => {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                if state
                    .pending
                    .get(&approval_id)
                    .is_some_and(|pending| pending.generation == generation)
                {
                    state.pending.remove(&approval_id);
                    push_audit(
                        &mut state,
                        &self.next_audit,
                        "approval_expired",
                        Some(action.to_string()),
                        None,
                    );
                }
                self.changed.notify_waiters();
                return ComputerAuthorization::Denied {
                    error_kind: "computer_control_approval_timeout",
                    message: "Computer control approval expired before the user responded.",
                };
            }
        };

        if !approved {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.stopped || state.generation != generation {
                let stopped = state.stopped;
                push_audit(
                    &mut state,
                    &self.next_audit,
                    "control_denied",
                    Some(action.to_string()),
                    Some("safety_state_changed".to_string()),
                );
                return ComputerAuthorization::Denied {
                    error_kind: if stopped {
                        "computer_control_stopped"
                    } else {
                        "computer_control_safety_changed"
                    },
                    message: if stopped {
                        "Computer control was stopped before dispatch."
                    } else {
                        "Computer control safety mode changed before dispatch."
                    },
                };
            }
            return ComputerAuthorization::Denied {
                error_kind: "computer_control_denied",
                message: "Computer control was denied by the user.",
            };
        }

        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.stopped || state.generation != generation {
            push_audit(
                &mut state,
                &self.next_audit,
                "control_denied",
                Some(action.to_string()),
                Some("safety_state_changed".to_string()),
            );
            return ComputerAuthorization::Denied {
                error_kind: "computer_control_stopped",
                message: "Computer control safety state changed before dispatch.",
            };
        }
        push_audit(
            &mut state,
            &self.next_audit,
            "control_allowed",
            Some(action.to_string()),
            Some("user_approved".to_string()),
        );
        ComputerAuthorization::Allowed(ComputerDispatchPermit {
            generation,
            action: action.to_string(),
        })
    }

    pub fn permit_is_current(&self, permit: &ComputerDispatchPermit) -> bool {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        !state.stopped && state.generation == permit.generation
    }

    pub async fn wait_until_invalid(&self, permit: &ComputerDispatchPermit) {
        loop {
            if !self.permit_is_current(permit) {
                return;
            }
            self.changed.notified().await;
        }
    }

    pub fn record_dispatch(&self, permit: &ComputerDispatchPermit) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        push_audit(
            &mut state,
            &self.next_audit,
            "control_dispatched",
            Some(permit.action.clone()),
            None,
        );
    }

    pub fn record_interrupted(&self, permit: &ComputerDispatchPermit) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        push_audit(
            &mut state,
            &self.next_audit,
            "control_dispatch_interrupted",
            Some(permit.action.clone()),
            Some("stop_during_dispatch".to_string()),
        );
    }

    pub fn record_batch_rejection(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        push_audit(
            &mut state,
            &self.next_audit,
            "control_denied",
            None,
            Some("batch_not_supported".to_string()),
        );
    }
}

fn drain_pending(state: &mut ComputerSafetyState) -> Vec<oneshot::Sender<bool>> {
    state
        .pending
        .drain()
        .map(|(_, pending)| pending.response)
        .collect()
}

fn deny_pending(responses: Vec<oneshot::Sender<bool>>) {
    for response in responses {
        let _ = response.send(false);
    }
}

fn push_audit(
    state: &mut ComputerSafetyState,
    next_audit: &AtomicU64,
    event: &str,
    action: Option<String>,
    reason: Option<String>,
) {
    state.audit.push_back(ComputerSafetyAuditEvent {
        sequence: next_audit.fetch_add(1, Ordering::SeqCst),
        timestamp_ms: now_ms(),
        event: event.to_string(),
        action,
        reason,
    });
    while state.audit.len() > MAX_AUDIT_EVENTS {
        state.audit.pop_front();
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub fn is_computer_control_action(action: &str) -> bool {
    matches!(
        action,
        "launch_application"
            | "activate_window"
            | "press"
            | "focus"
            | "scroll_to_element"
            | "key"
            | "input_text"
            | "pointer_move"
            | "pointer_click"
            | "write_clipboard"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_only_denies_without_pending_approval() {
        let safety = ComputerSafetyController::default();
        safety.set_mode(ComputerControlMode::ReadOnly);
        assert!(matches!(
            safety.authorize("press").await,
            ComputerAuthorization::Denied {
                error_kind: "computer_control_read_only",
                ..
            }
        ));
        assert!(safety.snapshot().pending_approvals.is_empty());
    }

    #[tokio::test]
    async fn allow_session_authorizes_until_stop_invalidates_permit() {
        let safety = ComputerSafetyController::default();
        safety.set_mode(ComputerControlMode::AllowSession);
        let ComputerAuthorization::Allowed(permit) = safety.authorize("press").await else {
            panic!("allow-session must authorize");
        };
        assert!(safety.permit_is_current(&permit));
        safety.stop();
        assert!(!safety.permit_is_current(&permit));
        assert!(matches!(
            safety.authorize("press").await,
            ComputerAuthorization::Denied {
                error_kind: "computer_control_stopped",
                ..
            }
        ));
    }

    #[test]
    fn allow_session_is_scoped_to_one_tunnel_session() {
        let safety = ComputerSafetyController::default();
        let allowed = safety.set_mode(ComputerControlMode::AllowSession);
        assert_eq!(allowed.mode, ComputerControlMode::AllowSession);

        safety.begin_session();
        let next_session = safety.snapshot();
        assert_eq!(next_session.mode, ComputerControlMode::AskBeforeControl);
        assert!(!next_session.stopped);
    }

    #[tokio::test]
    async fn ask_mode_waits_for_explicit_approval_without_storing_arguments() {
        let safety = std::sync::Arc::new(ComputerSafetyController::default());
        let authorizing = {
            let safety = std::sync::Arc::clone(&safety);
            tokio::spawn(async move { safety.authorize("input_text").await })
        };
        let approval = loop {
            if let Some(approval) = safety.snapshot().pending_approvals.first().cloned() {
                break approval;
            }
            tokio::task::yield_now().await;
        };
        assert_eq!(approval.action, "input_text");
        assert!(safety.approve(&approval.approval_id));
        let ComputerAuthorization::Allowed(permit) = authorizing.await.unwrap() else {
            panic!("approved request must be allowed");
        };
        assert!(safety.permit_is_current(&permit));
        let encoded = serde_json::to_string(&safety.snapshot()).unwrap();
        assert!(!encoded.contains("secret text"));
    }

    #[tokio::test]
    async fn stop_denies_pending_approval_and_invalidates_generation() {
        let safety = std::sync::Arc::new(ComputerSafetyController::default());
        let authorizing = {
            let safety = std::sync::Arc::clone(&safety);
            tokio::spawn(async move { safety.authorize("pointer_click").await })
        };
        while safety.snapshot().pending_approvals.is_empty() {
            tokio::task::yield_now().await;
        }
        let before = safety.snapshot().generation;
        let stopped = safety.stop();
        assert!(stopped.stopped);
        assert!(stopped.generation > before);
        assert!(stopped.pending_approvals.is_empty());
        assert!(matches!(
            authorizing.await.unwrap(),
            ComputerAuthorization::Denied { .. }
        ));
    }

    #[tokio::test]
    async fn stop_while_approval_is_pending_is_reported_as_stop_not_manual_deny() {
        let safety = std::sync::Arc::new(ComputerSafetyController::default());
        let authorizing = {
            let safety = std::sync::Arc::clone(&safety);
            tokio::spawn(async move { safety.authorize("key").await })
        };
        while safety.snapshot().pending_approvals.is_empty() {
            tokio::task::yield_now().await;
        }
        safety.stop();
        assert!(matches!(
            authorizing.await.unwrap(),
            ComputerAuthorization::Denied {
                error_kind: "computer_control_stopped",
                ..
            }
        ));
    }

    #[test]
    fn action_vocabulary_is_closed() {
        assert!(is_computer_control_action("launch_application"));
        assert!(is_computer_control_action("input_text"));
        assert!(is_computer_control_action("pointer_click"));
        assert!(!is_computer_control_action("shell"));
        assert!(!is_computer_control_action(""));
    }
}
