use super::*;
use std::time::Duration;

fn state(backend_url: String) -> IngressState {
    let tracker = Arc::new(VerificationTracker::default());
    tracker.reset(Some("/tmp/project".into()));
    IngressState {
        backend_url,
        client: Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        admission: Arc::new(Semaphore::new(MAX_IN_FLIGHT)),
        tracker,
        performance: Arc::new(PerformanceTraceStore::default()),
        armed: Arc::new(AtomicBool::new(true)),
        accepting: Arc::new(AtomicBool::new(true)),
    }
}
fn request() -> Request<Body> {
    Request::builder()
        .method("POST")
        .body(Body::from(
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"read_files"}}"#,
        ))
        .unwrap()
}
async fn backend(app: Router) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (url, task)
}

#[tokio::test]
async fn live_trace_is_visible_before_headers_and_completes_in_place() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let backend_entered = Arc::clone(&entered);
    let backend_release = Arc::clone(&release);
    let (url, server) = backend(Router::new().route("/mcp", any(move || {
        let entered = Arc::clone(&backend_entered);
        let release = Arc::clone(&backend_release);
        async move {
            entered.notify_one();
            release.notified().await;
            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#
        }
    }))).await;
    let state = state(url);
    let request_state = state.clone();
    let inflight = tokio::spawn(async move { proxy_inner(request_state, request()).await.unwrap() });
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    let live = state.performance.snapshot(10);
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].tool_names, vec!["read_files"]);
    assert_eq!(live[0].completion, "running");
    assert!(live[0].finished_at_ms.is_none());
    assert!(!state.tracker.snapshot().verified);
    release.notify_one();
    let response = inflight.await.unwrap();
    to_bytes(response.into_body(), MAX_REQUEST_BYTES).await.unwrap();
    let completed = state.performance.snapshot(10);
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].sequence, live[0].sequence);
    assert_eq!(completed[0].completion, "completed");
    assert!(completed[0].finished_at_ms.is_some());
    server.abort();
}

#[tokio::test]
async fn cancellation_before_headers_does_not_leave_a_live_trace() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let backend_entered = Arc::clone(&entered);
    let (url, server) = backend(Router::new().route("/mcp", any(move || {
        let entered = Arc::clone(&backend_entered);
        async move {
            entered.notify_one();
            std::future::pending::<StatusCode>().await
        }
    }))).await;
    let state = state(url);
    let request_state = state.clone();
    let inflight = tokio::spawn(async move { proxy_inner(request_state, request()).await.unwrap() });
    tokio::time::timeout(Duration::from_secs(2), entered.notified()).await.unwrap();
    inflight.abort();
    let _ = inflight.await;
    let traces = state.performance.snapshot(10);
    assert_eq!(traces.len(), 1);
    assert_eq!(traces[0].completion, "client_dropped");
    assert!(traces[0].finished_at_ms.is_some());
    assert_eq!(state.admission.available_permits(), MAX_IN_FLIGHT);
    server.abort();
}

#[test]
fn request_correlation_is_bounded_typed_and_does_not_retain_raw_ids() {
    let body = serde_json::to_vec(&serde_json::json!([
        {"id": "secret-path-or-token", "method": "tools/call"},
        {"id": 1, "method": "tools/list"},
        {"id": "1", "method": "tools/list"},
        {"method": "notifications/initialized"}
    ])).unwrap();
    let metadata = mcp_metadata(&body);
    assert_eq!(metadata.request_id_hashes.len(), 3);
    assert_ne!(metadata.request_id_hashes[1], metadata.request_id_hashes[2]);
    assert!(metadata.request_id_hashes.iter().all(|id| id.len() == 64 && id.bytes().all(|b| b.is_ascii_hexdigit())));
    assert!(!format!("{:?}", metadata.request_id_hashes).contains("secret-path"));
    let batch = vec![serde_json::json!({"id": 7, "method": "tools/list"}); 40];
    assert_eq!(mcp_metadata(&serde_json::to_vec(&batch).unwrap()).request_id_hashes.len(), 16);
    assert!(mcp_metadata(b"not json").request_id_hashes.is_empty());
}

