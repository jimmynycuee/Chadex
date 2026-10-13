use super::computer_safety::{ComputerAuthorization, ComputerSafetyController};
use super::performance::{duration_us, now_ms, McpPerformanceTrace, PerformanceTraceStore, BackendFailureKind, TransportDiagnostic, RecoveryOutcome};
use super::{ChadexError, ChadexResult};
use axum::body::{to_bytes, Body, Bytes};
use axum::extract::State;
use axum::http::{header, HeaderMap, Method, Request, Response, StatusCode};
use axum::routing::any;
use axum::Router;
use futures_core::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex as StdMutex, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, OwnedSemaphorePermit, Semaphore};

mod response_evidence;
use response_evidence::ResponseEvidence;
use tokio::task::JoinHandle;

const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
const MAX_IN_FLIGHT: usize = 32;
const REQUEST_BODY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const HELPER_INGRESS_STARTED_UNIX_NS_HEADER: &str = "x-chadex-helper-ingress-started-unix-ns";
const HELPER_INGRESS_PRE_BACKEND_US_HEADER: &str = "x-chadex-helper-ingress-pre-backend-us";
const BACKEND_RECOVERY_WAIT: Duration = Duration::from_secs(45);
const BACKEND_RECOVERY_COOLDOWN: Duration = Duration::from_secs(3);
const BACKEND_RESTARTING_RETRY_AFTER_SECONDS: u64 = 5;
const MAX_BACKEND_RECOVERIES: u8 = 3;

/// Recovery only restores the configured runtime; it never replays a request.
pub trait BackendRecovery: Send + Sync + 'static {
    fn recover(&self) -> Pin<Box<dyn Future<Output = bool> + Send + '_>>;
    fn generation(&self) -> u64 { 0 }
    fn backend_succeeded(&self) {}
}

struct BackendRecoveryGate {
    hook: Arc<dyn BackendRecovery>,
    state: StdMutex<BackendRecoveryState>,
}

#[derive(Default)]
struct BackendRecoveryState {
    in_flight: bool,
    last: Option<Instant>,
    attempts: u8,
}

impl BackendRecoveryGate {
    fn new(hook: Arc<dyn BackendRecovery>) -> Arc<Self> {
        Arc::new(Self { hook, state: StdMutex::default() })
    }

    fn backend_succeeded(&self) {
        self.state.lock().unwrap_or_else(|p| p.into_inner()).attempts = 0;
        self.hook.backend_succeeded();
    }

    /// Returns immediately, releasing ingress admission before recovery can drain it.
    fn trigger(self: &Arc<Self>, store: Arc<PerformanceTraceStore>, diagnostic: TransportDiagnostic) -> RecoveryOutcome {
        self.trigger_with_deadline(store, diagnostic, BACKEND_RECOVERY_WAIT)
    }

