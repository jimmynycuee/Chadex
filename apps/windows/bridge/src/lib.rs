//! Non-UI JSONL transport for the existing Chadex helper. No runtime fork,
//! automatic restart, RPC whitelist, response cache, or credential/log storage.
mod error;
mod transport;

pub use error::{BridgeError, ErrorCode};

use chadex_runtime_process::{is_executable_file, ManagedChild, SpawnOptions};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};
use tokio::sync::{oneshot, Notify};

pub const PROTOCOL_VERSION: u64 = 1;
pub const MAX_PENDING_REQUESTS: usize = 64;
pub const MAX_REQUEST_FRAME_BYTES: usize = 256 * 1024;
pub const MAX_RESPONSE_FRAME_BYTES: usize = 1024 * 1024;
const SHUTDOWN_RPC_TIMEOUT: Duration = Duration::from_millis(1500);
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
const SHUTDOWN_REAP: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperState {
    Running,
    Stopping,
    Stopped,
    Failed,
}

/// Process/transport health only. Running does not imply runtime readiness,
/// tunnel readiness, or ChatGPT verification; obtain those via getStatus.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Health {
    pub state: HelperState,
    pub pid: Option<u32>,
    pub error: Option<BridgeError>,
}

struct State {
    health: Health,
    next_id: u64,
    pending: HashMap<String, oneshot::Sender<Result<Value, BridgeError>>>,
    writing: Option<(String, Instant)>,
    shutdown_deadline: Option<Instant>,
}

struct Shared {
    state: Mutex<State>,
    process: Mutex<Option<ManagedChild>>,
    exit: Notify,
}

impl Shared {
    fn resolve(&self, id: &str, response: Result<Value, BridgeError>) {
        if let Some(sender) = lock(&self.state).pending.remove(id) {
            let _ = sender.send(response);
        }
    }

    fn fail_pending(state: &mut State, error: &BridgeError) {
        for (_, sender) in state.pending.drain() {
            let _ = sender.send(Err(error.clone()));
        }
    }

    fn invalidate(&self, error: BridgeError) {
        {
            let mut state = lock(&self.state);
            if state.health.error.is_none() {
                state.health.error = Some(error.clone());
            }
            state.health.state = HelperState::Failed;
            Self::fail_pending(&mut state, &error);
        }
        // Only this bridge's ManagedChild tree is ever targeted. No PID
        // enumeration, process-name kill, taskkill, or shared-runtime cleanup.
        if let Some(process) = lock(&self.process).as_mut() {
            let _ = process.terminate_tree();
        }
    }
}

/// Owns one helper lifetime. Launch a new Bridge explicitly after failure;
/// this object never restarts/replays requests or retains backend snapshots.
pub struct Bridge {
    shared: Arc<Shared>,
    writes: mpsc::SyncSender<transport::WriteJob>,
    shutdown_lock: tokio::sync::Mutex<()>,
}

