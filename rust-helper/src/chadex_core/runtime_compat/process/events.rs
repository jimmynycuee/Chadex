use serde_json::Value;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::Notify;

const EVENT_CAPACITY: usize = 64;
const CRITICAL_RESERVE: usize = 8;

#[derive(Default)]
struct EventQueueState {
    queue: VecDeque<Value>,
    closed: bool,
    dropped_progress: u64,
    dropped_critical: u64,
}

#[derive(Clone)]
pub(super) struct MachineEventEmitter {
    state: Arc<Mutex<EventQueueState>>,
    notify: Arc<Notify>,
}

pub(crate) struct MachineEventInbox {
    state: Arc<Mutex<EventQueueState>>,
    notify: Arc<Notify>,
}

pub(super) fn channel() -> (MachineEventEmitter, MachineEventInbox) {
    let state = Arc::new(Mutex::new(EventQueueState::default()));
    let notify = Arc::new(Notify::new());
    (
        MachineEventEmitter {
            state: Arc::clone(&state),
            notify: Arc::clone(&notify),
        },
        MachineEventInbox { state, notify },
    )
}

impl MachineEventInbox {
    pub(crate) async fn recv(&mut self) -> Option<Value> {
        loop {
            let notified = self.notify.notified();
            {
                let mut state = self
                    .state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());

                if state.dropped_critical > 0 {
                    let dropped = std::mem::take(&mut state.dropped_critical);
                    return Some(serde_json::json!({
                        "event": "machine_event_overflow",
                        "dropped_critical": dropped,
                    }));
                }
                if let Some(value) = state.queue.pop_front() {
                    return Some(value);
                }
                if state.closed {
                    return None;
                }
            }
            notified.await;
        }
    }
}

impl MachineEventEmitter {
    pub(super) fn send(&self, value: Value) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.closed {
            return;
        }

        if is_progress(&value) {
            let progress_limit = EVENT_CAPACITY.saturating_sub(CRITICAL_RESERVE);
            if state.queue.len() >= progress_limit {
                if let Some(existing) = state
                    .queue
                    .iter_mut()
                    .rev()
                    .find(|event| is_progress(event))
                {
                    *existing = value;
                } else {
                    state.dropped_progress = state.dropped_progress.saturating_add(1);
                }
                return;
            }
            state.queue.push_back(value);
        } else {
            if state.queue.len() >= EVENT_CAPACITY {
                if let Some(index) = state.queue.iter().position(is_progress) {
                    state.queue.remove(index);
                    state.dropped_progress = state.dropped_progress.saturating_add(1);
                } else if is_terminal(&value) {
                    if let Some(index) = state.queue.iter().position(|event| !is_terminal(event)) {
                        state.queue.remove(index);
                    } else {
                        state.queue.pop_front();
                    }
                    state.dropped_critical = state.dropped_critical.saturating_add(1);
                } else {
                    state.dropped_critical = state.dropped_critical.saturating_add(1);
                    drop(state);
                    self.notify.notify_one();
                    return;
                }
            }
            state.queue.push_back(value);
        }

        drop(state);
        self.notify.notify_one();
    }

    pub(super) fn close(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.closed = true;
        drop(state);
        self.notify.notify_waiters();
    }
}

fn is_progress(value: &Value) -> bool {
    value.get("event").and_then(Value::as_str) == Some("progress")
}

fn is_terminal(value: &Value) -> bool {
    matches!(
        value.get("event").and_then(Value::as_str),
        Some("ready" | "error" | "failed" | "stopped" | "exit" | "exited" | "terminal")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn progress_flood_stays_bounded_without_losing_ready() {
        let (sender, mut receiver) = channel();
        for sequence in 0..10_000_u64 {
            sender.send(serde_json::json!({
                "event": "progress",
                "sequence": sequence,
            }));
        }
        sender.send(serde_json::json!({ "event": "ready", "schema_version": 1 }));
        sender.close();

        let queued = sender
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .queue
            .len();
        assert!(queued <= EVENT_CAPACITY);

        let mut ready = false;
        while let Some(value) = receiver.recv().await {
            if value.get("event").and_then(Value::as_str) == Some("ready") {
                ready = true;
                break;
            }
        }
        assert!(ready);
    }

    #[tokio::test]
    async fn critical_loss_is_explicit_and_terminal_survives() {
        let (sender, mut receiver) = channel();
        for sequence in 0..EVENT_CAPACITY {
            sender.send(serde_json::json!({
                "event": "diagnostic",
                "sequence": sequence,
            }));
        }
        sender.send(serde_json::json!({ "event": "terminal", "status": "failed" }));
        sender.close();

        let mut overflow = false;
        let mut terminal = false;
        while let Some(value) = receiver.recv().await {
            match value.get("event").and_then(Value::as_str) {
                Some("machine_event_overflow") => overflow = true,
                Some("terminal") => terminal = true,
                _ => {}
            }
        }
        assert!(overflow);
        assert!(terminal);
    }
}
