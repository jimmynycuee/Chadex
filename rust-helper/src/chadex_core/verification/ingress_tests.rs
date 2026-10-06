use super::*;
use std::time::Duration;
use crate::chadex_core::computer_safety::ComputerControlMode;

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
        computer_safety: Arc::new(ComputerSafetyController::default()),
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

fn computer_request(action: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .body(Body::from(
            json!({
                "jsonrpc": "2.0",
                "id": 41,
                "method": "tools/call",
                "params": {
                    "name": "computer_control",
                    "arguments": {
                        "action": action,
                        "client_id": "test-runner",
                        "surface_id": "surface_test",
                        "element_id": "element_test"
                    }
                }
            })
            .to_string(),
        ))
        .unwrap()
}

async fn response_json(response: Response<Body>) -> Value {
    let bytes = to_bytes(response.into_body(), MAX_REQUEST_BYTES).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
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


#[tokio::test]
async fn computer_read_only_denies_before_backend_dispatch() {
    let state = state("http://127.0.0.1:1/mcp".into());
    state
        .computer_safety
        .set_mode(ComputerControlMode::ReadOnly);

    let response = proxy_inner(state.clone(), computer_request("press"))
        .await
        .unwrap();
    let body = response_json(response).await;
    assert_eq!(body["id"], 41);
    assert_eq!(body["result"]["isError"], true);
    assert_eq!(
        body["result"]["structuredContent"]["output"]["error_kind"],
        "computer_control_read_only"
    );
    assert_eq!(
        body["result"]["structuredContent"]["output"]["execution_state"],
        "not_started"
    );
    assert!(state
        .computer_safety
        .snapshot()
        .audit
        .iter()
        .any(|event| event.event == "control_denied"
            && event.reason.as_deref() == Some("read_only")));
}

#[tokio::test]
async fn computer_ask_mode_allows_only_after_explicit_approval() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let backend_entered = Arc::clone(&entered);
    let (url, server) = backend(Router::new().route(
        "/mcp",
        any(move || {
            let entered = Arc::clone(&backend_entered);
            async move {
                entered.notify_one();
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "application/json")],
                    r#"{"jsonrpc":"2.0","id":41,"result":{"content":[],"isError":false}}"#,
                )
            }
        }),
    ))
    .await;
    let state = state(url);
    let request_state = state.clone();
    let inflight =
        tokio::spawn(async move { proxy_inner(request_state, computer_request("press")).await.unwrap() });

    let approval = loop {
        if let Some(approval) = state
            .computer_safety
            .snapshot()
            .pending_approvals
            .first()
            .cloned()
        {
            break approval;
        }
        tokio::task::yield_now().await;
    };
    assert_eq!(approval.action, "press");
    assert!(state.computer_safety.approve(&approval.approval_id));
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();

    let body = response_json(inflight.await.unwrap()).await;
    assert_eq!(body["result"]["isError"], false);
    let audit = state.computer_safety.snapshot().audit;
    assert!(audit.iter().any(|event| event.event == "approval_requested"));
    assert!(audit.iter().any(|event| event.event == "approval_approved"));
    assert!(audit.iter().any(|event| event.event == "control_dispatched"));
    server.abort();
}

#[tokio::test]
async fn computer_ask_mode_denial_never_reaches_backend() {
    let state = state("http://127.0.0.1:1/mcp".into());
    let request_state = state.clone();
    let inflight =
        tokio::spawn(async move { proxy_inner(request_state, computer_request("input_text")).await.unwrap() });
    let approval = loop {
        if let Some(approval) = state
            .computer_safety
            .snapshot()
            .pending_approvals
            .first()
            .cloned()
        {
            break approval;
        }
        tokio::task::yield_now().await;
    };
    assert_eq!(approval.action, "input_text");
    assert!(state.computer_safety.deny(&approval.approval_id));
    let body = response_json(inflight.await.unwrap()).await;
    assert_eq!(
        body["result"]["structuredContent"]["output"]["error_kind"],
        "computer_control_denied"
    );
    assert_eq!(
        body["result"]["structuredContent"]["output"]["dispatch_certainty"],
        "not_started"
    );
}


#[tokio::test]
async fn computer_stop_before_action_denies_without_dispatch() {
    let state = state("http://127.0.0.1:1/mcp".into());
    state
        .computer_safety
        .set_mode(ComputerControlMode::AllowSession);
    state.computer_safety.stop();

    let body = response_json(
        proxy_inner(state.clone(), computer_request("key"))
            .await
            .unwrap(),
    )
    .await;
    let output = &body["result"]["structuredContent"]["output"];
    assert_eq!(output["error_kind"], "computer_control_stopped");
    assert_eq!(output["execution_state"], "not_started");
    assert_eq!(output["dispatch_certainty"], "not_started");
}