impl Bridge {
    /// Synchronous launch works without a Tokio runtime. Pipe I/O and process
    /// polling run on dedicated threads; requests/shutdown require Tokio time.
    /// Packaged runtime_dir must be <resources>/chadex-runtime, preserving
    /// the helper's release-build fail-closed bundled-runtime resolution.
    pub fn launch(
        helper_path: &Path,
        runtime_dir: &Path,
        data_dir: &Path,
    ) -> Result<Self, BridgeError> {
        if !is_executable_file(helper_path) {
            return Err(BridgeError::new(ErrorCode::HelperMissing));
        }
        let helper = helper_path
            .canonicalize()
            .map_err(|_| BridgeError::new(ErrorCode::HelperMissing))?;
        let runtime = runtime_dir
            .canonicalize()
            .ok()
            .filter(|directory| directory.is_dir())
            .ok_or_else(|| BridgeError::new(ErrorCode::RuntimeDirectoryMissing))?;
        if data_dir.as_os_str().is_empty() {
            return Err(BridgeError::new(ErrorCode::DataDirectoryUnavailable));
        }
        std::fs::create_dir_all(data_dir)
            .map_err(|_| BridgeError::new(ErrorCode::DataDirectoryUnavailable))?;
        let data = data_dir
            .canonicalize()
            .map_err(|_| BridgeError::new(ErrorCode::DataDirectoryUnavailable))?;
        let resources = if runtime
            .file_name()
            .is_some_and(|name| name == "chadex-runtime")
        {
            runtime
                .parent()
                .expect("canonical runtime has a parent")
                .to_path_buf()
        } else {
            // W2 debug isolation: do not inherit an installed app's resources.
            let resources = data.join("bridge-resources");
            std::fs::create_dir_all(&resources)
                .map_err(|_| BridgeError::new(ErrorCode::DataDirectoryUnavailable))?;
            resources
        };
        let mut command = Command::new(helper);
        command
            .current_dir(&data)
            .env_remove("WEBCODEX_DESKTOP_BIN_DIR")
            .env("CHADEX_DATA_DIR", &data)
            .env(
                "CHADEX_GLOBAL_INSTRUCTIONS_PATH",
                data.join("global-instructions.md"),
            )
            .env("CHADEX_RESOURCE_DIR", &resources)
            .env("CHADEX_RUNTIME_BIN_DIR", &runtime)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // Never retain, print, or accumulate helper stderr, including
            // malformed UTF-8 or arbitrarily large credential-bearing logs.
            .stderr(Stdio::null());
        let mut process = ManagedChild::spawn_with_options(
            &mut command,
            SpawnOptions {
                windows_creation_flags: 0x0800_0000, // CREATE_NO_WINDOW, as W2
                // No breakaway: kernel ownership must cover all descendants
                // if the desktop is killed. W2 inner jobs may nest normally.
                windows_silent_child_breakaway: false,
            },
        )
        .map_err(|_| BridgeError::new(ErrorCode::SpawnFailed))?;
        let stdin = process
            .child_mut()
            .stdin
            .take()
            .ok_or_else(|| BridgeError::new(ErrorCode::TransportStartFailed))?;
        let stdout = process
            .child_mut()
            .stdout
            .take()
            .ok_or_else(|| BridgeError::new(ErrorCode::TransportStartFailed))?;
        let pid = process.id();
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                health: Health {
                    state: HelperState::Running,
                    pid: Some(pid),
                    error: None,
                },
                next_id: 0,
                pending: HashMap::new(),
                writing: None,
                shutdown_deadline: None,
            }),
            process: Mutex::new(Some(process)),
            exit: Notify::new(),
        });
        let (writes, receiver) = mpsc::sync_channel(MAX_PENDING_REQUESTS);
        if transport::start(&shared, stdin, stdout, receiver).is_err() {
            shared.invalidate(BridgeError::new(ErrorCode::TransportStartFailed));
            // A failed monitor-thread spawn must not retain the process/job.
            lock(&shared.process).take();
            return Err(BridgeError::new(ErrorCode::TransportStartFailed));
        }
        Ok(Self {
            shared,
            writes,
            shutdown_lock: tokio::sync::Mutex::new(()),
        })
    }

    pub fn health(&self) -> Health {
        lock(&self.shared.state).health.clone()
    }

    /// Passes existing RPC names through; Tauri owns the frontend whitelist.
    /// Timeout includes queueing/writing. No implicit retries after timeout.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value, BridgeError> {
        let deadline = Instant::now() + request_timeout(method);
        let (sender, receiver) = oneshot::channel();
        let id = {
            let mut state = lock(&self.shared.state);
            if state.health.state != HelperState::Running {
                return Err(state
                    .health
                    .error
                    .clone()
                    .unwrap_or_else(|| BridgeError::new(ErrorCode::NotRunning)));
            }
            if state.pending.len() >= MAX_PENDING_REQUESTS && method != "shutdown" {
                return Err(BridgeError::new(ErrorCode::TooManyPendingRequests));
            }
            if method == "shutdown" {
                state.health.state = HelperState::Stopping;
                state.shutdown_deadline = Some(Instant::now() + SHUTDOWN_GRACE);
                Shared::fail_pending(&mut state, &BridgeError::new(ErrorCode::Disconnected));
            }
            state.next_id = state
                .next_id
                .checked_add(1)
                .ok_or_else(|| BridgeError::new(ErrorCode::TooManyPendingRequests))?;
            let id = format!("desktop-{}", state.next_id);
            state.pending.insert(id.clone(), sender);
            id
        };
        // Dropping/cancelling the request future always frees its pending slot.
        let _pending = PendingGuard {
            shared: Arc::clone(&self.shared),
            id: id.clone(),
        };
        let frame = transport::encode_request(&id, method, params)?;
        self.writes
            .try_send(transport::WriteJob::Frame {
                id: id.clone(),
                frame,
                deadline,
            })
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => BridgeError::new(ErrorCode::TooManyPendingRequests),
                mpsc::TrySendError::Disconnected(_) => BridgeError::new(ErrorCode::WriteFailed),
            })?;
        match tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), receiver)
            .await
        {
            Ok(Ok(response)) => response,
            Ok(Err(_)) => Err(BridgeError::new(ErrorCode::Disconnected)),
            Err(_) => {
                let error = BridgeError::new(ErrorCode::RequestTimeout);
                let writing = lock(&self.shared.state)
                    .writing
                    .as_ref()
                    .is_some_and(|(writing_id, _)| writing_id == &id);
                if writing {
                    // An interrupted/blocked frame cannot safely be reused.
                    self.shared.invalidate(error.clone());
                }
                Err(error)
            }
        }
    }

    /// Send shutdown RPC (1.5s), close stdin, wait up to 5s total for graceful
    /// helper/tree exit, then kill only the owned tree and observe/reap exit.
    /// The monitor owns the deadline even if this future is cancelled.
    /// ShutdownForced means cleanup succeeded but the graceful gate failed.
    pub async fn shutdown(&self) -> Result<(), BridgeError> {
        let _shutdown = self.shutdown_lock.lock().await;
        let rpc_error = if self.health().state == HelperState::Running {
            self.request("shutdown", serde_json::json!({})).await.err()
        } else {
            None
        };
        let _ = self.writes.try_send(transport::WriteJob::Close);
        let deadline = lock(&self.shared.state)
            .shutdown_deadline
            .unwrap_or_else(Instant::now)
            + SHUTDOWN_REAP;
        loop {
            // Register before checking the predicate, avoiding lost wakeups.
            let notified = self.shared.exit.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let health = self.health();
            if health.pid.is_none() {
                return health.error.or(rpc_error).map_or(Ok(()), Err);
            }
            if tokio::time::timeout(deadline.saturating_duration_since(Instant::now()), notified)
                .await
                .is_err()
            {
                let error = BridgeError::new(ErrorCode::ShutdownFailed);
                self.shared.invalidate(error.clone());
                return Err(error);
            }
        }
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        // Bridge owner drop is an explicit fallback. On Windows abrupt desktop
        // death also closes the non-inherited Job Object handle in the kernel.
        if lock(&self.shared.process).is_some() {
            self.shared
                .invalidate(BridgeError::new(ErrorCode::Disconnected));
        }
    }
}

struct PendingGuard {
    shared: Arc<Shared>,
    id: String,
}
impl Drop for PendingGuard {
    fn drop(&mut self) {
        lock(&self.shared.state).pending.remove(&self.id);
    }
}

pub fn request_timeout(method: &str) -> Duration {
    match method {
        "connectChatGPT" | "startTunnel" | "configureLocalSetup" | "resumeService" => {
            Duration::from_secs(120)
        }
        "switchLocalProject" | "activateProject" | "stopLocalService" | "disconnectAI"
        | "stopTunnel" => Duration::from_secs(30),
        "shutdown" => SHUTDOWN_RPC_TIMEOUT,
        _ => Duration::from_secs(10),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