    fn trigger_with_deadline(self: &Arc<Self>, store: Arc<PerformanceTraceStore>, mut diagnostic: TransportDiagnostic, deadline: Duration) -> RecoveryOutcome {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.in_flight { return RecoveryOutcome::InFlight; }
        if state.last.is_some_and(|last| last.elapsed() < BACKEND_RECOVERY_COOLDOWN) { return RecoveryOutcome::Cooldown; }
        if state.attempts >= MAX_BACKEND_RECOVERIES { return RecoveryOutcome::Exhausted; }
        state.attempts += 1;
        state.in_flight = true;
        let gate = Arc::clone(self);
        tokio::spawn(async move {
            let started = Instant::now();
            let hook = Arc::clone(&gate.hook);
            let mut task = tokio::spawn(async move { hook.recover().await });
            let outcome = match tokio::time::timeout(deadline, &mut task).await {
                Ok(Ok(true)) => RecoveryOutcome::Ready,
                Ok(_) => RecoveryOutcome::Failed,
                Err(_) => {
                    task.abort();
                    let _ = task.await;
                    RecoveryOutcome::TimedOut
                }
            };
            {
                let mut state = gate.state.lock().unwrap_or_else(|p| p.into_inner());
                state.in_flight = false;
                state.last = Some(Instant::now());
            }
            diagnostic.at_ms = now_ms();
            diagnostic.elapsed_us = duration_us(started.elapsed());
            diagnostic.recovery = outcome;
            store.record_transport(diagnostic);
        });
        RecoveryOutcome::Started
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VerificationSnapshot {
    pub epoch: u64,
    pub project_path: Option<String>,
    pub chatgpt_connected: bool,
    pub verified: bool,
    pub last_seen_at_ms: Option<u64>,
    pub verified_at_ms: Option<u64>,
}

pub struct VerificationTracker {
    next_epoch: AtomicU64,
    state: RwLock<VerificationSnapshot>,
}

impl Default for VerificationTracker {
    fn default() -> Self {
        Self {
            next_epoch: AtomicU64::new(1),
            state: RwLock::new(VerificationSnapshot::default()),
        }
    }
}

impl VerificationTracker {
    pub fn snapshot(&self) -> VerificationSnapshot {
        self.state
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub fn reset(&self, project_path: Option<String>) -> VerificationSnapshot {
        let snapshot = VerificationSnapshot {
            epoch: self.next_epoch.fetch_add(1, Ordering::SeqCst),
            project_path,
            chatgpt_connected: false,
            verified: false,
            last_seen_at_ms: None,
            verified_at_ms: None,
        };
        *self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = snapshot.clone();
        snapshot
    }

    fn observe_request(&self, epoch: u64, methods: &[String]) {
        if methods.is_empty() {
            return;
        }
        let meaningful = methods.iter().any(|method| {
            matches!(method.as_str(), "initialize" | "tools/list" | "tools/call")
                || method.starts_with("notifications/")
        });
        if !meaningful {
            return;
        }
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.epoch != epoch {
            return;
        }
        state.chatgpt_connected = true;
        state.last_seen_at_ms = Some(now_ms());
    }

    fn verify_project_read(&self, epoch: u64, project_path: &str) {
        let mut state = self
            .state
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.epoch != epoch {
            return;
        }
        let Some(selected_path) = state.project_path.as_deref() else {
            return;
        };
        if !chadex_runtime_runner_config::paths::paths_equal(
            std::path::Path::new(selected_path),
            std::path::Path::new(project_path),
        ) {
            return;
        }
        let now = now_ms();
        state.chatgpt_connected = true;
        state.verified = true;
        state.last_seen_at_ms = Some(now);
        state.verified_at_ms = Some(now);
    }
}

#[derive(Clone)]
struct IngressState {
    backend_url: String,
    recovery: Option<Arc<BackendRecoveryGate>>,
    client: Client,
    admission: Arc<Semaphore>,
    tracker: Arc<VerificationTracker>,
    performance: Arc<PerformanceTraceStore>,
    computer_safety: Arc<ComputerSafetyController>,
    armed: Arc<AtomicBool>,
    accepting: Arc<AtomicBool>,
}

pub struct McpIngress {
    mcp_url: String,
    armed: Arc<AtomicBool>,
    accepting: Arc<AtomicBool>,
    admission: Arc<Semaphore>,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl McpIngress {
    pub async fn start(
        backend_url: String,
        tracker: Arc<VerificationTracker>,
        performance: Arc<PerformanceTraceStore>,
    ) -> ChadexResult<Self> {
        Self::start_with_safety(
            backend_url,
            tracker,
            performance,
            Arc::new(ComputerSafetyController::default()),
        )
        .await
    }

    pub async fn start_with_safety(
        backend_url: String,
        tracker: Arc<VerificationTracker>,
        performance: Arc<PerformanceTraceStore>,
        computer_safety: Arc<ComputerSafetyController>,
    ) -> ChadexResult<Self> {
        Self::start_with_safety_and_recovery(backend_url, tracker, performance, computer_safety, None).await
    }

    pub async fn start_with_safety_and_recovery(
        backend_url: String,
        tracker: Arc<VerificationTracker>,
        performance: Arc<PerformanceTraceStore>,
        computer_safety: Arc<ComputerSafetyController>,
        recovery: Option<Arc<dyn BackendRecovery>>,
    ) -> ChadexResult<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await.map_err(|_| {
            ChadexError::new(
                "ingress_unavailable",
                "Chadex could not bind its local MCP ingress",
                "Check local network permissions and retry.",
            )
        })?;
        let address = listener.local_addr().map_err(|_| {
            ChadexError::new(
                "ingress_unavailable",
                "Chadex could not determine its local MCP ingress address",
                "Retry the connection.",
            )
        })?;
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(5))
            .retry(reqwest::retry::never())
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| proxy_error())?;
        let armed = Arc::new(AtomicBool::new(false));
        let accepting = Arc::new(AtomicBool::new(true));
        let admission = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
        let state = IngressState {
            backend_url,
            recovery: recovery.map(BackendRecoveryGate::new),
            client,
            admission: Arc::clone(&admission),
            tracker,
            performance,
            computer_safety: Arc::clone(&computer_safety),
            armed: Arc::clone(&armed),
            accepting: Arc::clone(&accepting),
        };
        let app = Router::new()
            .route("/mcp", any(proxy))
            .route("/mcp/", any(proxy))
            .with_state(state);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let server = axum::serve(listener, app).with_graceful_shutdown(async move {
                let _ = shutdown_rx.await;
            });
            let _ = server.await;
        });
        Ok(Self {
            mcp_url: format!("http://{address}/mcp"),
            armed,
            accepting,
            admission,
            shutdown: Some(shutdown_tx),
            task,
        })
    }

    pub fn mcp_url(&self) -> &str {
        &self.mcp_url
    }

    pub fn arm_verification(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    pub(crate) async fn hold_admission_for_test(&self) -> OwnedSemaphorePermit {
        Arc::clone(&self.admission)
            .acquire_owned()
            .await
            .expect("test ingress semaphore remains open")
    }

    pub async fn pause_and_drain(&self) {
        self.accepting.store(false, Ordering::SeqCst);
        // Wait until every request that entered before the pause has released
        // its admission permit. Requests that observed the old accepting=true
        // but lose this race fail closed at admission instead of reaching the
        // backend during project activation.
        if let Ok(permits) = Arc::clone(&self.admission)
            .acquire_many_owned(MAX_IN_FLIGHT as u32)
            .await
        {
            drop(permits);
        }
    }

    pub fn resume(&self) {
        self.accepting.store(true, Ordering::SeqCst);
    }
}

impl Drop for McpIngress {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.task.abort();
    }
}

async fn proxy(State(state): State<IngressState>, request: Request<Body>) -> Response<Body> {
    match proxy_inner(state, request).await {
        Ok(response) => response,
        Err(_) => Response::builder()
            .status(StatusCode::BAD_GATEWAY)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(
                r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"Chadex local MCP backend is unavailable"},"id":null}"#,
            ))
            .unwrap_or_else(|_| Response::new(Body::empty())),
    }
}

