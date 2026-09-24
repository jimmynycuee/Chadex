use super::events::{channel as machine_event_channel, MachineEventEmitter, MachineEventInbox};
use super::{ProcessKind, ProcessPhase, ProcessSnapshot};
use crate::chadex_core::runtime_compat::activity::{sanitize_message, ActivityEventKind, ActivityLevel, ActivityLog};
use crate::chadex_core::runtime_compat::deadline::Deadline;
use crate::chadex_core::runtime_compat::error::{DesktopError, DesktopResult};
use crate::chadex_core::runtime_compat::platform;
use serde_json::Value;
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use chadex_runtime_process::{GracefulTermination, ManagedChild};

const LOG_LINES: usize = 80;
const LOG_LINE_BYTES: usize = 2048;
const MACHINE_LINE_BYTES: usize = 16 * 1024;
const GRACEFUL_STOP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const LOCAL_EOF_GRACE: std::time::Duration = std::time::Duration::from_millis(250);
const PROCESS_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(20);

struct OwnedGeneration {
    child: ManagedChild,
    phase: ProcessPhase,
    exit_code: Option<i32>,
    logs: Arc<Mutex<VecDeque<String>>>,
    stdout_drain: JoinHandle<()>,
    stderr_drain: JoinHandle<()>,
}

pub struct LifecycleRegistry {
    generations: HashMap<ProcessKind, OwnedGeneration>,
    activity: ActivityLog,
}

impl LifecycleRegistry {
    pub fn new(activity: ActivityLog) -> Self {
        Self {
            generations: HashMap::new(),
            activity,
        }
    }

    pub async fn spawn_owned(
        &mut self,
        kind: ProcessKind,
        mut command: Command,
        machine_stdout: bool,
    ) -> DesktopResult<Option<MachineEventInbox>> {
        self.refresh();

        if self.generations.get(&kind).is_some_and(|generation| {
            matches!(
                generation.phase,
                ProcessPhase::Starting | ProcessPhase::Running | ProcessPhase::Stopping
            )
        }) {
            return Err(DesktopError::new(
                "process_already_running",
                format!("Desktop already owns an active {kind:?} process"),
                "Stop the existing Desktop-owned process first.",
            ));
        }

        if self.generations.contains_key(&kind) {
            self.stop(kind).await;
        }

        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut child = ManagedChild::spawn_with_options(
            &mut command,
            platform::managed_spawn_options(kind.allows_child_breakaway()),
        )
        .map_err(|error| {
            DesktopError::new(
                "process_start_failed",
                format!("Could not start the {kind:?} process"),
                "Check the configured WebCodex binaries and retry.",
            )
            .with_details(serde_json::json!({ "io_kind": format!("{:?}", error.kind()) }))
        })?;

        let pid = child.id();
        let stdout = child.child_mut().stdout.take().ok_or_else(|| {
            DesktopError::new(
                "process_start_failed",
                "Could not capture process output",
                "Retry the operation.",
            )
        })?;
        let stderr = child.child_mut().stderr.take().ok_or_else(|| {
            DesktopError::new(
                "process_start_failed",
                "Could not capture process diagnostics",
                "Retry the operation.",
            )
        })?;

        let logs = Arc::new(Mutex::new(VecDeque::new()));
        let (machine_emitter, machine_inbox) = if machine_stdout {
            let (emitter, inbox) = machine_event_channel();
            (Some(emitter), Some(inbox))
        } else {
            (None, None)
        };

        let stdout_logs = Arc::clone(&logs);
        let stdout_drain = tokio::task::spawn_blocking(move || {
            drain_stream(stdout, stdout_logs, machine_emitter, machine_stdout)
        });
        let stderr_logs = Arc::clone(&logs);
        let stderr_drain =
            tokio::task::spawn_blocking(move || drain_stream(stderr, stderr_logs, None, false));

        self.activity.push(
            ActivityEventKind::ProcessStarted,
            kind.activity_source(),
            ActivityLevel::Info,
            format!("Desktop started the process (PID {pid})"),
        );

        self.generations.insert(
            kind,
            OwnedGeneration {
                child,
                phase: ProcessPhase::Starting,
                exit_code: None,
                logs,
                stdout_drain,
                stderr_drain,
            },
        );

        Ok(machine_inbox)
    }

