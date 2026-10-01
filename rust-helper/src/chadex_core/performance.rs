use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PERFORMANCE_TRACE_LIMIT: usize = 100;
const LIFECYCLE_TRACE_LIMIT: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct McpPerformanceTrace {
    pub sequence: u64,
    pub started_at_ms: u64,
    #[serde(default)]
    pub finished_at_ms: Option<u64>,
    #[serde(default)]
    pub request_id_hashes: Vec<String>,
    #[serde(default)]
    pub server_trace_id: Option<String>,
    pub methods: Vec<String>,
    pub tool_names: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_failed: Option<bool>,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub status_code: Option<u16>,
    pub ingress_pre_backend_us: u64,
    pub backend_headers_us: u64,
    pub response_stream_us: u64,
    pub total_us: u64,
    pub completion: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LifecyclePerformanceTrace {
    pub sequence: u64,
    pub started_at_ms: u64,
    pub operation: String,
    pub phase: String,
    pub total_us: u64,
    pub completion: String,
}

#[derive(Default)]
pub struct PerformanceTraceStore {
    inner: Mutex<PerformanceTraceState>,
}

#[derive(Default)]
struct PerformanceTraceState {
    next_sequence: u64,
    entries: VecDeque<McpPerformanceTrace>,
    next_lifecycle_sequence: u64,
    lifecycle_entries: VecDeque<LifecyclePerformanceTrace>,
}

impl PerformanceTraceStore {
    pub fn push(&self, mut trace: McpPerformanceTrace) -> u64 {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if trace.sequence != 0 {
            if let Some(entry) = inner.entries.iter_mut().find(|entry| entry.sequence == trace.sequence) {
                let sequence = trace.sequence;
                *entry = trace;
                return sequence;
            }
        }
        inner.next_sequence = inner.next_sequence.saturating_add(1);
        trace.sequence = inner.next_sequence;
        let sequence = trace.sequence;
        inner.entries.push_back(trace);
        while inner.entries.len() > PERFORMANCE_TRACE_LIMIT {
            inner.entries.pop_front();
        }
        sequence
    }

    pub fn begin_request(self: &Arc<Self>, trace: McpPerformanceTrace) -> PendingMcpTrace {
        let sequence = self.push(trace);
        PendingMcpTrace { store: Arc::clone(self), sequence: Some(sequence) }
    }

    pub fn snapshot(&self, limit: usize) -> Vec<McpPerformanceTrace> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let limit = limit.clamp(1, PERFORMANCE_TRACE_LIMIT);
        let start = inner.entries.len().saturating_sub(limit);
        inner.entries.iter().skip(start).cloned().collect()
    }

    pub fn push_lifecycle(&self, mut trace: LifecyclePerformanceTrace) {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.next_lifecycle_sequence = inner.next_lifecycle_sequence.saturating_add(1);
        trace.sequence = inner.next_lifecycle_sequence;
        inner.lifecycle_entries.push_back(trace);
        while inner.lifecycle_entries.len() > LIFECYCLE_TRACE_LIMIT {
            inner.lifecycle_entries.pop_front();
        }
    }

    pub fn lifecycle_snapshot(&self, limit: usize) -> Vec<LifecyclePerformanceTrace> {
        let inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let limit = limit.clamp(1, LIFECYCLE_TRACE_LIMIT);
        let start = inner.lifecycle_entries.len().saturating_sub(limit);
        inner
            .lifecycle_entries
            .iter()
            .skip(start)
            .cloned()
            .collect()
    }
}

/// Covers cancellation while awaiting upstream headers. Once a response stream
/// takes over, its existing Drop handler owns completion of the same record.
pub struct PendingMcpTrace {
    store: Arc<PerformanceTraceStore>,
    sequence: Option<u64>,
}

impl PendingMcpTrace {
    pub fn sequence(&self) -> u64 { self.sequence.unwrap_or(0) }
    pub fn hand_off(&mut self) { self.sequence = None; }
}

impl Drop for PendingMcpTrace {
    fn drop(&mut self) {
        let Some(sequence) = self.sequence.take() else { return; };
        let mut inner = self.store.inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(entry) = inner.entries.iter_mut().find(|entry| entry.sequence == sequence) {
            if entry.finished_at_ms.is_none() {
                let finished = now_ms();
                entry.finished_at_ms = Some(finished);
                entry.total_us = finished.saturating_sub(entry.started_at_ms).saturating_mul(1000);
                entry.completion = "client_dropped".to_string();
            }
        }
    }
}

pub fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_trace_history_is_bounded() {
        let store = PerformanceTraceStore::default();
        for index in 0..125 {
            store.push_lifecycle(LifecyclePerformanceTrace {
                sequence: 0,
                started_at_ms: index,
                operation: "connect".to_string(),
                phase: "runtime_ensure".to_string(),
                total_us: index,
                completion: "completed".to_string(),
            });
        }

        let entries = store.lifecycle_snapshot(100);
        assert_eq!(entries.len(), LIFECYCLE_TRACE_LIMIT);
        assert_eq!(entries.first().map(|entry| entry.sequence), Some(26));
        assert_eq!(entries.last().map(|entry| entry.sequence), Some(125));
    }

    #[test]
    fn trace_history_is_bounded() {
        let store = PerformanceTraceStore::default();
        for index in 0..125 {
            store.push(McpPerformanceTrace {
                sequence: 0,
                started_at_ms: index,
                methods: vec!["tools/call".to_string()],
                finished_at_ms: Some(index),
                request_id_hashes: Vec::new(),
                server_trace_id: None,
                tool_names: vec!["read_files".to_string()],
                tool_failed: None,
                request_bytes: 1,
                response_bytes: 2,
                status_code: Some(200),
                ingress_pre_backend_us: 3,
                backend_headers_us: 4,
                response_stream_us: 5,
                total_us: 12,
                completion: "completed".to_string(),
            });
        }

        let entries = store.snapshot(100);
        assert_eq!(entries.len(), PERFORMANCE_TRACE_LIMIT);
        assert_eq!(entries.first().map(|entry| entry.sequence), Some(26));
        assert_eq!(entries.last().map(|entry| entry.sequence), Some(125));
    }
}
