use super::performance::{duration_us, now_ms, McpPerformanceTrace, PerformanceTraceStore};
use super::{ChadexError, ChadexResult};
use axum::body::{to_bytes, Body};
use axum::extract::State;
use axum::http::{header, HeaderMap, Method, Request, Response, StatusCode};
use axum::routing::any;
use axum::Router;
use futures_core::Stream;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
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
    client: Client,
    admission: Arc<Semaphore>,
    tracker: Arc<VerificationTracker>,
    performance: Arc<PerformanceTraceStore>,
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
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| proxy_error())?;
        let armed = Arc::new(AtomicBool::new(false));
        let accepting = Arc::new(AtomicBool::new(true));
        let admission = Arc::new(Semaphore::new(MAX_IN_FLIGHT));
        let state = IngressState {
            backend_url,
            client,
            admission: Arc::clone(&admission),
            tracker,
            performance,
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
    let metadata = mcp_metadata(&body);

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
    let backend_started = Instant::now();
    let upstream = match builder.send().await {
        Ok(upstream) => upstream,
        Err(_) => {
            state.performance.push(McpPerformanceTrace {
                sequence: 0,
                started_at_ms,
                finished_at_ms: Some(now_ms()),
                request_id_hashes: metadata.request_id_hashes,
                server_trace_id: None,
                methods: metadata.methods,
                tool_names: metadata.tool_names,
                request_bytes: body.len().try_into().unwrap_or(u64::MAX),
                response_bytes: 0,
                status_code: None,
                ingress_pre_backend_us,
                backend_headers_us: duration_us(backend_started.elapsed()),
                response_stream_us: 0,
                total_us: duration_us(started.elapsed()),
                completion: "backend_error".to_string(),
            });
            return Err(proxy_error());
        }
    };
    let backend_headers_us = duration_us(backend_started.elapsed());
    let status = upstream.status();
    let headers = upstream.headers().clone();
    let evidence = if armed && status.is_success() {
        ResponseEvidence::new(
            &body,
            &headers,
            Arc::clone(&state.tracker),
            verification_epoch,
        )
    } else {
        None
    };
    let stream = TracedResponseStream::new(
        upstream.bytes_stream(),
        Arc::clone(&state.performance),
        McpPerformanceTrace {
            sequence: 0,
            started_at_ms,
            finished_at_ms: None,
            request_id_hashes: metadata.request_id_hashes,
            server_trace_id: server_trace_id(&headers),
            methods: metadata.methods,
            tool_names: metadata.tool_names,
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

struct TracedResponseStream<S> {
    inner: Pin<Box<S>>,
    store: Arc<PerformanceTraceStore>,
    trace: Option<McpPerformanceTrace>,
    started: Instant,
    headers_at: Instant,
    evidence: Option<ResponseEvidence>,
    permit: Option<OwnedSemaphorePermit>,
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
    ) -> Self {
        Self {
            inner: Box::pin(stream),
            store,
            trace: Some(trace),
            started,
            headers_at: Instant::now(),
            evidence,
            permit: Some(permit),
        }
    }

    fn finish(&mut self, completion: &str) {
        self.permit.take();
        let Some(mut trace) = self.trace.take() else {
            return;
        };
        trace.response_stream_us = duration_us(self.headers_at.elapsed());
        trace.total_us = duration_us(self.started.elapsed());
        trace.finished_at_ms = Some(now_ms());
        trace.completion = completion.to_string();
        self.store.push(trace);
    }
}

impl<S, T, E> Stream for TracedResponseStream<S>
where
    S: Stream<Item = Result<T, E>>,
    T: AsRef<[u8]>,
{
    type Item = Result<T, E>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                if let Some(evidence) = this.evidence.as_mut() {
                    evidence.push(chunk.as_ref());
                }
                if let Some(trace) = this.trace.as_mut() {
                    trace.response_bytes = trace
                        .response_bytes
                        .saturating_add(chunk.as_ref().len().try_into().unwrap_or(u64::MAX));
                }
                Poll::Ready(Some(Ok(chunk)))
            }
            Poll::Ready(Some(Err(error))) => {
                this.finish("stream_error");
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(None) => {
                if let Some(evidence) = this.evidence.as_mut() {
                    evidence.finish();
                }
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
