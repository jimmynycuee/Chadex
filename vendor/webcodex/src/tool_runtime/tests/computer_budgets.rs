use super::support::*;
use crate::runner_protocol::{RunnerCapabilities, RunnerResultRequest};
use crate::tool_runtime::tool_call::ComputerObserveToolCall;
use crate::tool_runtime::ToolCall;
use base64::{engine::general_purpose, Engine as _};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const DISPLAY_ID: &str = "display_iavN7wEjRWeJq83v";

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test]
async fn computer_display_snapshot_clamps_budget_before_runner_and_validates_effective_bound() {
    let client_id = "computer-budget-display";
    let runtime = runtime_with_agent_project(client_id);
    let auth = open_auth_context();
    register_agent_projects_for_auth(
        &runtime,
        client_id,
        &auth,
        RunnerCapabilities {
            computer_display_observe: true,
            ..Default::default()
        },
        vec![registered_project("agent-proj", "/tmp/agent-proj")],
    )
    .await;

    let task = tokio::spawn({
        let runtime = runtime.clone();
        let auth = auth.clone();
        async move {
            runtime
                .dispatch_computer_tool(
                    ToolCall::ComputerObserve(ComputerObserveToolCall::SnapshotDisplay {
                        client_id: client_id.to_string(),
                        display_id: DISPLAY_ID.to_string(),
                        max_width: Some(10_000),
                        max_height: Some(u32::MAX),
                    }),
                    Some(&auth),
                )
                .await
        }
    });

    let request = wait_for_runner_request_for_client(&runtime, client_id).await;
    assert_eq!(request.kind, "computer_snapshot_display");
    let payload: Value = serde_json::from_str(request.stdin.as_deref().unwrap()).unwrap();
    assert_eq!(payload["max_width"], 4096);
    assert_eq!(payload["max_height"], 4096);

    let image = [0xff, 0xd8, 0xff, 0xe0];
    runtime
        .runner_registry
        .complete(RunnerResultRequest {
            client_id: client_id.to_string(),
            runner_instance_id: format!("inst-{client_id}"),
            request_id: request.request_id,
            exit_code: Some(0),
            stdout: Some(
                json!({
                    "display_id": DISPLAY_ID,
                    "snapshot_generation": 7,
                    "source_width": 1920,
                    "source_height": 1080,
                    "width": 1920,
                    "height": 1080,
                    "mime_type": "image/jpeg",
                    "file_bytes": image.len(),
                    "sha256": sha256_hex(&image),
                    "captured_at_unix_ms": 1_700_000_000_000u64,
                    "content_base64": general_purpose::STANDARD.encode(image),
                })
                .to_string(),
            ),
            stderr: Some(String::new()),
            stdout_truncated: false,
            stderr_truncated: false,
            duration_ms: Some(1),
            error: None,
        })
        .await
        .unwrap();

    let result = task.await.unwrap();
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.output["width"], 1920);
    assert_eq!(result.output["height"], 1080);
}
