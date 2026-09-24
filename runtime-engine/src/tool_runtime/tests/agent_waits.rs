use crate::tool_runtime::{AgentWaitEventSelectorCall, ToolRuntime};
use std::sync::Arc;

fn assert_sparse_wait(
    tool: &str,
    arguments: serde_json::Value,
    canonical: &super::super::ToolResult,
) {
    let call = super::super::ToolCall::from_tool_name(tool, arguments).unwrap();
    let mut model = super::super::ToolResult {
        success: canonical.success,
        output: canonical.output.clone(),
        error: canonical.error.clone(),
    };
    super::super::dispatch::ModelFacingProjectionPlan::capture(&call).project(&mut model);
    let durable = &canonical.output["agent_wait"];
    let wait = &model.output["agent_wait"];
    assert_eq!(wait["wait_id"], durable["wait_id"]);
    assert_eq!(wait["state"], durable["state"]);
    assert_eq!(model.output["replayed"], canonical.output["replayed"]);
    assert_eq!(
        model.output["state_changed"],
        canonical.output["state_changed"]
    );
    assert_eq!(
        model.output["agent_continuation"],
        canonical.output["agent_continuation"]
    );
    for key in [
        "target_agent_id",
        "revision",
        "created_at_unix_ms",
        "updated_at_unix_ms",
        "triggered_at_unix_ms",
        "resumed_at_unix_ms",
        "cancelled_at_unix_ms",
        "source_count",
        "match_count",
        "match_sequence",
        "sources",
    ] {
        assert!(
            durable.get(key).is_some(),
            "durable {key} must remain intact"
        );
        assert!(wait.get(key).is_none(), "model must not receive {key}");
    }
    if durable["matches"].as_array().unwrap().is_empty() {
        assert_eq!(wait.as_object().unwrap().len(), 2);
    } else {
        for (matched, original) in wait["matches"]
            .as_array()
            .unwrap()
            .iter()
            .zip(durable["matches"].as_array().unwrap())
        {
            assert_eq!(matched.as_object().unwrap().len(), 3);
            for key in ["task_id", "task_attempt_id", "terminal_task_state"] {
                assert_eq!(matched[key], original[key]);
            }
        }
    }
    let schema = super::super::registry::output_schema_for_tool(tool);
    super::super::startup_brief::validate_schema_instance_for_test(
        &serde_json::to_value(model).unwrap(),
        &schema,
    )
    .unwrap();
}

fn runtime_with_db() -> (tempfile::TempDir, Arc<crate::db::Database>, ToolRuntime) {
    let temp = tempfile::tempdir().unwrap();
    let db = Arc::new(crate::db::Database::open(&temp.path().join("agent-waits.db")).unwrap());
    let runtime = ToolRuntime::new_for_tests().with_communication_database(db.clone());
    (temp, db, runtime)
}