#[tokio::test]
async fn response_trace_correlates_without_changing_body_or_verification() {
    const TRACE: &str = "b3e8101b-6692-4536-a6d0-3989772a2f51";
    let (url, server) = backend(Router::new().route("/mcp", any(|| async {
        ([("x-chadex-trace-id", TRACE)], r#"{"jsonrpc":"2.0","id":1,"result":{}}"#)
    }))).await;
    let state = state(url);
    let response = proxy_inner(state.clone(), request()).await.unwrap();
    assert_eq!(response.headers()["x-chadex-trace-id"], TRACE);
    let bytes = to_bytes(response.into_body(), MAX_REQUEST_BYTES).await.unwrap();
    assert_eq!(&bytes[..], br#"{"jsonrpc":"2.0","id":1,"result":{}}"#);
    let traces = state.performance.snapshot(1);
    let trace = &traces[0];
    assert_eq!(trace.server_trace_id.as_deref(), Some(TRACE));
    assert_eq!(trace.request_id_hashes, mcp_metadata(br#"{"id":1}"#).request_id_hashes);
    assert!(trace.finished_at_ms.unwrap() >= trace.started_at_ms);
    assert!(!state.tracker.snapshot().verified);
    server.abort();
    let mut headers = HeaderMap::new();
    headers.insert("x-chadex-trace-id", "sensitive-invalid-header".parse().unwrap());
    assert!(server_trace_id(&headers).is_none());
}

#[tokio::test]
async fn http_ok_tool_failure_is_observed_without_changing_response() {
    const BODY: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[],"isError":true}}"#;
    let (url, server) = backend(Router::new().route("/mcp", any(|| async {
        ([("content-type", "application/json")], BODY)
    }))).await;
    let state = state(url);
    let response = proxy_inner(state.clone(), request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), MAX_REQUEST_BYTES).await.unwrap();
    assert_eq!(&bytes[..], BODY.as_bytes());
    let traces = state.performance.snapshot(1);
    assert_eq!(traces[0].tool_failed, Some(true));
    assert_eq!(traces[0].completion, "completed");
    assert!(!state.tracker.snapshot().verified);
    server.abort();
}

#[tokio::test]
async fn unauthorized_and_redirect_responses_never_verify() {
    for status in [StatusCode::UNAUTHORIZED, StatusCode::FOUND] {
        let (url, server) = backend(
            Router::new()
                .route(
                    "/mcp",
                    any(move || async move {
                        (
                            status,
                            [
                                (header::LOCATION, "/success"),
                                (header::CONTENT_TYPE, "application/json"),
                            ],
                            r#"{"jsonrpc":"2.0","id":1,"result":{}}"#,
                        )
                    }),
                )
                .route(
                    "/success",
                    any(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
                ),
        )
        .await;
        let state = state(url);
        let response = proxy_inner(state.clone(), request()).await.unwrap();
        assert_eq!(response.status(), status);
        to_bytes(response.into_body(), MAX_REQUEST_BYTES)
            .await
            .unwrap();
        assert!(!state.tracker.snapshot().chatgpt_connected);
        assert!(!state.tracker.snapshot().verified);
        server.abort();
    }
}

#[tokio::test]
async fn admission_and_body_limits_reject_before_backend_dispatch() {
    let state = state("http://127.0.0.1:1/mcp".into());
    let permits = state
        .admission
        .clone()
        .acquire_many_owned(MAX_IN_FLIGHT as u32)
        .await
        .unwrap();
    let response = proxy_inner(state.clone(), request()).await.unwrap();
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    drop(permits);
    let response = proxy_inner(
        state.clone(),
        Request::new(Body::from(vec![0; MAX_REQUEST_BYTES + 1])),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(state.admission.available_permits(), MAX_IN_FLIGHT);
    assert!(!state.tracker.snapshot().verified);
}

struct Pending;
impl Stream for Pending {
    type Item = Result<Vec<u8>, std::io::Error>;
    fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Poll::Pending
    }
}

#[tokio::test]
async fn slow_body_expires_and_cancellation_releases_admission() {
    let state = state("http://127.0.0.1:1/mcp".into());
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        proxy_inner(task_state, Request::new(Body::from_stream(Pending))).await
    });
    tokio::task::yield_now().await;
    assert_eq!(state.admission.available_permits(), MAX_IN_FLIGHT - 1);
    task.abort();
    let _ = task.await;
    assert_eq!(state.admission.available_permits(), MAX_IN_FLIGHT);
    let response = proxy_inner(state.clone(), Request::new(Body::from_stream(Pending)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::REQUEST_TIMEOUT);
    assert_eq!(state.admission.available_permits(), MAX_IN_FLIGHT);
}

#[tokio::test]
async fn response_drop_releases_admission_without_verifying() {
    let (url, server) = backend(Router::new().route(
        "/mcp",
        any(|| async {
            (
                [(header::CONTENT_TYPE, "text/event-stream")],
                Body::from_stream(Pending),
            )
        }),
    ))
    .await;
    let state = state(url);
    let response = proxy_inner(state.clone(), request()).await.unwrap();
    assert_eq!(state.admission.available_permits(), MAX_IN_FLIGHT - 1);
    drop(response);
    assert_eq!(state.admission.available_permits(), MAX_IN_FLIGHT);
    assert!(!state.tracker.snapshot().verified);
    server.abort();
}

#[test]
fn connection_nominated_headers_are_not_forwarded() {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONNECTION,
        "keep-alive, x-private-hop".parse().unwrap(),
    );
    headers.insert("x-private-hop", "private".parse().unwrap());
    headers.insert(header::AUTHORIZATION, "Bearer test".parse().unwrap());
    headers.insert(HELPER_INGRESS_STARTED_UNIX_NS_HEADER, "1".parse().unwrap());
    headers.insert(HELPER_INGRESS_PRE_BACKEND_US_HEADER, "2".parse().unwrap());
    let builder = copy_request_headers(Client::new().get("http://127.0.0.1/"), &headers);
    let request = add_helper_ingress_timing_headers(builder, 1234, 56)
        .build()
        .unwrap();
    assert!(!request.headers().contains_key("x-private-hop"));
    assert!(!request.headers().contains_key(header::CONNECTION));
    assert!(request.headers().contains_key(header::AUTHORIZATION));
    assert_eq!(
        request.headers()[HELPER_INGRESS_STARTED_UNIX_NS_HEADER],
        "1234"
    );
    assert_eq!(
        request.headers()[HELPER_INGRESS_PRE_BACKEND_US_HEADER],
        "56"
    );
}