async fn proxy_inner(state: IngressState, request: Request<Body>) -> ChadexResult<Response<Body>> {
    let started = Instant::now();
    let helper_ingress_started_unix_ns = unix_time_ns();
    let started_at_ms = now_ms();
    if !state.accepting.load(Ordering::SeqCst) {
        return Ok(ingress_rejection(StatusCode::SERVICE_UNAVAILABLE));
    }
    // Fence each request to the project epoch that was current when the request
    // entered the ingress. If a project switch happens while the request is in
    // flight, its eventual response cannot verify the newly selected project.
    let verification_epoch = state.tracker.snapshot().epoch;
    // A request arriving before readiness cannot later become connection evidence.
    let armed = state.armed.load(Ordering::SeqCst);
    let permit = match Arc::clone(&state.admission).try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return Ok(ingress_rejection(StatusCode::TOO_MANY_REQUESTS)),
    };
    let (parts, body) = request.into_parts();
    let body =
        match tokio::time::timeout(REQUEST_BODY_TIMEOUT, to_bytes(body, MAX_REQUEST_BYTES)).await {
            Ok(Ok(body)) => body,
            result => {
                let status = if result.is_err() {
                    StatusCode::REQUEST_TIMEOUT
                } else {
                    StatusCode::PAYLOAD_TOO_LARGE
                };
                state.performance.push(McpPerformanceTrace {
                    sequence: 0,
                    started_at_ms,
                    finished_at_ms: Some(now_ms()),
                    request_id_hashes: Vec::new(),
                    server_trace_id: None,
                    methods: Vec::new(),
                    tool_names: Vec::new(),
                    tool_failed: None,
                    request_bytes: 0,
                    response_bytes: 0,
                    status_code: None,
                    ingress_pre_backend_us: duration_us(started.elapsed()),
                    backend_headers_us: 0,
                    response_stream_us: 0,
                    total_us: duration_us(started.elapsed()),
                    completion: "request_body_error".to_string(),
                });
                return Ok(ingress_rejection(status));
            }
        };
    // The safety gates below inspect the parsed body. A body this parser rejects
    // could still be accepted by the runtime's laxer struct parsing (e.g. deep
    // nesting or out-of-range numbers in unknown fields), so never forward it.
    if parts.method == Method::POST
        && !body.is_empty()
        && serde_json::from_slice::<Value>(&body).is_err()
    {
        return Ok(ingress_rejection(StatusCode::BAD_REQUEST));
    }
    let metadata = mcp_metadata(&body);

    let computer_dispatch_permit = match inspect_computer_control(&body) {
        ComputerControlInspection::None => None,
        ComputerControlInspection::BatchRejected => {
            state.computer_safety.record_batch_rejection();
            return Ok(computer_safety_response(
                Value::Null,
                "computer_control_batch_rejected",
                "Computer control cannot be authorized inside a JSON-RPC batch.",
                false,
            ));
        }
        ComputerControlInspection::Single { id, action } => {
            match state.computer_safety.authorize(&action).await {
                ComputerAuthorization::Allowed(permit) => Some(permit),
                ComputerAuthorization::Denied {
                    error_kind,
                    message,
                } => {
                    return Ok(computer_safety_response(id, error_kind, message, false));
                }
            }
        }
    };

    if let Some(id) = tunnel_blocked_management_call(&body) {
        return Ok(computer_safety_response(
            id,
            "management_tool_not_available_over_tunnel",
            "Installing or changing managed Skills and purging memory scopes is only available from the Chadex desktop app.",
            false,
        ));
    }

    let mut url = state.backend_url.clone();
    if let Some(query) = parts.uri.query() {
        url.push('?');
        url.push_str(query);
    }
    let mut builder = state.client.request(parts.method.clone(), url);
    builder = copy_request_headers(builder, &parts.headers);
    if parts.method != Method::GET && parts.method != Method::HEAD {
        builder = builder.body(body.clone());
    }
    let ingress_pre_backend_us = duration_us(started.elapsed());
    builder = add_helper_ingress_timing_headers(
        builder,
        helper_ingress_started_unix_ns,
        ingress_pre_backend_us,
    );

    if let Some(permit) = computer_dispatch_permit.as_ref() {
        if !state.computer_safety.permit_is_current(permit) {
            return Ok(computer_safety_response(
                computer_control_request_id(&body),
                "computer_control_stopped",
                "Computer control was stopped before the action could be dispatched.",
                false,
            ));
        }
    }
    let mut pending = state.performance.begin_request(McpPerformanceTrace {
        sequence: 0,
        started_at_ms,
        finished_at_ms: None,
        request_id_hashes: metadata.request_id_hashes.clone(),
        server_trace_id: None,
        methods: metadata.methods.clone(),
        tool_names: metadata.tool_names.clone(),
        tool_failed: None,
        request_bytes: body.len().try_into().unwrap_or(u64::MAX),
        response_bytes: 0,
        status_code: None,
        ingress_pre_backend_us,
        backend_headers_us: 0,
        response_stream_us: 0,
        total_us: 0,
        completion: "running".to_string(),
    });
    let trace_sequence = pending.sequence();
    let backend_started = Instant::now();
    let upstream_result = if let Some(permit) = computer_dispatch_permit.as_ref() {
        let send = builder.send();
        tokio::pin!(send);
        tokio::select! {
            result = &mut send => result,
            _ = state.computer_safety.wait_until_invalid(permit) => {
                state.computer_safety.record_interrupted(permit);
                pending.complete("computer_control_stopped_during_dispatch");
                return Ok(computer_safety_response(
                    computer_control_request_id(&body),
                    "computer_control_stopped_during_dispatch",
                    "Computer control was stopped while dispatch was in progress. The effect may have occurred; re-observe the affected state before retrying.",
                    true,
                ));
            }
        }
    } else {
        builder.send().await
    };
    let upstream = match upstream_result {
        Ok(upstream) => upstream,
        Err(error) => {
            let (kind, os_code) = classify_backend_error(&error);
            let mut diagnostic = TransportDiagnostic {
                at_ms: now_ms(), trace_sequence, request_id_hashes: metadata.request_id_hashes.clone(),
                error_kind: kind, os_error_code: os_code, elapsed_us: duration_us(backend_started.elapsed()),
                generation: state.recovery.as_ref().map_or(0, |gate| gate.hook.generation()),
                recovery: RecoveryOutcome::Unavailable,
            };
            if kind.not_started() {
                if let Some(gate) = &state.recovery {
                    diagnostic.recovery = gate.trigger(Arc::clone(&state.performance), diagnostic.clone());
                }
            }
            let retryable = matches!(diagnostic.recovery, RecoveryOutcome::Started | RecoveryOutcome::InFlight | RecoveryOutcome::Cooldown);
            state.performance.record_transport(diagnostic);
            state.performance.push(McpPerformanceTrace {
                sequence: trace_sequence,
                started_at_ms,
                finished_at_ms: Some(now_ms()),
                request_id_hashes: metadata.request_id_hashes,
                server_trace_id: None,
                methods: metadata.methods,
                tool_names: metadata.tool_names,
                tool_failed: None,
                request_bytes: body.len().try_into().unwrap_or(u64::MAX),
                response_bytes: 0,
                status_code: None,
                ingress_pre_backend_us,
                backend_headers_us: duration_us(backend_started.elapsed()),
                response_stream_us: 0,
                total_us: duration_us(started.elapsed()),
                completion: kind.as_str().to_string(),
            });
            pending.hand_off();
            return Ok(backend_failure_response(&body, kind, retryable));
        }
    };
    if let Some(permit) = computer_dispatch_permit.as_ref() {
        state.computer_safety.record_dispatch(permit);
    }
    let backend_headers_us = duration_us(backend_started.elapsed());
    let status = upstream.status();
    let headers = upstream.headers().clone();
    let evidence = if status.is_success() {
        ResponseEvidence::new(
            &body,
            &headers,
            Arc::clone(&state.tracker),
            verification_epoch,
        ).map(|mut evidence| { evidence.set_verification_enabled(armed); evidence })
    } else {
        None
    };
    pending.hand_off();
    let stream = TracedResponseStream::new(
        BackendResponseStream::new(upstream.bytes_stream(), headers.get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("application/json"))),
        Arc::clone(&state.performance),
        McpPerformanceTrace {
            sequence: trace_sequence,
            started_at_ms,
            finished_at_ms: None,
            request_id_hashes: metadata.request_id_hashes,
            server_trace_id: server_trace_id(&headers),
            methods: metadata.methods,
            tool_names: metadata.tool_names,
            tool_failed: None,
            request_bytes: body.len().try_into().unwrap_or(u64::MAX),
            response_bytes: 0,
            status_code: Some(status.as_u16()),
            ingress_pre_backend_us,
            backend_headers_us,
            response_stream_us: 0,
            total_us: 0,
            completion: String::new(),
        },
        started,
        evidence,
        permit,
        StreamFailureContext {
            request: request_identity(&body),
            sse: headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(|v| v.starts_with("text/event-stream")),
            recovery: state.recovery.clone(),
        },
    );
    let mut response = Response::builder().status(status);
    for (name, value) in headers.iter() {
        if is_hop_by_hop(name.as_str())
            || connection_header_names(&headers, name.as_str())
            || name == header::CONTENT_LENGTH
        {
            continue;
        }
        response = response.header(name, value);
    }
    response
        .body(Body::from_stream(stream))
        .map_err(|_| proxy_error())
}

