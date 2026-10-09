use super::*;

#[tokio::test]
async fn computer_accessibility_enqueue_requires_distinct_capability() {
    let registry = RunnerRegistry::default();
    register_computer_test_runner(&registry, "computer-ax", "alice", true, false, false, false)
        .await;
    let alice = auth_context(Some("alice"), false);
    let error = registry
        .enqueue_computer(
            "computer-ax".to_string(),
            "computer_accessibility_status",
            "{}".to_string(),
            "alice".to_string(),
            Some(&alice),
            5,
        )
        .await
        .unwrap_err();
    assert!(error.contains("does not support computer_accessibility_observe"));

    register_computer_test_runner(&registry, "computer-ax", "alice", true, true, false, false)
        .await;
    let (_request_id, _rx) = registry
        .enqueue_computer(
            "computer-ax".to_string(),
            "computer_accessibility_tree",
            r#"{"surface_id":"surface_test","max_depth":2,"max_nodes":8}"#.to_string(),
            "alice".to_string(),
            Some(&alice),
            5,
        )
        .await
        .unwrap();
    let request = registry
        .poll(RunnerPollRequest {
            client_id: "computer-ax".to_string(),
            runner_instance_id: "computer-inst".to_string(),
        })
        .await
        .unwrap()
        .expect("queued accessibility request");
    assert_eq!(request.kind, "computer_accessibility_tree");
    assert!(request.command.is_empty());
}

#[tokio::test]
async fn computer_element_state_requires_its_own_additive_capability() {
    let registry = RunnerRegistry::default();
    let alice = auth_context(Some("alice"), false);
    register_computer_test_runner(
        &registry,
        "computer-state",
        "alice",
        true,
        true,
        false,
        false,
    )
    .await;
    let payload = r#"{"surface_id":"surface_test","element_id":"element_test"}"#;
    let error = registry
        .enqueue_computer(
            "computer-state".to_string(),
            "computer_element_state",
            payload.to_string(),
            "alice".to_string(),
            Some(&alice),
            5,
        )
        .await
        .unwrap_err();
    assert!(error.contains("does not support computer_element_state"));

    registry
        .register(current_runner_registration(RunnerRegisterRequest {
            process_started_at: None,
            build: None,
            job_concurrency_limit: None,
            job_inventory: None,
            coding_agent_providers: None,
            coding_agent_inventory: None,
            client_id: "computer-state-capable".to_string(),
            runner_instance_id: "computer-inst".to_string(),
            runner_protocol_generation: crate::runner_protocol::RUNNER_PROTOCOL_GENERATION_V2,
            display_name: None,
            owner: Some("alice".to_string()),
            hostname: None,
            host_context: None,
            capabilities: RunnerCapabilities {
                shell: true,
                file_read: true,
                computer_observe: true,
                computer_accessibility_observe: true,
                computer_element_state: true,
                ..Default::default()
            },
            policy: None,
        }))
        .await
        .unwrap();
    let (_request_id, _rx) = registry
        .enqueue_computer(
            "computer-state-capable".to_string(),
            "computer_element_state",
            payload.to_string(),
            "alice".to_string(),
            Some(&alice),
            5,
        )
        .await
        .unwrap();
    let request = registry
        .poll(RunnerPollRequest {
            client_id: "computer-state-capable".to_string(),
            runner_instance_id: "computer-inst".to_string(),
        })
        .await
        .unwrap()
        .expect("queued computer element-state request");
    assert_eq!(request.kind, "computer_element_state");
    assert_eq!(request.stdin.as_deref(), Some(payload));
    assert!(request.command.is_empty());
}

async fn register_accessibility_query_runner(
    registry: &RunnerRegistry,
    client_id: &str,
    observe: bool,
    query: bool,
) {
    registry
        .register(current_runner_registration(RunnerRegisterRequest {
            process_started_at: None,
            build: None,
            job_concurrency_limit: None,
            job_inventory: None,
            coding_agent_providers: None,
            coding_agent_inventory: None,
            client_id: client_id.to_string(),
            runner_instance_id: "computer-inst".to_string(),
            runner_protocol_generation: crate::runner_protocol::RUNNER_PROTOCOL_GENERATION_V2,
            display_name: None,
            owner: Some("alice".to_string()),
            hostname: None,
            host_context: None,
            capabilities: RunnerCapabilities {
                shell: true,
                file_read: true,
                computer_observe: true,
                computer_accessibility_observe: observe,
                computer_accessibility_query: query,
                ..Default::default()
            },
            policy: None,
        }))
        .await
        .unwrap();
}

const SUBTREE_PAYLOAD: &str =
    r#"{"surface_id":"surface_test","root_element_id":null,"max_depth":2,"max_nodes":8}"#;
