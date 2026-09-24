use super::*;

fn local_edit_request(client_id: &str, expected_sha256: Option<&str>) -> ShellFileOpRequest {
    let mut change = serde_json::json!({
        "kind": "edit",
        "path": "src/lib.rs",
        "edits": [{
            "kind": "replace_exact",
            "old_text": "fn old() {}",
            "new_text": "fn new() {}"
        }]
    });
    if let Some(expected_sha256) = expected_sha256 {
        change["expected_sha256"] = expected_sha256.into();
    }
    ShellFileOpRequest {
        op: "apply_text_edits".to_string(),
        client_id: client_id.to_string(),
        path: "src/lib.rs".to_string(),
        cwd: Some("/tmp/proj".to_string()),
        content: Some(
            serde_json::json!({
                "changes": [change],
                "recovery_metadata_version": 1
            })
            .to_string(),
        ),
        max_bytes: None,
        old_text: None,
        pattern: None,
        expected_sha256: None,
        expected_prefix: None,
        start_line: None,
        end_line: None,
        line: None,
        create_dirs: false,
        wait_timeout_secs: 30,
    }
}

async fn register_local_guard_instance(
    registry: &RunnerRegistry,
    client_id: &str,
    supported: bool,
) {
    register_instance_with_capabilities(
        registry,
        client_id,
        "inst",
        RunnerCapabilities {
            file_write: true,
            apply_text_edit_local_guard_without_sha: supported,
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn sha_less_local_edit_requires_explicit_capability_before_queue_admission() {
    let registry = RunnerRegistry::default();
    register_local_guard_instance(&registry, "legacy-local-edit", false).await;

    let error = registry
        .enqueue_file_op(
            local_edit_request("legacy-local-edit", None),
            "tester".to_string(),
        )
        .await
        .unwrap_err();
    assert!(error.contains("capability_unavailable"), "{error}");
    assert!(
        error.contains("apply_text_edit_local_guard_without_sha"),
        "{error}"
    );
    assert!(registry
        .poll(RunnerPollRequest {
            client_id: "legacy-local-edit".to_string(),
            runner_instance_id: "inst".to_string(),
        })
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn current_runner_capability_admits_sha_less_local_edit() {
    let registry = RunnerRegistry::default();
    register_local_guard_instance(&registry, "current-local-edit", true).await;

    let (request_id, _rx) = registry
        .enqueue_file_op(
            local_edit_request("current-local-edit", None),
            "tester".to_string(),
        )
        .await
        .expect("capable Runner should accept SHA-less local edit");
    let request = registry
        .poll(RunnerPollRequest {
            client_id: "current-local-edit".to_string(),
            runner_instance_id: "inst".to_string(),
        })
        .await
        .unwrap()
        .expect("request should be queued");
    assert_eq!(request.request_id, request_id);
    let payload: serde_json::Value =
        serde_json::from_str(request.content.as_deref().unwrap()).unwrap();
    assert!(payload["changes"][0].get("expected_sha256").is_none());
}

#[tokio::test]
async fn older_runner_still_accepts_local_edit_with_existing_wire_sha_guard() {
    let registry = RunnerRegistry::default();
    register_local_guard_instance(&registry, "legacy-guarded-edit", false).await;
    let hash = "a".repeat(64);

    let (request_id, _rx) = registry
        .enqueue_file_op(
            local_edit_request("legacy-guarded-edit", Some(&hash)),
            "tester".to_string(),
        )
        .await
        .expect("existing SHA wire guard remains compatible with older Runner");
    let request = registry
        .poll(RunnerPollRequest {
            client_id: "legacy-guarded-edit".to_string(),
            runner_instance_id: "inst".to_string(),
        })
        .await
        .unwrap()
        .expect("request should be queued");
    assert_eq!(request.request_id, request_id);
    let payload: serde_json::Value =
        serde_json::from_str(request.content.as_deref().unwrap()).unwrap();
    assert_eq!(payload["changes"][0]["expected_sha256"], hash);
}

#[test]
fn local_guard_capability_is_additive_and_missing_defaults_false() {
    let baseline = v2_baseline_capabilities();
    assert!(!baseline.apply_text_edit_local_guard_without_sha);
    assert!(!RunnerFeatureSet::try_from_registration(&baseline)
        .unwrap()
        .supports(RunnerFeature::ApplyTextEditLocalGuardWithoutSha));

    let mut current = baseline;
    current.apply_text_edit_local_guard_without_sha = true;
    assert!(RunnerFeatureSet::try_from_registration(&current)
        .unwrap()
        .supports(RunnerFeature::ApplyTextEditLocalGuardWithoutSha));

    let legacy: RunnerCapabilities = serde_json::from_str(
        r#"{"shell":true,"file_read":true,"file_write":true,"apply_text_edit_occurrence":true}"#,
    )
    .unwrap();
    assert!(!legacy.apply_text_edit_local_guard_without_sha);
    let serialized = serde_json::to_value(RunnerCapabilities::default()).unwrap();
    assert!(serialized
        .get("apply_text_edit_local_guard_without_sha")
        .is_none());
}
