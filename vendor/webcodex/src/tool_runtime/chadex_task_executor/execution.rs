//! Phase 12 execution receipt. This is an execution fence, not a resume engine.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ExecutionState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
    Unknown,
}

impl ExecutionState {
    pub(super) fn terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }

    pub(super) fn transition(&mut self, next: Self) -> Result<(), String> {
        if self.terminal()
            || !matches!(
                (*self, next),
                (Self::Queued, Self::Running)
                    | (
                        Self::Running,
                        Self::Succeeded
                            | Self::Failed
                            | Self::Cancelled
                            | Self::Interrupted
                            | Self::Unknown
                    )
            )
        {
            return Err("invalid_execution_transition".into());
        }
        *self = next;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct Execution {
    pub(super) execution_id: String,
    pub(super) previous_execution_id: Option<String>,
    pub(super) state: ExecutionState,
    // Conservative crash fence, committed before any workspace or tool mutation.
    pub(super) effects_possible: bool,
    pub(super) outcome_uncertain: bool,
    #[serde(skip)]
    pub(super) receipt_path: PathBuf,
    #[serde(skip)]
    _owner: Arc<fs::File>,
}

impl Execution {
    pub(super) fn claim(root: &Path, id: &str, previous: Option<String>) -> Result<Self, String> {
        if !valid_task_id(id) {
            return Err("invalid_execution_id".into());
        }
        let directory = root.join("executions");
        fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
        if fs::symlink_metadata(&directory)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("unsafe_execution_directory".into());
        }
        // An exclusive directory is also a permanent replay tombstone. Never GC
        // it with the bounded UI history, logs, or discarded recovery plans.
        let claim = directory.join(id);
        fs::create_dir(&claim).map_err(|e| format!("execution_claim_rejected: {e}"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&claim, fs::Permissions::from_mode(0o700))
                .map_err(|e| e.to_string())?;
        }
        fs::File::open(&directory)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        fs::File::open(root)
            .and_then(|f| f.sync_all())
            .map_err(|e| e.to_string())?;
        let owner = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(claim.join("owner.lock"))
            .map_err(|e| e.to_string())?;
        owner.try_lock().map_err(|e| e.to_string())?;
        let execution = Self {
            execution_id: id.into(),
            previous_execution_id: previous,
            state: ExecutionState::Queued,
            effects_possible: false,
            outcome_uncertain: false,
            receipt_path: claim.join("receipt.json"),
            _owner: Arc::new(owner),
        };
        execution.persist(Value::Null)?;
        Ok(execution)
    }

    pub(super) fn persist(&self, result: Value) -> Result<(), String> {
        let bytes = serde_json::to_vec(&json!({"execution": self, "result": result}))
            .map_err(|e| e.to_string())?;
        write_private_file(&self.receipt_path, &bytes).map_err(|e| e.to_string())
    }
}

/// Covers dropped HTTP futures, package join cancellation and panic unwinding.
/// It never assumes dropping a future stopped a Runner-side process.
pub(super) struct ExecutionGuard(pub(super) Arc<TaskControl>);
impl Drop for ExecutionGuard {
    fn drop(&mut self) {
        let _ = update_snapshot(&self.0, |snapshot| {
            snapshot.status = "interrupted".into();
            snapshot.finished_at_ms = Some(now_ms());
            snapshot.failure =
                Some(json!({"kind":"execution_future_dropped", "category":"interruption"}));
        });
    }
}

pub(super) fn uncertain_result(kind: &str, result: &ToolResult) -> bool {
    if !matches!(kind, "edit" | "run_process" | "validate") {
        return false;
    }
    if result_requires_observation(result) {
        return true;
    }
    let markers = result_markers(result);
    if markers.iter().any(|m| *m == "outcome_unknown") {
        return true;
    }
    !result.success
        && result.output.get("state_changed").and_then(Value::as_bool) != Some(false)
        && result
            .output
            .get("command_completed")
            .and_then(Value::as_bool)
            != Some(true)
        && result
            .output
            .get("exit_code")
            .and_then(Value::as_i64)
            .is_none()
}

/// Feed authoritative receipts into the existing startup recovery projection.
/// This does not resume anything. Terminal receipts are never rewritten.
pub(super) fn restore_receipts(root: &Path) -> HashSet<String> {
    let mut active = HashSet::new();
    let Ok(entries) = fs::read_dir(root.join("executions")) else {
        return active;
    };
    let state_dir = root.join("state");
    if fs::create_dir_all(&state_dir).is_err() {
        return active;
    }
    for entry in entries.flatten() {
        let id = entry.file_name().to_string_lossy().into_owned();
        if !valid_task_id(&id) || !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        if entry.path().join("discarded").exists() {
            continue;
        }
        let path = entry.path().join("receipt.json");
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_file() && !m.file_type().is_symlink()) {
            continue;
        }
        // A second runtime may inspect the same private directory. A live
        // owner must not be classified as crashed or have its receipt changed.
        let Ok(owner) = OpenOptions::new()
            .read(true)
            .write(true)
            .open(entry.path().join("owner.lock"))
        else {
            continue;
        };
        if owner.try_lock().is_err() {
            active.insert(id);
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(mut receipt) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        if receipt
            .pointer("/execution/execution_id")
            .and_then(Value::as_str)
            != Some(id.as_str())
        {
            continue;
        }
        let Some(state) = receipt
            .pointer("/execution/state")
            .cloned()
            .and_then(|v| serde_json::from_value::<ExecutionState>(v).ok())
        else {
            continue;
        };
        if !receipt["result"].is_object() {
            // A queued claim precedes the UI projection. If the process died
            // while waiting for admission, recover that projection without
            // ever treating the absence of a result as permission to replay.
            let projection = fs::read(state_dir.join(format!("{id}.json")))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
            let Some(projection) = projection
                .filter(|value| value.get("task_id").and_then(Value::as_str) == Some(id.as_str()))
            else {
                continue;
            };
            receipt["result"] = projection;
        }
        if !state.terminal() {
            let effects = receipt
                .pointer("/execution/effects_possible")
                .and_then(Value::as_bool)
                != Some(false);
            let state = if effects { "unknown" } else { "interrupted" };
            receipt["execution"]["state"] = json!(state);
            receipt["result"]["execution_state"] = json!(state);
            receipt["result"]["status"] = json!(state);
            receipt["result"]["finished_at_ms"] = json!(now_ms());
            receipt["result"]["failure"] =
                json!({"kind":"runtime_interrupted", "category":"interruption"});
            if receipt["result"]["workspace"].is_object() {
                receipt["result"]["workspace"]["state"] = json!("preserved");
            }
            if write_private_file(&path, &serde_json::to_vec(&receipt).unwrap_or_default()).is_err()
            {
                continue;
            }
        }
        let _ = write_private_file(
            &state_dir.join(format!("{id}.json")),
            &serde_json::to_vec(&receipt["result"]).unwrap_or_default(),
        );
    }
    active
}

#[cfg(test)]
#[path = "../tests/execution_semantics.rs"]
mod tests;

pub(super) fn uncertain_workspace_error(error: &str) -> bool {
    !matches!(
        error,
        "execution_workspace_source_changed"
            | "execution_workspace_source_overlap"
            | "execution_workspace_reconciliation_conflict"
            | "execution_workspace_invalid_reconciliation_input"
    )
}