fn ingress_rejection(status: StatusCode) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(format!(
            r#"{{"jsonrpc":"2.0","error":{{"code":-32000,"message":"{}"}},"id":null}}"#,
            status.canonical_reason().unwrap_or("Request rejected")
        )))
        .expect("static rejection response")
}

fn io_error_code(error: &(dyn std::error::Error + 'static)) -> (Option<std::io::ErrorKind>, Option<i32>) {
    let mut source = Some(error);
    let mut kind = None;
    while let Some(error) = source {
        if let Some(io) = error.downcast_ref::<std::io::Error>() {
            kind = Some(io.kind());
            if io.raw_os_error().is_some() { return (kind, io.raw_os_error()); }
        }
        source = error.source();
    }
    (kind, None)
}

fn classify_backend_error(error: &reqwest::Error) -> (BackendFailureKind, Option<i32>) {
    let (io_kind, code) = io_error_code(error);
    let kind = if error.is_connect() && io_kind == Some(std::io::ErrorKind::ConnectionRefused) {
        BackendFailureKind::ConnectionRefused
    } else if error.is_connect() && error.is_timeout() {
        BackendFailureKind::ConnectTimeout
    } else {
        BackendFailureKind::HeadersInterrupted
    };
    (kind, code)
}

/// Keep only the in-memory identity necessary to answer; never retain arguments.
fn request_identity(body: &[u8]) -> Value {
    fn one(value: &Value) -> Value {
        json!({"id": value.get("id").cloned().unwrap_or(Value::Null), "method": value.get("method")})
    }
    let value = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
    match value {
        Value::Array(items) => Value::Array(items.iter().filter(|v| v.get("id").is_some()).map(one).collect()),
        value => one(&value),
    }
}

fn backend_failure_value(identity: &Value, kind: BackendFailureKind, retryable: bool) -> Value {
    if let Value::Array(items) = identity {
        return Value::Array(items.iter().map(|id| backend_failure_value(id, kind, retryable)).collect());
    }
    let message = if kind.not_started() {
        "The local runtime refused the connection before dispatch. Nothing was executed. Recovery may be in progress; retry only a safe observation when retry_after_secs is present."
    } else {
        "The local runtime connection was interrupted. The operation may have executed. Reconcile the affected state before retrying; this request was not replayed."
    };
    let mut output = json!({
        "error_kind": kind.as_str(), "failure_kind": kind.as_str(), "message": message,
        "execution_state": if kind.not_started() { "not_started" } else { "outcome_unknown" },
        "dispatch_certainty": if kind.not_started() { "not_started" } else { "unknown" },
        "recovery_kind": if retryable { "retry" } else if kind.not_started() { "user_action" } else { "reobserve" },
    });
    if retryable { output["retry_after_secs"] = json!(BACKEND_RESTARTING_RETRY_AFTER_SECONDS); }
    let id = identity.get("id").cloned().unwrap_or(Value::Null);
    if identity.get("method").and_then(Value::as_str) == Some("tools/call") {
        json!({"jsonrpc":"2.0", "id": id, "result": {
            "content": [{"type":"text", "text":message}],
            "structuredContent": {"success":false, "output":output}, "isError":true
        }})
    } else {
        json!({"jsonrpc":"2.0", "id": id, "error": {"code":-32603, "message":message, "data":output}})
    }
}

fn backend_failure_response(body: &[u8], kind: BackendFailureKind, retryable: bool) -> Response<Body> {
    Response::builder().status(StatusCode::OK).header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(backend_failure_value(&request_identity(body), kind, retryable).to_string()))
        .expect("static backend failure envelope")
}

