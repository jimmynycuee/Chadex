use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

const JOURNAL_BYTES: u64 = 256 * 1024;
const JOURNAL_ENTRIES: usize = 100;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackendFailureKind {
    ConnectionRefused,
    ConnectTimeout,
    HeadersInterrupted,
    StreamInterrupted,
    ResponseTooLarge,
}

impl BackendFailureKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConnectionRefused => "backend_connection_refused",
            Self::ConnectTimeout => "backend_connect_timeout",
            Self::HeadersInterrupted => "backend_headers_interrupted",
            Self::StreamInterrupted => "backend_stream_interrupted",
            Self::ResponseTooLarge => "backend_response_too_large",
        }
    }
    pub fn not_started(self) -> bool { self == Self::ConnectionRefused }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryOutcome { Started, InFlight, Cooldown, Exhausted, Unavailable, Ready, Failed, TimedOut }

/// Deliberately contains no request payload, URL, header, credential or raw error.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TransportDiagnostic {
    pub at_ms: u64,
    pub trace_sequence: u64,
    pub request_id_hashes: Vec<String>,
    pub error_kind: BackendFailureKind,
    pub os_error_code: Option<i32>,
    pub elapsed_us: u64,
    pub generation: u64,
    pub recovery: RecoveryOutcome,
}

#[derive(Default)]
pub(super) struct TransportJournal {
    path: Option<PathBuf>,
    entries: VecDeque<TransportDiagnostic>,
}

impl TransportJournal {
    pub(super) fn open(path: PathBuf) -> Self {
        let mut journal = Self { path: Some(path.clone()), entries: VecDeque::new() };
        for file in [path.with_extension("jsonl.1"), path] {
            if let Ok(file) = fs::File::open(file) {
                // Never load an unbounded or externally enlarged journal.
                let reader = BufReader::new(std::io::Read::take(file, JOURNAL_BYTES));
                for line in reader.lines().map_while(Result::ok) {
                    if let Ok(entry) = serde_json::from_str::<TransportDiagnostic>(&line) {
                        journal.entries.push_back(entry);
                        while journal.entries.len() > JOURNAL_ENTRIES { journal.entries.pop_front(); }
                    }
                }
            }
        }
        journal
    }

    pub(super) fn push(&mut self, entry: TransportDiagnostic) {
        if let Some(path) = &self.path {
            if let Ok(mut bytes) = serde_json::to_vec(&entry) {
                bytes.push(b'\n');
                let can_write = if fs::metadata(path).map(|m| m.len()).unwrap_or(0) + bytes.len() as u64 > JOURNAL_BYTES {
                    let rotated = path.with_extension("jsonl.1");
                    // Windows cannot rename over an existing destination.
                    let _ = fs::remove_file(&rotated);
                    fs::rename(path, rotated).is_ok()
                } else { true };
                let mut options = OpenOptions::new();
                options.create(true).append(true);
                #[cfg(unix)] {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                if can_write {
                    if let Ok(mut file) = options.open(path) { let _ = file.write_all(&bytes); }
                }
            }
        }
        self.entries.push_back(entry);
        while self.entries.len() > JOURNAL_ENTRIES { self.entries.pop_front(); }
    }

    pub(super) fn snapshot(&self, limit: usize) -> Vec<TransportDiagnostic> {
        self.entries.iter().skip(self.entries.len().saturating_sub(limit.clamp(1, JOURNAL_ENTRIES))).cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diagnostics_survive_restart_and_rotate_with_safe_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transport.jsonl");
        let mut journal = TransportJournal::open(path.clone());
        for index in 0..2000 {
            journal.push(TransportDiagnostic { at_ms: index, trace_sequence: index,
                request_id_hashes: vec!["a".repeat(64)], error_kind: BackendFailureKind::ConnectionRefused,
                os_error_code: Some(61), elapsed_us: 2, generation: 3, recovery: RecoveryOutcome::Started });
        }
        assert_eq!(journal.snapshot(100).len(), 100);
        assert!(path.with_extension("jsonl.1").exists());
        for file in [&path, &path.with_extension("jsonl.1")] {
            assert!(fs::metadata(file).unwrap().len() <= JOURNAL_BYTES);
            let contents = fs::read_to_string(file).unwrap();
            for forbidden in ["Bearer", "arguments", "headers", "backend_url"] { assert!(!contents.contains(forbidden)); }
        }
        let restored = TransportJournal::open(path);
        assert_eq!(restored.snapshot(100), journal.snapshot(100));
    }
}