#[test]
fn computer_control_inspection_covers_direct_gateway_and_rejects_batches() {
    let direct = serde_json::to_vec(&json!({
        "jsonrpc":"2.0",
        "id": 7,
        "method":"tools/call",
        "params":{"name":"computer_control","arguments":{"action":"press"}}
    }))
    .unwrap();
    assert!(matches!(
        inspect_computer_control(&direct),
        ComputerControlInspection::Single { id, action }
            if id == json!(7) && action == "press"
    ));

    let gateway = serde_json::to_vec(&json!({
        "jsonrpc":"2.0",
        "id": "gateway",
        "method":"tools/call",
        "params":{
            "name":"call_runtime_tool",
            "arguments":{
                "tool":"computer_control",
                "arguments":{"action":"write_clipboard","text":"MUST_NOT_BE_RETAINED"}
            }
        }
    }))
    .unwrap();
    assert!(matches!(
        inspect_computer_control(&gateway),
        ComputerControlInspection::Single { id, action }
            if id == json!("gateway") && action == "write_clipboard"
    ));

    let batch = serde_json::to_vec(&json!([
        {"jsonrpc":"2.0","id":1,"method":"tools/list"},
        {"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"computer_control","arguments":{"action":"press"}}}
    ]))
    .unwrap();
    assert!(matches!(
        inspect_computer_control(&batch),
        ComputerControlInspection::BatchRejected
    ));
}

#[tokio::test]
async fn computer_stop_during_backend_dispatch_returns_outcome_unknown_and_reobserve() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let backend_entered = Arc::clone(&entered);
    let backend_release = Arc::clone(&release);
    let (url, server) = backend(Router::new().route(
        "/mcp",
        any(move || {
            let entered = Arc::clone(&backend_entered);
            let release = Arc::clone(&backend_release);
            async move {
                entered.notify_one();
                release.notified().await;
                (
                    StatusCode::OK,
                    [(header::CONTENT_TYPE, "application/json")],
                    r#"{"jsonrpc":"2.0","id":41,"result":{"content":[],"isError":false}}"#,
                )
            }
        }),
    ))
    .await;
    let state = state(url);
    state
        .computer_safety
        .set_mode(ComputerControlMode::AllowSession);
    let request_state = state.clone();
    let inflight =
        tokio::spawn(async move { proxy_inner(request_state, computer_request("pointer_click")).await.unwrap() });

    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    state.computer_safety.stop();

    let body = response_json(inflight.await.unwrap()).await;
    let output = &body["result"]["structuredContent"]["output"];
    assert_eq!(output["error_kind"], "computer_control_stopped_during_dispatch");
    assert_eq!(output["execution_state"], "outcome_unknown");
    assert_eq!(output["dispatch_certainty"], "unknown");
    assert_eq!(output["recovery_kind"], "reobserve");
    assert_eq!(output["reconcile_with"], "computer_observe");
    assert!(state
        .computer_safety
        .snapshot()
        .audit
        .iter()
        .any(|event| event.event == "control_dispatch_interrupted"));
    let traces = state.performance.snapshot(1);
    assert_eq!(traces.len(), 1);
    assert_eq!(
        traces[0].completion,
        "computer_control_stopped_during_dispatch"
    );
    release.notify_waiters();
    server.abort();
}

#[tokio::test]
async fn computer_control_in_json_rpc_batch_fails_closed_before_backend() {
    let state = state("http://127.0.0.1:1/mcp".into());
    state
        .computer_safety
        .set_mode(ComputerControlMode::AllowSession);
    let request = Request::builder()
        .method("POST")
        .body(Body::from(
            json!([
                {"jsonrpc":"2.0","id":1,"method":"tools/list"},
                {
                    "jsonrpc":"2.0",
                    "id":2,
                    "method":"tools/call",
                    "params":{"name":"computer_control","arguments":{"action":"press"}}
                }
            ])
            .to_string(),
        ))
        .unwrap();
    let body = response_json(proxy_inner(state.clone(), request).await.unwrap()).await;
    assert_eq!(
        body["result"]["structuredContent"]["output"]["error_kind"],
        "computer_control_batch_rejected"
    );
    assert!(state
        .computer_safety
        .snapshot()
        .audit
        .iter()
        .any(|event| event.reason.as_deref() == Some("batch_not_supported")));
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

fn tool_call(id: i64, name: &str, arguments: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":arguments}})
}

async fn post(state: IngressState, body: Value) -> Value {
    let request = Request::builder()
        .method("POST")
        .body(Body::from(body.to_string()))
        .unwrap();
    response_json(proxy_inner(state, request).await.unwrap()).await
}

#[tokio::test]
async fn skill_and_memory_management_tools_are_refused_over_the_tunnel() {
    // Backend is unreachable: any forwarded request would come back as a proxy error.
    let state = state("http://127.0.0.1:1/mcp".into());
    for tool in [
        "skill_install",
        "skill_activate",
        "skill_deactivate",
        "skill_remove_revision",
        "memory_scope_purge",
    ] {
        for body in [
            tool_call(7, tool, json!({"project":"p","skill_key":"k"})),
            tool_call(
                7,
                "call_runtime_tool",
                json!({"tool": tool, "arguments": {"project":"p"}}),
            ),
        ] {
            let response = post(state.clone(), body).await;
            assert_eq!(response["id"], 7, "{tool}");
            let output = &response["result"]["structuredContent"]["output"];
            assert_eq!(
                output["error_kind"], "management_tool_not_available_over_tunnel",
                "{tool}"
            );
            assert_eq!(output["dispatch_certainty"], "not_started");
        }
    }
    let batch = json!([
        {"jsonrpc":"2.0","id":1,"method":"tools/list"},
        tool_call(2, "skill_install", json!({}))
    ]);
    let response = post(state.clone(), batch).await;
    assert_eq!(
        response["result"]["structuredContent"]["output"]["error_kind"],
        "management_tool_not_available_over_tunnel"
    );
}

#[test]
fn read_only_skill_and_memory_tools_are_not_blocked() {
    for tool in [
        "skill_list",
        "skill_load",
        "skill_read_file",
        "skill_inventory",
        "skill_versions",
        "run_skill_resource",
        "memory_search",
        "memory_set",
        "memory_scope_list",
    ] {
        let body = tool_call(1, tool, json!({})).to_string();
        assert!(tunnel_blocked_management_call(body.as_bytes()).is_none(), "{tool}");
        let wrapped = tool_call(1, "call_runtime_tool", json!({"tool": tool})).to_string();
        assert!(tunnel_blocked_management_call(wrapped.as_bytes()).is_none(), "{tool}");
    }
}