struct TracedResponseStream<S> {
    inner: Pin<Box<S>>,
    store: Arc<PerformanceTraceStore>,
    trace: Option<McpPerformanceTrace>,
    started: Instant,
    headers_at: Instant,
    evidence: Option<ResponseEvidence>,
    permit: Option<OwnedSemaphorePermit>,
    failure: StreamFailureContext,
    emitted: bool,
    failed: bool,
}

impl<S> Unpin for TracedResponseStream<S> {}

impl<S> TracedResponseStream<S> {
    fn new(
        stream: S,
        store: Arc<PerformanceTraceStore>,
        trace: McpPerformanceTrace,
        started: Instant,
        evidence: Option<ResponseEvidence>,
        permit: OwnedSemaphorePermit,
        failure: StreamFailureContext,
    ) -> Self {
        Self {
            inner: Box::pin(stream),
            store,
            trace: Some(trace),
            started,
            headers_at: Instant::now(),
            evidence,
            permit: Some(permit),
            failure,
            emitted: false,
            failed: false,
        }
    }

    fn finish(&mut self, completion: &str) {
        self.permit.take();
        let Some(mut trace) = self.trace.take() else {
            return;
        };
        trace.tool_failed = self.evidence.as_ref().and_then(ResponseEvidence::tool_failed);
        trace.response_stream_us = duration_us(self.headers_at.elapsed());
        trace.total_us = duration_us(self.started.elapsed());
        trace.finished_at_ms = Some(now_ms());
        trace.completion = completion.to_string();
        self.store.push(trace);
    }
}

struct StreamFailureContext {
    request: Value,
    sse: bool,
    recovery: Option<Arc<BackendRecoveryGate>>,
}