const FIND_PAYLOAD: &str = r#"{"surface_id":"surface_test","root_element_id":null,"role":"AXButton","subrole":null,"label":null,"value":null,"focused":null,"enabled":null,"limit":8,"max_depth":32}"#;

#[tokio::test]
async fn computer_accessibility_query_kinds_require_the_query_capability() {
    let registry = RunnerRegistry::default();
    let alice = auth_context(Some("alice"), false);
    // A Runner that only observes (an older build) must be refused for both new kinds.
    register_accessibility_query_runner(&registry, "ax-old", true, false).await;
    for (kind, payload) in [
        ("computer_accessibility_subtree", SUBTREE_PAYLOAD),
        ("computer_accessibility_find", FIND_PAYLOAD),
    ] {
        let error = registry
            .enqueue_computer(
                "ax-old".to_string(),
                kind,
                payload.to_string(),
                "alice".to_string(),
                Some(&alice),
                5,
            )
            .await
            .unwrap_err();
        assert!(
            error.contains("does not support computer_accessibility_query"),
            "{kind}: {error}"
        );
    }
    // The legacy kind keeps working on that Runner.
    registry
        .enqueue_computer(
            "ax-old".to_string(),
            "computer_accessibility_tree",
            r#"{"surface_id":"surface_test","max_depth":2,"max_nodes":8}"#.to_string(),
            "alice".to_string(),
            Some(&alice),
            5,
        )
        .await
        .unwrap();

    // The query capability alone is not enough: it never implies observation.
    register_accessibility_query_runner(&registry, "ax-query-only", false, true).await;
    let error = registry
        .enqueue_computer(
            "ax-query-only".to_string(),
            "computer_accessibility_find",
            FIND_PAYLOAD.to_string(),
            "alice".to_string(),
            Some(&alice),
            5,
        )
        .await
        .unwrap_err();
    assert!(
        error.contains("does not support computer_accessibility_observe"),
        "{error}"
    );

    register_accessibility_query_runner(&registry, "ax-new", true, true).await;
    for (kind, payload) in [
        ("computer_accessibility_subtree", SUBTREE_PAYLOAD),
        ("computer_accessibility_find", FIND_PAYLOAD),
    ] {
        registry
            .enqueue_computer(
                "ax-new".to_string(),
                kind,
                payload.to_string(),
                "alice".to_string(),
                Some(&alice),
                5,
            )
            .await
            .unwrap();
        let request = registry
            .poll(RunnerPollRequest {
                client_id: "ax-new".to_string(),
                runner_instance_id: "computer-inst".to_string(),
            })
            .await
            .unwrap()
            .expect("queued accessibility query request");
        assert_eq!(request.kind, kind);
        assert!(request.command.is_empty());
    }
}

#[tokio::test]
async fn computer_accessibility_find_payload_bound_is_larger_than_the_baseline_only_for_find() {
    let registry = RunnerRegistry::default();
    let alice = auth_context(Some("alice"), false);
    register_accessibility_query_runner(&registry, "ax-bounds", true, true).await;
    // Worst-case JSON escaping of four 256-byte filters is accepted for find ...
    let escaped = "\\u0001".repeat(256);
    let payload = format!(
        r#"{{"surface_id":"surface_test","root_element_id":null,"role":"{escaped}","subrole":"{escaped}","label":"{escaped}","value":"{escaped}","focused":null,"enabled":null,"limit":8,"max_depth":32}}"#
    );
    assert!(payload.len() > crate::runner_protocol::SHELL_COMPUTER_REQUEST_PAYLOAD_MAX_BYTES);
    registry
        .enqueue_computer(
            "ax-bounds".to_string(),
            "computer_accessibility_find",
            payload.clone(),
            "alice".to_string(),
            Some(&alice),
            5,
        )
        .await
        .unwrap();
    // ... but not for the other observation kinds, and never beyond the find bound.
    let error = registry
        .enqueue_computer(
            "ax-bounds".to_string(),
            "computer_accessibility_subtree",
            payload.clone(),
            "alice".to_string(),
            Some(&alice),
            5,
        )
        .await
        .unwrap_err();
    assert!(error.contains("invalid or too large"), "{error}");
    let oversized = format!(
        "{payload}{}",
        " ".repeat(crate::runner_protocol::SHELL_COMPUTER_ACCESSIBILITY_FIND_PAYLOAD_MAX_BYTES)
    );
    let error = registry
        .enqueue_computer(
            "ax-bounds".to_string(),
            "computer_accessibility_find",
            oversized,
            "alice".to_string(),
            Some(&alice),
            5,
        )
        .await
        .unwrap_err();
    assert!(error.contains("invalid or too large"), "{error}");
}
