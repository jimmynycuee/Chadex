use super::*;

fn read_request(client_id: &str, cwd: &str) -> ShellFileOpRequest {
    ShellFileOpRequest {
        op: "read".to_string(),
        client_id: client_id.to_string(),
        path: "src/lib.rs".to_string(),
        cwd: Some(cwd.to_string()),
        content: None,
        max_bytes: Some(512 * 1024),
        old_text: None,
        pattern: None,
        expected_sha256: None,
        expected_prefix: None,
        start_line: Some(1),
        end_line: Some(10),
        line: None,
        create_dirs: false,
        wait_timeout_secs: 30,
    }
}

async fn register_read_runner(
    registry: &RunnerRegistry,
    client_id: &str,
    instance_id: &str,
    project_path: &str,
) {
    registry
        .register(current_runner_registration(runner_registration(
            client_id,
            instance_id,
            Vec::new(),
        )))
        .await
        .unwrap();
    crate::test_support::apply_project_inventory_snapshot(
        registry,
        client_id,
        instance_id,
        vec![project_summary("demo", project_path)],
    )
    .await;
}

#[tokio::test]
async fn exact_project_read_admission_rejects_runner_replacement_before_enqueue() {
    let registry = RunnerRegistry::default();
    let client_id = "project-read-runner-race";
    register_read_runner(&registry, client_id, "inst-a", "/tmp/project").await;

    registry
        .set_last_seen_for_test(client_id, now_ts() - RUNNER_ONLINE_WINDOW_SECS - 1)
        .await;
    register_read_runner(&registry, client_id, "inst-b", "/tmp/project").await;

    let error = registry
        .enqueue_project_file_read(
            read_request(client_id, "/tmp/project"),
            "demo",
            "/tmp/project",
            "inst-a",
            "tool_runtime".to_string(),
        )
        .await
        .unwrap_err();
    assert!(error.contains("stale_runner"), "{error}");
    assert!(registry
        .poll(RunnerPollRequest {
            client_id: client_id.to_string(),
            runner_instance_id: "inst-b".to_string(),
        })
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn exact_project_read_admission_rejects_changed_project_placement() {
    let registry = RunnerRegistry::default();
    let client_id = "project-read-placement-race";
    register_read_runner(&registry, client_id, "inst-a", "/tmp/project").await;
    crate::test_support::apply_project_inventory_snapshot(
        &registry,
        client_id,
        "inst-a",
        vec![project_summary("demo", "/tmp/replaced")],
    )
    .await;

    let error = registry
        .enqueue_project_file_read(
            read_request(client_id, "/tmp/project"),
            "demo",
            "/tmp/project",
            "inst-a",
            "tool_runtime".to_string(),
        )
        .await
        .unwrap_err();
    assert!(error.contains("stale_project"), "{error}");
    assert!(registry
        .poll(RunnerPollRequest {
            client_id: client_id.to_string(),
            runner_instance_id: "inst-a".to_string(),
        })
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn exact_project_read_revalidates_placement_before_dequeue() {
    let registry = RunnerRegistry::default();
    let client_id = "project-read-dequeue-race";
    register_read_runner(&registry, client_id, "inst-a", "/tmp/project").await;
    let (_request_id, response_rx) = registry
        .enqueue_project_file_read(
            read_request(client_id, "/tmp/project"),
            "demo",
            "/tmp/project",
            "inst-a",
            "tool_runtime".to_string(),
        )
        .await
        .unwrap();

    crate::test_support::apply_project_inventory_snapshot(
        &registry,
        client_id,
        "inst-a",
        vec![project_summary("demo", "/tmp/replaced")],
    )
    .await;
    assert!(registry
        .poll(RunnerPollRequest {
            client_id: client_id.to_string(),
            runner_instance_id: "inst-a".to_string(),
        })
        .await
        .unwrap()
        .is_none());
    let response = tokio::time::timeout(std::time::Duration::from_secs(1), response_rx)
        .await
        .expect("stale placement response timed out")
        .expect("stale placement response channel closed");
    assert_eq!(response.request_dispatched, Some(false));
    assert_eq!(
        response.command_execution_state,
        Some(ShellCommandExecutionState::NotStarted)
    );
    assert!(
        response
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("stale_project"),
        "{response:?}"
    );
}

#[tokio::test]
async fn exact_project_read_batch_admits_all_requests_in_order() {
    let registry = RunnerRegistry::default();
    let client_id = "project-read-batch";
    register_read_runner(&registry, client_id, "inst-a", "/tmp/project").await;

    let batch = registry
        .enqueue_project_file_reads(
            vec![
                read_request(client_id, "/tmp/project"),
                read_request(client_id, "/tmp/project"),
            ],
            "demo",
            "/tmp/project",
            "inst-a",
            "tool_runtime".to_string(),
        )
        .await
        .unwrap();
    assert_eq!(batch.len(), 2);

    let first = registry
        .poll(RunnerPollRequest {
            client_id: client_id.to_string(),
            runner_instance_id: "inst-a".to_string(),
        })
        .await
        .unwrap()
        .expect("first batch request");
    let second = registry
        .poll(RunnerPollRequest {
            client_id: client_id.to_string(),
            runner_instance_id: "inst-a".to_string(),
        })
        .await
        .unwrap()
        .expect("second batch request");
    assert_eq!(first.request_id, batch[0].0);
    assert_eq!(second.request_id, batch[1].0);
}

#[tokio::test]
async fn exact_project_read_batch_capacity_failure_is_atomic() {
    let registry = RunnerRegistry::default();
    let client_id = "project-read-batch-capacity";
    register_read_runner(&registry, client_id, "inst-a", "/tmp/project").await;

    for _ in 0..(MAX_QUEUED_REQUESTS_PER_RUNNER - 1) {
        registry
            .enqueue_project_file_read(
                read_request(client_id, "/tmp/project"),
                "demo",
                "/tmp/project",
                "inst-a",
                "tool_runtime".to_string(),
            )
            .await
            .unwrap();
    }
    let before = registry
        .get_runner_view(client_id)
        .await
        .expect("runner view")
        .pending_requests;
    assert_eq!(before, MAX_QUEUED_REQUESTS_PER_RUNNER - 1);

    let error = registry
        .enqueue_project_file_reads(
            vec![
                read_request(client_id, "/tmp/project"),
                read_request(client_id, "/tmp/project"),
            ],
            "demo",
            "/tmp/project",
            "inst-a",
            "tool_runtime".to_string(),
        )
        .await
        .unwrap_err();
    assert!(error.contains("queue full"), "{error}");
    let after = registry
        .get_runner_view(client_id)
        .await
        .expect("runner view")
        .pending_requests;
    assert_eq!(after, before, "failed batch must not partially enqueue");
}