/// JSON must remain atomic: a truncated success document cannot be repaired
/// after bytes have been sent. SSE stays streaming for long polling.
struct BackendResponseStream<S> {
    inner: Pin<Box<S>>,
    json: bool,
    buffer: Vec<u8>,
    done: bool,
}
impl<S> Unpin for BackendResponseStream<S> {}
impl<S> BackendResponseStream<S> {
    fn new(inner: S, json: bool) -> Self { Self { inner: Box::pin(inner), json, buffer: Vec::new(), done: false } }
}
impl<S> Stream for BackendResponseStream<S>
where S: Stream<Item = Result<Bytes, reqwest::Error>> {
    type Item = Result<Bytes, std::io::Error>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.done { return Poll::Ready(None); }
        loop {
            match this.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(chunk))) if this.json => {
                    if this.buffer.len().saturating_add(chunk.len()) > MAX_REQUEST_BYTES {
                        this.done = true;
                        this.buffer.clear();
                        return Poll::Ready(Some(Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "backend_response_too_large"))));
                    }
                    this.buffer.extend_from_slice(&chunk);
                }
                Poll::Ready(Some(Ok(chunk))) => return Poll::Ready(Some(Ok(chunk))),
                Poll::Ready(Some(Err(error))) => {
                    this.done = true;
                    this.buffer.clear();
                    return Poll::Ready(Some(Err(std::io::Error::other(error))));
                }
                Poll::Ready(None) => {
                    this.done = true;
                    if this.json { return Poll::Ready(Some(Ok(Bytes::from(std::mem::take(&mut this.buffer))))); }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<S, E> Stream for TracedResponseStream<S>
where S: Stream<Item = Result<Bytes, E>>, E: std::error::Error + 'static {
    type Item = Result<Bytes, E>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        if this.failed { return Poll::Ready(None); }
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                this.emitted = true;
                if let Some(evidence) = this.evidence.as_mut() { evidence.push(&chunk); }
                if let Some(trace) = this.trace.as_mut() {
                    trace.response_bytes = trace.response_bytes.saturating_add(chunk.len() as u64);
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(error))) => {
                this.failed = true;
                let unanswered = this.evidence.as_ref().map_or_else(|| this.failure.request.clone(),
                    |evidence| evidence.unanswered(&this.failure.request));
                if let Some(trace) = this.trace.as_ref() {
                    this.store.record_transport(TransportDiagnostic {
                        at_ms: now_ms(), trace_sequence: trace.sequence,
                        request_id_hashes: trace.request_id_hashes.clone(),
                        error_kind: BackendFailureKind::StreamInterrupted,
                        os_error_code: io_error_code(&error).1, elapsed_us: duration_us(this.started.elapsed()),
                        generation: this.failure.recovery.as_ref().map_or(0, |gate| gate.hook.generation()),
                        recovery: RecoveryOutcome::Unavailable,
                    });
                }
                this.finish(BackendFailureKind::StreamInterrupted.as_str());
                if unanswered.as_array().is_some_and(Vec::is_empty) {
                    // A complete upstream answer already reached the client.
                    // Do not overwrite it with a second answer for the same id.
                    return Poll::Ready(None);
                }
                if this.failure.sse || !this.emitted {
                    let value = backend_failure_value(&unanswered, BackendFailureKind::StreamInterrupted, false);
                    let bytes = if this.failure.sse { format!("\n\nevent: message\ndata: {value}\n\n") } else { value.to_string() };
                    Poll::Ready(Some(Ok(Bytes::from(bytes))))
                } else {
                    Poll::Ready(Some(Err(error)))
                }
            }
            Poll::Ready(None) => {
                if let Some(evidence) = this.evidence.as_mut() { evidence.finish(); }
                if let Some(gate) = &this.failure.recovery { gate.backend_succeeded(); }
                this.finish("completed");
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S> Drop for TracedResponseStream<S> {
    fn drop(&mut self) {
        self.finish("client_dropped");
    }
}

fn unix_time_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .min(u128::from(u64::MAX)) as u64
}

fn add_helper_ingress_timing_headers(
    builder: reqwest::RequestBuilder,
    started_unix_ns: u64,
    pre_backend_us: u64,
) -> reqwest::RequestBuilder {
    builder
        .header(
            HELPER_INGRESS_STARTED_UNIX_NS_HEADER,
            started_unix_ns.to_string(),
        )
        .header(
            HELPER_INGRESS_PRE_BACKEND_US_HEADER,
            pre_backend_us.to_string(),
        )
}

fn copy_request_headers(
    mut builder: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for (name, value) in headers.iter() {
        if is_hop_by_hop(name.as_str())
            || connection_header_names(headers, name.as_str())
            || name == header::HOST
            || name == header::CONTENT_LENGTH
            || name.as_str() == HELPER_INGRESS_STARTED_UNIX_NS_HEADER
            || name.as_str() == HELPER_INGRESS_PRE_BACKEND_US_HEADER
        {
            continue;
        }
        builder = builder.header(name, value);
    }
    builder
}

fn connection_header_names(headers: &HeaderMap, name: &str) -> bool {
    headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|token| token.trim().eq_ignore_ascii_case(name))
}

fn is_hop_by_hop(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "trailers"
            | "transfer-encoding"
            | "upgrade"
    )
}


enum ComputerControlInspection {
    None,
    Single { id: Value, action: String },
    BatchRejected,
}

fn inspect_computer_control(body: &[u8]) -> ComputerControlInspection {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return ComputerControlInspection::None;
    };
    match value {
        Value::Array(items) => {
            if items.iter().any(is_computer_control_request) {
                ComputerControlInspection::BatchRejected
            } else {
                ComputerControlInspection::None
            }
        }
        Value::Object(object) => computer_control_from_object(&object)
            .map(|(id, action)| ComputerControlInspection::Single { id, action })
            .unwrap_or(ComputerControlInspection::None),
        _ => ComputerControlInspection::None,
    }
}

/// Store-mutating management tools. The desktop app calls them directly on the
/// local runtime with its own admin credential, never through this ingress, so
/// ChatGPT (which reaches the runtime only through here) cannot invoke them.
const TUNNEL_BLOCKED_MANAGEMENT_TOOLS: &[&str] = &[
    "skill_install",
    "skill_activate",
    "skill_deactivate",
    "skill_remove_revision",
    "memory_scope_purge",
];