    pub fn refresh(&mut self) {
        for (kind, generation) in &mut self.generations {
            if !matches!(
                generation.phase,
                ProcessPhase::Starting | ProcessPhase::Running
            ) {
                continue;
            }

            match generation.child.try_wait() {
                Ok(Some(status)) => {
                    generation.exit_code = status.code();
                    generation.phase = if status.success() {
                        ProcessPhase::Exited
                    } else {
                        ProcessPhase::Failed
                    };
                    self.activity.push(
                        ActivityEventKind::ProcessExited,
                        kind.activity_source(),
                        if status.success() {
                            ActivityLevel::Info
                        } else {
                            ActivityLevel::Error
                        },
                        format!("Desktop-owned process exited with status {status}"),
                    );
                }
                Ok(None) => generation.phase = ProcessPhase::Running,
                Err(_) => {
                    generation.phase = ProcessPhase::Failed;
                    self.activity.push(
                        ActivityEventKind::ProcessObservationFailed,
                        kind.activity_source(),
                        ActivityLevel::Error,
                        "Desktop could not observe the child process state",
                    );
                }
            }
        }
    }

    pub fn snapshot(&mut self, kind: ProcessKind) -> Option<ProcessSnapshot> {
        self.refresh();
        self.generations
            .get(&kind)
            .map(|generation| ProcessSnapshot {
                kind,
                phase: generation.phase,
                pid: Some(generation.child.id()),
                exit_code: generation.exit_code,
                owned_by_desktop: true,
            })
    }