fn create_agent(runtime: &ToolRuntime, handle: &str) -> String {
    let result = runtime.create_agent_identity(
        None,
        handle.to_string(),
        format!("{handle} display"),
        None,
        Vec::new(),
        format!("create-{handle}"),
    );
    assert!(result.success, "{:?}", result.output);
    result.output["agent"]["agent_id"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn wait_runtime_surface_returns_exact_wait_and_existing_continuation_projection() {
    let (_temp, _db, runtime) = runtime_with_db();
    let watcher = create_agent(&runtime, "wait-runtime-watcher");
    let worker = create_agent(&runtime, "wait-runtime-worker");
    let endpoint = runtime.attach_agent_endpoint(
        None,
        watcher.clone(),
        "ChatGPT".to_string(),
        Some("wait-runtime-view".to_string()),
        "wait-runtime-endpoint".to_string(),
    );
    assert!(endpoint.success, "{:?}", endpoint.output);
    let endpoint_id = endpoint.output["endpoint"]["endpoint_id"]
        .as_str()
        .unwrap()
        .to_string();
    let generation = endpoint.output["endpoint"]["controller_generation"]
        .as_i64()
        .unwrap();

    let created_task = runtime.create_agent_task(
        None,
        "Wait runtime source".to_string(),
        "PRIVATE source instruction".to_string(),
        Some(worker.clone()),
        None,
        None,
        Some("agent:special:private-source-project".to_string()),
        "wait-runtime-task".to_string(),
    );
    assert!(created_task.success, "{:?}", created_task.output);
    let task_id = created_task.output["task"]["summary"]["task_id"]
        .as_str()
        .unwrap()
        .to_string();
    let started = runtime.start_agent_task_attempt(
        None,
        task_id.clone(),
        worker.clone(),
        "wait-runtime-attempt".to_string(),
    );
    assert!(started.success, "{:?}", started.output);
    let completed = runtime.complete_agent_task_attempt(
        None,
        task_id.clone(),
        started.output["attempt"]["attempt_id"]
            .as_str()
            .unwrap()
            .to_string(),
        worker,
        started.output["attempt_fence"]
            .as_str()
            .unwrap()
            .to_string(),
        started.output["attempt"]["attempt_controller_generation"]
            .as_i64()
            .unwrap(),
        "succeeded".to_string(),
        Some("PRIVATE terminal result".to_string()),
        Some("PRIVATE terminal reason".to_string()),
        "wait-runtime-complete".to_string(),
    );
    assert!(completed.success, "{:?}", completed.output);

    let waited = runtime.wait_for_agent_events(
        None,
        watcher.clone(),
        endpoint_id.clone(),
        generation,
        vec![AgentWaitEventSelectorCall {
            kind: "agent_task_terminal".to_string(),
            task_id: task_id.clone(),
        }],
        "wait-runtime-create".to_string(),
    );
    assert!(waited.success, "{:?}", waited.output);
    assert_eq!(waited.output["agent_wait"]["state"], "triggered");
    assert_eq!(waited.output["agent_wait"]["match_count"], 1);
    assert_eq!(
        waited.output["agent_wait"]["matches"][0]["task_id"],
        task_id
    );
    let wait_id = waited.output["agent_wait"]["wait_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        waited.output["agent_continuation"]["wake"]["wait_id"],
        wait_id
    );
    assert_eq!(
        waited.output["agent_continuation"]["wake"]["wait_match_count"],
        1
    );
    assert_sparse_wait(
        "wait_for_agent_events",
        serde_json::json!({
            "agent_id": watcher, "endpoint_id": endpoint_id, "expected_controller_generation": generation,
            "events": [{"kind": "agent_task_terminal", "task_id": task_id}], "idempotency_key": "wait-runtime-create"
        }),
        &waited,
    );
    let serialized = waited.output.to_string();
    for private in [
        "PRIVATE source instruction",
        "PRIVATE terminal result",
        "PRIVATE terminal reason",
        "private-source-project",
        "attempt_fence",
        "consume_token",
    ] {
        assert!(
            !serialized.contains(private),
            "Wait output leaked {private}"
        );
    }

    let read = runtime.read_agent_wait(None, wait_id.clone());
    assert!(read.success, "{:?}", read.output);
    assert_eq!(read.output["agent_wait"]["state"], "triggered");
    let app_read = runtime.agent_wait_state(None, wait_id.clone());
    assert!(app_read.success, "{:?}", app_read.output);
    assert_eq!(app_read.output, read.output);
    assert_sparse_wait(
        "read_agent_wait",
        serde_json::json!({"wait_id": wait_id}),
        &read,
    );

    let cancelled =
        runtime.cancel_agent_wait(None, wait_id.clone(), "wait-runtime-cancel".to_string());
    assert!(cancelled.success, "{:?}", cancelled.output);
    assert_eq!(cancelled.output["agent_wait"]["state"], "cancelled");
    assert_sparse_wait(
        "cancel_agent_wait",
        serde_json::json!({"wait_id": wait_id, "idempotency_key": "wait-runtime-cancel"}),
        &cancelled,
    );
    let replay = runtime.cancel_agent_wait(None, wait_id, "wait-runtime-cancel".to_string());
    assert!(replay.success, "{:?}", replay.output);
    assert_eq!(replay.output["replayed"], true);
    assert_eq!(replay.output["state_changed"], false);
}

#[test]
fn wait_tool_contracts_are_definition_owned_and_hidden_state_stays_app_only() {
    let specs = crate::tool_runtime::registered_tool_specs();
    for name in [
        "wait_for_agent_events",
        "read_agent_wait",
        "cancel_agent_wait",
    ] {
        let spec = specs.iter().find(|spec| spec.name == name).unwrap();
        assert!(spec.description.contains("AgentWait") || spec.description.contains("Agent Wait"));
    }
    assert!(
        specs.iter().all(|spec| spec.name != "agent_wait_state"),
        "App-only Wait polling must stay hidden from the ordinary model-visible registry"
    );
}

#[tokio::test]
async fn agent_wait_dispatch_is_sparse_while_app_and_durable_read_remain_complete() {
    use super::super::{ToolCall, ToolResult};
    use serde_json::json;
    let (_temp, _db, runtime) = runtime_with_db();
    let auth = super::support::auth_context(None, true);
    let created_agent = runtime.create_agent_identity(
        Some(&auth),
        "sparse-wait-agent".into(),
        "Sparse Wait".into(),
        None,
        Vec::new(),
        "sparse-agent".into(),
    );
    assert!(created_agent.success);
    let agent = created_agent.output["agent"]["agent_id"]
        .as_str()
        .unwrap()
        .to_string();
    let endpoint = runtime.attach_agent_endpoint(
        Some(&auth),
        agent.clone(),
        "test".into(),
        None,
        "sparse-endpoint".into(),
    );
    let task = runtime.create_agent_task(
        Some(&auth),
        "source".into(),
        "instruction".into(),
        Some(agent.clone()),
        None,
        None,
        None,
        "sparse-source".into(),
    );
    assert!(endpoint.success && task.success);
    let arguments = json!({
        "agent_id": agent,
        "endpoint_id": endpoint.output["endpoint"]["endpoint_id"],
        "expected_controller_generation": endpoint.output["endpoint"]["controller_generation"],
        "events": [{"kind": "agent_task_terminal", "task_id": task.output["task"]["summary"]["task_id"]}],
        "idempotency_key": "sparse-create"
    });
    let call = || ToolCall::from_tool_name("wait_for_agent_events", arguments.clone()).unwrap();
    let created = runtime.dispatch_with_auth(call(), Some(&auth)).await;
    assert!(created.success, "{:?}", created);
    let wait_id = created.output["agent_wait"]["wait_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        created.output["agent_wait"],
        json!({"wait_id": wait_id, "state": "waiting"})
    );
    assert_eq!(created.output["replayed"], false);
    assert_eq!(created.output["state_changed"], true);
    let replay = runtime.dispatch_with_auth(call(), Some(&auth)).await;
    assert_eq!(replay.output["agent_wait"], created.output["agent_wait"]);
    assert_eq!(replay.output["replayed"], true);
    assert_eq!(replay.output["state_changed"], false);
    let durable = runtime.read_agent_wait(Some(&auth), wait_id.clone());
    assert_sparse_wait("read_agent_wait", json!({"wait_id": wait_id}), &durable);
    let app = runtime
        .dispatch_with_auth(
            ToolCall::AgentWaitState {
                wait_id: wait_id.clone(),
            },
            Some(&auth),
        )
        .await;
    assert_eq!(app.output, durable.output);
    assert_eq!(app.output["agent_wait"]["target_agent_id"], agent);
    assert_eq!(app.output["agent_wait"]["revision"], 1);
    let audit =
        super::super::tool_audit::session_log_result_for_tool("read_agent_wait", &durable.output);
    assert_eq!(audit["revision"], 1);
    assert_eq!(audit["match_count"], 0);
    let cancelled = runtime
        .dispatch_with_auth(
            ToolCall::CancelAgentWait {
                wait_id: wait_id.clone(),
                idempotency_key: "sparse-cancel".into(),
            },
            Some(&auth),
        )
        .await;
    assert!(cancelled.success);
    assert_eq!(
        cancelled.output["agent_wait"],
        json!({"wait_id": wait_id, "state": "cancelled"})
    );
    assert_eq!(cancelled.output["state_changed"], true);
    let mut failure = ToolResult::err("unknown Wait");
    let before = serde_json::to_value(&failure).unwrap();
    super::super::agent_wait::agent_wait_model_projection(&mut failure);
    assert_eq!(serde_json::to_value(&failure).unwrap(), before);
}