/// Returns the JSON-RPC id to answer with when the body calls a blocked tool,
/// directly or through `call_runtime_tool`; a batch containing one is refused whole.
fn tunnel_blocked_management_call(body: &[u8]) -> Option<Value> {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return None;
    };
    let blocked = |object: &serde_json::Map<String, Value>| {
        if object.get("method").and_then(Value::as_str) != Some("tools/call") {
            return false;
        }
        let Some(params) = object.get("params") else {
            return false;
        };
        let name = params.get("name").and_then(Value::as_str);
        let target = if name == Some("call_runtime_tool") {
            params
                .get("arguments")
                .and_then(|arguments| arguments.get("tool"))
                .and_then(Value::as_str)
        } else {
            name
        };
        target.is_some_and(|tool| TUNNEL_BLOCKED_MANAGEMENT_TOOLS.contains(&tool.trim()))
    };
    match value {
        Value::Object(object) => {
            blocked(&object).then(|| object.get("id").cloned().unwrap_or(Value::Null))
        }
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_object)
            .any(blocked)
            .then_some(Value::Null),
        _ => None,
    }
}

fn is_computer_control_request(value: &Value) -> bool {
    value
        .as_object()
        .and_then(computer_control_from_object)
        .is_some()
}

fn computer_control_from_object(
    object: &serde_json::Map<String, Value>,
) -> Option<(Value, String)> {
    if object.get("method").and_then(Value::as_str) != Some("tools/call") {
        return None;
    }
    let params = object.get("params")?;
    let name = params.get("name").and_then(Value::as_str)?;
    let arguments = params.get("arguments")?;
    let action = if name == "computer_control" {
        arguments.get("action").and_then(Value::as_str)
    } else if name == "call_runtime_tool"
        && arguments.get("tool").and_then(Value::as_str) == Some("computer_control")
    {
        arguments
            .get("arguments")
            .and_then(|inner| inner.get("action"))
            .and_then(Value::as_str)
    } else {
        return None;
    };
    Some((
        object.get("id").cloned().unwrap_or(Value::Null),
        action.unwrap_or("").to_string(),
    ))
}

fn computer_control_request_id(body: &[u8]) -> Value {
    let Ok(Value::Object(object)) = serde_json::from_slice::<Value>(body) else {
        return Value::Null;
    };
    object.get("id").cloned().unwrap_or(Value::Null)
}

fn computer_safety_response(
    id: Value,
    error_kind: &str,
    message: &str,
    outcome_unknown: bool,
) -> Response<Body> {
    let output = if outcome_unknown {
        json!({
            "error_kind": error_kind,
            "failure_kind": error_kind,
            "message": message,
            "execution_state": "outcome_unknown",
            "dispatch_certainty": "unknown",
            "recovery_kind": "reobserve",
            "reconcile_with": "computer_observe"
        })
    } else {
        json!({
            "error_kind": error_kind,
            "failure_kind": error_kind,
            "message": message,
            "execution_state": "not_started",
            "dispatch_certainty": "not_started",
            "recovery_kind": "user_action"
        })
    };
    let body = json!({
        "jsonrpc": "2.0",
        "result": {
            "content": [{"type": "text", "text": message}],
            "structuredContent": {
                "success": false,
                "output": output
            },
            "isError": true
        },
        "id": id
    });
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .expect("Computer safety response is valid")
}

#[derive(Default)]
struct McpMetadata {
    methods: Vec<String>,
    tool_names: Vec<String>,
    request_id_hashes: Vec<String>,
}

fn server_trace_id(headers: &HeaderMap) -> Option<String> {
    let value = headers.get("x-chadex-trace-id")?.to_str().ok()?;
    // Do not export arbitrary upstream header text to diagnostics.
    (value.len() == 36 && value.bytes().enumerate().all(|(index, byte)| {
        if matches!(index, 8 | 13 | 18 | 23) { byte == b'-' }
        else { byte.is_ascii_hexdigit() }
    })).then(|| value.to_string())
}

fn mcp_metadata(body: &[u8]) -> McpMetadata {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return McpMetadata::default();
    };
    match value {
        Value::Object(object) => {
            let mut metadata = McpMetadata::default();
            collect_mcp_metadata(&object, &mut metadata);
            metadata
        }
        Value::Array(items) => {
            let mut metadata = McpMetadata::default();
            for object in items.iter().filter_map(Value::as_object).take(16) {
                collect_mcp_metadata(object, &mut metadata);
            }
            metadata
        }
        _ => McpMetadata::default(),
    }
}

fn collect_mcp_metadata(object: &serde_json::Map<String, Value>, metadata: &mut McpMetadata) {
    if metadata.request_id_hashes.len() < 16 {
        if let Some(id @ (Value::String(_) | Value::Number(_))) = object.get("id") {
            // Hash canonical JSON, including its type; never retain raw string IDs,
            // which callers can populate with paths, prompts or credentials.
            let mut digest = Sha256::new();
            if serde_json::to_writer(&mut digest, id).is_ok() {
                metadata.request_id_hashes.push(format!("{:x}", digest.finalize()));
            }
        }
    }
    let Some(method) = object.get("method").and_then(Value::as_str) else {
        return;
    };
    if metadata.methods.len() < 16 {
        metadata.methods.push(bounded_label(method));
    }
    if method == "tools/call" && metadata.tool_names.len() < 16 {
        if let Some(name) = object
            .get("params")
            .and_then(|params| params.get("name"))
            .and_then(Value::as_str)
        {
            metadata.tool_names.push(bounded_label(name));
        }
    }
}

fn bounded_label(value: &str) -> String {
    value.chars().take(128).collect()
}

fn proxy_error() -> ChadexError {
    ChadexError::new(
        "ingress_unavailable",
        "Chadex could not reach its local MCP backend",
        "Restore the local service and retry the connection.",
    )
}