    pub fn logs(&self, kind: ProcessKind) -> Vec<String> {
        self.generations
            .get(&kind)
            .map(|generation| {
                generation
                    .logs
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .iter()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    pub async fn stop(&mut self, kind: ProcessKind) {
        self.stop_until(kind, Deadline::after(GRACEFUL_STOP_TIMEOUT))
            .await;
    }

    pub async fn stop_until(&mut self, kind: ProcessKind, deadline: Deadline) {
        let Some(mut generation) = self.generations.remove(&kind) else {
            return;
        };

        drop(generation.child.child_mut().stdin.take());

        if matches!(
            generation.phase,
            ProcessPhase::Starting | ProcessPhase::Running
        ) {
            generation.phase = ProcessPhase::Stopping;
            self.activity.push(
                ActivityEventKind::ProcessStopping,
                kind.activity_source(),
                ActivityLevel::Info,
                "Stopping the Desktop-owned process",
            );

            let now = tokio::time::Instant::now();
            let eof_deadline = if kind.keeps_full_eof_grace() {
                deadline.instant()
            } else {
                std::cmp::min(deadline.instant(), now + LOCAL_EOF_GRACE)
            };

            let graceful = wait_for_tree_exit(&mut generation.child, eof_deadline).await;
            if !graceful && tokio::time::Instant::now() < deadline.instant() {
                if matches!(
                    generation.child.request_terminate_tree(),
                    Ok(GracefulTermination::Requested)
                ) {
                    let signal_deadline = std::cmp::min(
                        deadline.instant(),
                        tokio::time::Instant::now() + LOCAL_EOF_GRACE,
                    );
                    let _ = wait_for_tree_exit(&mut generation.child, signal_deadline).await;
                }
            }

            force_reclaim_if_needed(&mut generation.child, deadline.instant()).await;
        }

        force_reclaim_if_needed(&mut generation.child, deadline.instant()).await;
        finish_drain_task(generation.stdout_drain, deadline.instant()).await;
        finish_drain_task(generation.stderr_drain, deadline.instant()).await;

        self.activity.push(
            ActivityEventKind::ProcessStopped,
            kind.activity_source(),
            ActivityLevel::Info,
            "Desktop-owned process stopped",
        );
    }

    pub async fn stop_all(&mut self) {
        for kind in [
            ProcessKind::QuickShare,
            ProcessKind::RegularTunnel,
            ProcessKind::LocalRunner,
            ProcessKind::LocalServer,
        ] {
            self.stop(kind).await;
        }
    }
}

async fn force_reclaim_if_needed(child: &mut ManagedChild, deadline: tokio::time::Instant) {
    if !child.try_tree_exit().unwrap_or(false) {
        let _ = child.terminate_tree();
        let _ = wait_for_tree_exit(child, deadline).await;
    }
}

async fn wait_for_tree_exit(child: &mut ManagedChild, deadline: tokio::time::Instant) -> bool {
    loop {
        let _ = child.try_wait();
        if child.try_tree_exit().unwrap_or(false) {
            return true;
        }

        let now = tokio::time::Instant::now();
        if now >= deadline {
            return false;
        }
        tokio::time::sleep_until(std::cmp::min(deadline, now + PROCESS_POLL_INTERVAL)).await;
    }
}

async fn finish_drain_task(mut task: JoinHandle<()>, deadline: tokio::time::Instant) {
    if tokio::time::Instant::now() >= deadline
        || tokio::time::timeout_at(deadline, &mut task).await.is_err()
    {
        task.abort();
        let _ = task.await;
    }
}

fn drain_stream<R>(
    mut reader: R,
    logs: Arc<Mutex<VecDeque<String>>>,
    machine_emitter: Option<MachineEventEmitter>,
    machine_only: bool,
) where
    R: Read,
{
    let mut buffer = [0_u8; 4096];
    let mut line = Vec::with_capacity(4096);
    let line_limit = if machine_only {
        MACHINE_LINE_BYTES
    } else {
        LOG_LINE_BYTES
    };

    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        for byte in &buffer[..read] {
            if *byte == b'\n' {
                process_line(&line, &logs, machine_emitter.as_ref(), machine_only);
                line.clear();
            } else if line.len() < line_limit {
                line.push(*byte);
            }
        }
    }

    if !line.is_empty() {
        process_line(&line, &logs, machine_emitter.as_ref(), machine_only);
    }
    if let Some(emitter) = machine_emitter {
        emitter.close();
    }
}

fn process_line(
    line: &[u8],
    logs: &Arc<Mutex<VecDeque<String>>>,
    machine_emitter: Option<&MachineEventEmitter>,
    machine_only: bool,
) {
    let text = String::from_utf8_lossy(line).trim().to_string();
    if text.is_empty() {
        return;
    }

    if machine_only {
        if let (Some(emitter), Ok(value)) = (machine_emitter, serde_json::from_str::<Value>(&text))
        {
            emitter.send(value);
        }
        return;
    }

    let mut logs = logs.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    logs.push_back(sanitize_message(&text));
    while logs.len() > LOG_LINES {
        logs.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stopping_an_unowned_kind_is_a_noop() {
        let mut registry = LifecycleRegistry::new(ActivityLog::default());
        registry.stop(ProcessKind::LocalRunner).await;
        assert!(registry.snapshot(ProcessKind::LocalRunner).is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn local_parent_lease_closes_before_forced_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("local-eof");
        let marker_arg = marker.to_string_lossy().into_owned();
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "cat >/dev/null; printf eof > \"$1\"",
            "chadex-parent-eof",
            marker_arg.as_str(),
        ]);

        let mut registry = LifecycleRegistry::new(ActivityLog::default());
        registry
            .spawn_owned(ProcessKind::LocalServer, command, false)
            .await
            .unwrap();
        registry.stop(ProcessKind::LocalServer).await;

        assert!(marker.is_file());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn quick_share_keeps_eof_graceful_stop() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("quick-share-eof");
        let marker_arg = marker.to_string_lossy().into_owned();
        let mut command = Command::new("/bin/sh");
        command.args([
            "-c",
            "cat >/dev/null; printf eof > \"$1\"",
            "chadex-quick-share-eof",
            marker_arg.as_str(),
        ]);

        let mut registry = LifecycleRegistry::new(ActivityLog::default());
        registry
            .spawn_owned(ProcessKind::QuickShare, command, false)
            .await
            .unwrap();
        registry.stop(ProcessKind::QuickShare).await;

        assert!(marker.is_file());
    }

    #[cfg(windows)]
    #[tokio::test]
    #[ignore = "Windows real-process lane: waits on a real PowerShell stdin EOF"]
    async fn regular_tunnel_stop_closes_stdin_on_windows() {
        let temp = tempfile::tempdir().unwrap();
        let marker = temp.path().join("regular-tunnel-eof");
        let escaped_marker = marker.to_string_lossy().replace('\'', "''");
        let mut command = Command::new("powershell.exe");
        command
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(format!(
                "$marker = '{escaped_marker}'; Set-Content -LiteralPath $marker -Value 'ready'; $null = [Console]::In.ReadToEnd(); Set-Content -LiteralPath $marker -Value 'eof'"
            ));

        let mut registry = LifecycleRegistry::new(ActivityLog::default());
        registry
            .spawn_owned(ProcessKind::RegularTunnel, command, false)
            .await
            .unwrap();

        let ready_deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while !std::fs::read_to_string(&marker)
            .ok()
            .is_some_and(|value| value.trim() == "ready")
        {
            assert!(tokio::time::Instant::now() < ready_deadline);
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        registry.stop(ProcessKind::RegularTunnel).await;
        assert_eq!(std::fs::read_to_string(&marker).unwrap().trim(), "eof");
    }
}