#[cfg(test)]
mod ingress_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_are_detected_for_single_and_batch_json_rpc() {
        let single = mcp_metadata(br#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#);
        assert_eq!(single.methods, vec!["tools/list"]);
        let batch = mcp_metadata(
            br#"[{"jsonrpc":"2.0","method":"initialize","id":1},{"jsonrpc":"2.0","method":"tools/call","params":{"name":"read_files"},"id":2}]"#,
        );
        assert_eq!(batch.methods, vec!["initialize", "tools/call"]);
        assert_eq!(batch.tool_names, vec!["read_files"]);
    }

    #[test]
    fn stale_epoch_cannot_verify_new_project() {
        let tracker = VerificationTracker::default();
        let old = tracker.reset(Some("/tmp/a".to_string()));
        let new = tracker.reset(Some("/tmp/b".to_string()));
        tracker.observe_request(old.epoch, &["tools/list".to_string()]);
        tracker.verify_project_read(old.epoch, "/tmp/a");
        let snapshot = tracker.snapshot();
        assert_eq!(snapshot.epoch, new.epoch);
        assert_eq!(snapshot.project_path.as_deref(), Some("/tmp/b"));
        assert!(!snapshot.chatgpt_connected);
        assert!(!snapshot.verified);
    }

    #[test]
    fn current_epoch_requires_matching_project_read_identity() {
        let tracker = VerificationTracker::default();
        let current = tracker.reset(Some("/tmp/current".to_string()));
        tracker.observe_request(current.epoch, &["tools/list".to_string()]);
        assert!(tracker.snapshot().chatgpt_connected);
        assert!(!tracker.snapshot().verified);
        tracker.verify_project_read(current.epoch, "/tmp/other");
        assert!(!tracker.snapshot().verified);
        tracker.verify_project_read(current.epoch, "/tmp/current");
        let snapshot = tracker.snapshot();
        assert!(snapshot.verified);
        assert!(snapshot.verified_at_ms.is_some());
    }

    #[tokio::test]
    async fn ingress_forwards_and_verifies_only_after_it_is_armed() {
        let backend_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend_address = backend_listener.local_addr().unwrap();
        let backend = Router::new().route(
            "/mcp",
            any(|body: axum::body::Bytes| async move {
                let request: Value = serde_json::from_slice(&body).unwrap();
                let proof_path = request
                    .pointer("/params/arguments/proof_path")
                    .and_then(Value::as_str);
                let result = if request.pointer("/params/name").and_then(Value::as_str)
                    == Some("read_files")
                {
                    serde_json::json!({
                        "content": [],
                        "structuredContent": {
                            "success": true,
                            "output": {
                                "project_path": proof_path,
                                "items": [{"success": proof_path.is_some()}]
                            }
                        }
                    })
                } else {
                    serde_json::json!({"content": []})
                };
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "application/json")],
                    serde_json::json!({"jsonrpc": "2.0", "result": result, "id": request["id"]})
                        .to_string(),
                )
            }),
        );
        let backend_task = tokio::spawn(async move {
            let _ = axum::serve(backend_listener, backend).await;
        });

        let tracker = Arc::new(VerificationTracker::default());
        let performance = Arc::new(PerformanceTraceStore::default());
        tracker.reset(Some("/tmp/current".to_string()));
        let ingress = McpIngress::start(
            format!("http://{backend_address}/mcp"),
            Arc::clone(&tracker),
            Arc::clone(&performance),
        )
        .await
        .unwrap();
        let client = Client::builder().no_proxy().build().unwrap();

        client
            .post(ingress.mcp_url())
            .header(header::AUTHORIZATION, "Bearer test")
            .body(r#"{"jsonrpc":"2.0","method":"tools/list","id":1}"#)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert!(!tracker.snapshot().chatgpt_connected);

        ingress.arm_verification();
        client
            .post(ingress.mcp_url())
            .header(header::AUTHORIZATION, "Bearer test")
            .body(r#"{"jsonrpc":"2.0","method":"tools/list","id":2}"#)
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert!(tracker.snapshot().chatgpt_connected);
        assert!(!tracker.snapshot().verified);

        client
            .post(ingress.mcp_url())
            .header(header::AUTHORIZATION, "Bearer test")
            .body(
                r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"read_files","arguments":{"proof_path":"/tmp/current"}},"id":3}"#,
            )
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        assert!(tracker.snapshot().verified);
        let traces = performance.snapshot(10);
        assert_eq!(traces.len(), 3);
        assert_eq!(traces.last().unwrap().tool_names, vec!["read_files"]);
        assert_eq!(traces.last().unwrap().completion, "completed");
        assert_eq!(traces.last().unwrap().status_code, Some(200));

        ingress.pause_and_drain().await;
        let paused = client
            .post(ingress.mcp_url())
            .header(header::AUTHORIZATION, "Bearer test")
            .body(
                r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"read_files"},"id":4}"#,
            )
            .send()
            .await
            .unwrap();
        assert_eq!(paused.status(), StatusCode::SERVICE_UNAVAILABLE);

        tracker.reset(Some("/tmp/next".to_string()));
        ingress.resume();
        client
            .post(ingress.mcp_url())
            .header(header::AUTHORIZATION, "Bearer test")
            .body(
                r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"read_files","arguments":{"proof_path":"/tmp/next"}},"id":5}"#,
            )
            .send()
            .await
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let next = tracker.snapshot();
        assert_eq!(next.project_path.as_deref(), Some("/tmp/next"));
        assert!(next.chatgpt_connected);
        assert!(next.verified);

        drop(ingress);
        backend_task.abort();
    }
}
