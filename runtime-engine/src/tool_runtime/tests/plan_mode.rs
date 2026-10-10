//! Plan Mode backend enforcement, plan revisions/approval, and Goal admission
//! through the real tool kernel.

use super::super::*;
use super::support::*;
use crate::auth::{AuthContext, AuthKind};
use crate::db::{PlanApprovalBinding, PlanReader, DEFAULT_PLAN_APPROVAL_TTL_MS};
use crate::runner_protocol::RunnerCapabilities;
use crate::tool_runtime::kernel::{ToolCallContext, ToolCallRequest, ToolTransport};
use crate::tool_runtime::plan_mode::{
    with_goal_run_dispatch, PLAN_CONTROL_TOOLS, PLAN_MODE_READ_ONLY_TOOLS, PLAN_OPERATOR_TOOLS,
};
use serde_json::{json, Value};

const ALL_SCOPES: &[&str] = &[
    crate::auth::SCOPE_RUNTIME_READ,
    crate::auth::SCOPE_PROJECT_READ,
    crate::auth::SCOPE_PROJECT_WRITE,
    crate::auth::SCOPE_JOB_RUN,
    crate::auth::SCOPE_COMPUTER_READ,
    crate::auth::SCOPE_COMPUTER_CONTROL,
    crate::auth::SCOPE_CODING_AGENT_RUN,
    crate::auth::SCOPE_MCP_LOCAL,
    crate::auth::SCOPE_PLUGIN_INVOKE,
    crate::auth::SCOPE_SSH_LOCAL,
    crate::auth::SCOPE_MEMORY_READ,
    crate::auth::SCOPE_MEMORY_MANAGE,
];

/// The ChatGPT connector principal (OAuth) for one user.
fn chatgpt(user: &str) -> AuthContext {
    let mut auth = auth_context(Some(user), false);
    auth.kind = AuthKind::OAuth2Token;
    auth.token_kind = Some("oauth2".to_string());
    auth.api_key_id = Some(format!("oauth-token-{user}"));
    auth.scopes = ALL_SCOPES.iter().map(|scope| scope.to_string()).collect();
    auth
}

/// The Chadex app operator (owner user token) for the same user.
fn operator(user: &str) -> AuthContext {
    let mut auth = auth_context(Some(user), false);
    auth.scopes = ALL_SCOPES.iter().map(|scope| scope.to_string()).collect();
    auth
}

struct Fixture {
    _tmp: tempfile::TempDir,
    _root: tempfile::TempDir,
    runtime: ToolRuntime,
    project: String,
    root: String,
}

async fn fixture() -> Fixture {
    let (tmp, db) = crate::test_support::test_db();
    let root = tempfile::tempdir().unwrap();
    let runtime = ToolRuntime::new_for_tests().with_communication_database(db);
    register_agent_with_projects(
        &runtime,
        "client-a",
        Some("alice"),
        RunnerCapabilities {
            shell: true,
            structured_process_argv: true,
            ..Default::default()
        },
        vec![registered_project("demo", &root.path().to_string_lossy())],
    )
    .await;
    let project = crate::tool_runtime::runner_project_runtime_id("client-a", "demo");
    let root_path = runtime
        .resolve_project_for_auth(&project, Some(&chatgpt("alice")))
        .await
        .unwrap()
        .path;
    Fixture {
        _tmp: tmp,
        _root: root,
        runtime,
        project,
        root: root_path,
    }
}

async fn call(
    runtime: &ToolRuntime,
    auth: &AuthContext,
    transport: ToolTransport,
    tool: &str,
    arguments: Value,
) -> (bool, Value) {
    let outcome = runtime
        .call_tool_with_context(
            ToolCallRequest {
                tool_name: tool.to_string(),
                arguments,
            },
            ToolCallContext {
                transport,
                session_id: None,
                auth: Some(auth),
                window: None,
                record_oauth_scope_denials: false,
                host_file_import_trust: Default::default(),
            },
        )
        .await;
    match outcome.result {
        Some(result) => (result.success, result.output),
        None => (false, json!({"error_status": format!("{:?}", outcome.error_status)})),
    }
}

fn error_kind(output: &Value) -> &str {
    output.get("error_kind").and_then(Value::as_str).unwrap_or("")
}

async fn start_plan(fx: &Fixture, auth: &AuthContext) -> String {
    let (ok, output) = call(
        &fx.runtime,
        auth,
        ToolTransport::Mcp,
        "plan_start",
        json!({"project": fx.project, "title": "Add retries"}),
    )
    .await;
    assert!(ok, "{output}");
    assert_eq!(output["plan"]["plan_mode_active"], json!(true));
    output["plan"]["plan_id"].as_str().unwrap().to_string()
}

#[test]
fn allowlist_is_reviewed_read_only_metadata() {
    for name in PLAN_MODE_READ_ONLY_TOOLS {
        let metadata = webcodex_tool_contracts::metadata::lookup_tool_metadata(name)
            .unwrap_or_else(|| panic!("allowlisted tool {name} does not exist"));
        assert_eq!(
            metadata.effect,
            webcodex_tool_contracts::metadata::ToolEffect::Observe,
            "{name} must be an observation"
        );
    }
    for name in PLAN_CONTROL_TOOLS.iter().chain(PLAN_OPERATOR_TOOLS) {
        assert!(
            webcodex_tool_contracts::metadata::lookup_tool_metadata(name).is_some(),
            "{name}"
        );
    }
}

#[tokio::test]
async fn plan_mode_denies_every_other_tool_for_the_locked_principal_only() {
    let fx = fixture().await;
    // `admin` passes every scope check, so each call reaches the Plan Mode
    // gate; it also shows that admin authority does not bypass Plan Mode.
    let mut alice = chatgpt("alice");
    alice.scopes.push(crate::auth::SCOPE_ADMIN.to_string());
    start_plan(&fx, &alice).await;

    let mut denied = 0;
    for definition in crate::tool_runtime::tool_definition::tool_definitions() {
        let name = definition.name;
        if PLAN_MODE_READ_ONLY_TOOLS.contains(&name)
            || PLAN_CONTROL_TOOLS.contains(&name)
            || PLAN_OPERATOR_TOOLS.contains(&name)
        {
            continue;
        }
        let (ok, output) = call(
            &fx.runtime,
            &alice,
            ToolTransport::Mcp,
            name,
            json!({"project": fx.project}),
        )
        .await;
        assert!(!ok, "{name} must be refused in Plan Mode");
        assert_eq!(error_kind(&output), "plan_mode_read_only", "{name}: {output}");
        denied += 1;
    }
    assert!(denied > 80, "only {denied} tools checked");
    // Tools the MCP adapter dispatches outside the kernel.
    for name in [
        crate::mcp_gateway::MCP_TOOL_NAME,
        crate::plugin_gateway::PLUGIN_TOOL_NAME,
        crate::ssh_resource_gateway::SSH_RESOURCE_TOOL_NAME,
        "call_runtime_tool",
        "some_future_tool",
    ] {
        let result = fx.runtime.plan_mode_denial(Some(&alice), name, None).expect(name);
        assert_eq!(error_kind(&result.output), "plan_mode_read_only", "{name}");
    }

    // The same user's Chadex app operator principal and other users are not
    // affected by Alice's ChatGPT Plan Mode.
    for other in [operator("alice"), chatgpt("bob")] {
        for name in ["write_project_file", "run_process", "computer_control"] {
            assert!(
                fx.runtime.plan_mode_denial(Some(&other), name, Some(&fx.project)).is_none(),
                "{name} wrongly blocked for an unrelated principal"
            );
        }
    }
}

#[tokio::test]
async fn read_only_tools_stay_available_but_only_for_the_plan_project() {
    let fx = fixture().await;
    let alice = chatgpt("alice");
    start_plan(&fx, &alice).await;
    let (_, output) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "read_files",
        json!({"project": fx.project, "items": [{"path": "README.md"}]}),
    )
    .await;
    assert_ne!(error_kind(&output), "plan_mode_read_only", "{output}");
    let (_, output) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "read_files",
        json!({"project": "agent:client-a:other", "items": [{"path": "README.md"}]}),
    )
    .await;
    assert_eq!(error_kind(&output), "plan_mode_read_only", "{output}");
}

#[tokio::test]
async fn operator_review_is_unreachable_from_mcp_and_from_the_chatgpt_principal() {
    let fx = fixture().await;
    let alice = chatgpt("alice");
    let plan_id = start_plan(&fx, &alice).await;
    let review = json!({"action": "close", "plan_id": plan_id});
    // Over MCP, even the operator credential is refused.
    let (ok, output) = call(&fx.runtime, &operator("alice"), ToolTransport::Mcp, "plan_review", review.clone()).await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "operator_only");
    // Over operator REST, the ChatGPT (OAuth) principal is refused.
    let (ok, output) = call(&fx.runtime, &alice, ToolTransport::Api, "plan_review", review.clone()).await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "operator_only");
    for (tool, args) in [
        ("goal_operator", json!({"action": "emergency_stop"})),
        ("goal_operator", json!({"action": "release_emergency_stop"})),
    ] {
        let (ok, output) = call(&fx.runtime, &alice, ToolTransport::Mcp, tool, args).await;
        assert!(!ok);
        assert_eq!(error_kind(&output), "operator_only");
    }
    // Plan Mode is still active after all those attempts.
    assert!(fx.runtime.plan_mode_denial(Some(&alice), "write_project_file", None).is_some());
    // The local operator over REST can exit Plan Mode.
    let (ok, output) = call(&fx.runtime, &operator("alice"), ToolTransport::Api, "plan_review", review).await;
    assert!(ok, "{output}");
    assert!(fx.runtime.plan_mode_denial(Some(&alice), "write_project_file", None).is_none());
}

#[tokio::test]
async fn plan_revision_approval_and_goal_admission_through_the_kernel() {
    let fx = fixture().await;
    let alice = chatgpt("alice");
    let plan_id = start_plan(&fx, &alice).await;
    let acceptance = json!([{"name": "tests", "argv": ["cargo", "test"]}]);
    let (ok, v1) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "plan_save",
        json!({"plan_id": plan_id, "parent_revision": 0, "content": "step one", "acceptance": acceptance}),
    )
    .await;
    assert!(ok, "{v1}");
    let (ok, v2) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "plan_save",
        json!({"plan_id": plan_id, "parent_revision": 1, "content": "step one\nstep two"}),
    )
    .await;
    assert!(ok, "{v2}");
    assert_eq!(v2["diff_from_previous"]["lines"], json!([" step one", "+step two"]));
    // Acceptance is carried over when omitted.
    let (_, detail) = call(&fx.runtime, &alice, ToolTransport::Mcp, "plan_get", json!({"plan_id": plan_id, "diff_from": 1})).await;
    assert_eq!(detail["revision"]["spec"]["acceptance"][0]["name"], json!("tests"));
    assert_eq!(detail["diff"]["changed_lines"], json!(1));

    // While Plan Mode is active, Goal start is itself refused by the read-only gate.
    let (ok, output) = call(&fx.runtime, &alice, ToolTransport::Mcp, "goal_run_start", json!({"plan_id": plan_id})).await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "plan_mode_read_only");

    // The model cannot approve; the operator approves revision 2 (store path,
    // because this fake Runner does not execute `git rev-parse`).
    let db = fx.runtime.communication_db.clone().unwrap();
    db.plan_approve(
        &plan_id,
        2,
        v2["revision"]["content_sha256"].as_str().unwrap(),
        PlanApprovalBinding {
            project_id: fx.project.clone(),
            runner_client_id: "client-a".to_string(),
            project_root: fx.root.clone(),
            base_commit: "abc".to_string(),
        },
        "operator:alice",
        DEFAULT_PLAN_APPROVAL_TTL_MS,
        crate::tool_runtime::plan_mode::now_unix_ms(),
    )
    .unwrap();
    fx.runtime.plan_guard.reload();
    // Approval released Plan Mode for Alice.
    assert!(fx.runtime.plan_mode_denial(Some(&alice), "run_process", Some(&fx.project)).is_none());

    // Rejecting keeps it unapproved; the operator closing nothing yet, so a
    // Goal start without approval after leaving Plan Mode is refused too.
    // (Covered at store level; here: the approval exists, continue.)
    // Another principal cannot see or start Alice's plan.
    let (ok, output) = call(&fx.runtime, &chatgpt("bob"), ToolTransport::Mcp, "goal_run_start", json!({"plan_id": plan_id})).await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "plan_not_found");

    // Missing execution scope: Goal start is refused before the approval is consumed.
    let mut read_only = chatgpt("alice");
    read_only.scopes.retain(|scope| scope != crate::auth::SCOPE_JOB_RUN);
    let (ok, _) = call(&fx.runtime, &read_only, ToolTransport::Mcp, "goal_run_start", json!({"plan_id": plan_id})).await;
    assert!(!ok);

    let (ok, started) = call(&fx.runtime, &alice, ToolTransport::Mcp, "goal_run_start", json!({"plan_id": plan_id})).await;
    assert!(ok, "{started}");
    assert_eq!(started["created"], json!(true));
    assert_eq!(started["goal_run"]["revision"], json!(2));
    let run_id = started["goal_run"]["goal_run_id"].as_str().unwrap().to_string();
    // Duplicate start replays the same run.
    let (ok, again) = call(&fx.runtime, &alice, ToolTransport::Mcp, "goal_run_start", json!({"plan_id": plan_id})).await;
    assert!(ok);
    assert_eq!(again["created"], json!(false));
    assert_eq!(again["goal_run"]["goal_run_id"], json!(run_id));
    let (ok, status) = call(&fx.runtime, &alice, ToolTransport::Mcp, "goal_run_status", json!({"goal_run_id": run_id})).await;
    assert!(ok, "{status}");
    assert_eq!(status["goal_run"]["state"], json!("queued"));
    assert_eq!(status["goal_run"]["cost"], json!("unknown"));
    // The run's stored dispatch principal is restricted to Goal scopes.
    let record = db.goal_run_read(PlanReader::Operator, &run_id).unwrap().run;
    let scopes: Vec<_> = record.dispatch_auth["scopes"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(scopes.contains(&"job:run") && scopes.contains(&"project:write"));
    assert!(!scopes.iter().any(|scope| scope.starts_with("computer") || *scope == "mcp:local" || *scope == "admin"));
}

#[tokio::test]
async fn goal_dispatch_scope_only_reaches_process_and_job_tools() {
    let fx = fixture().await;
    let alice = chatgpt("alice");
    with_goal_run_dispatch("wc_grun_test".to_string(), async {
        for name in ["write_project_file", "computer_control", "coding_agent_start", "mcp_tool", "plan_review"] {
            let denied = fx.runtime.plan_mode_denial(Some(&alice), name, None).expect(name);
            assert_eq!(error_kind(&denied.output), "goal_dispatch_tool_not_allowed", "{name}");
        }
        for name in ["run_process", "observe_jobs", "stop_job"] {
            assert!(fx.runtime.plan_mode_denial(Some(&alice), name, None).is_none(), "{name}");
        }
    })
    .await;
    // Outside the supervisor task the restriction does not apply.
    assert!(fx.runtime.plan_mode_denial(Some(&alice), "write_project_file", None).is_none());
}

/// The local ChatGPT connector reaches the runtime through the tunnel with
/// the runtime bootstrap credential (see rust-helper TunnelSession).
fn chatgpt_via_tunnel() -> AuthContext {
    auth_context(None, true)
}

#[tokio::test]
async fn tunnel_bootstrap_principal_is_locked_cannot_approve_and_goal_dispatch_is_owner_restricted() {
    let fx = fixture().await;
    let tunnel = chatgpt_via_tunnel();
    let plan_id = start_plan(&fx, &tunnel).await;
    // Bootstrap/admin authority does not bypass Plan Mode.
    let (ok, output) = call(&fx.runtime, &tunnel, ToolTransport::Mcp, "write_project_file", json!({"project": fx.project, "path": "x", "content": "y"})).await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "plan_mode_read_only");
    // Nor can it approve, over either transport.
    for transport in [ToolTransport::Mcp, ToolTransport::Api] {
        let (ok, output) = call(
            &fx.runtime,
            &tunnel,
            transport,
            "plan_review",
            json!({"action": "close", "plan_id": plan_id}),
        )
        .await;
        assert!(!ok);
        assert_eq!(error_kind(&output), "operator_only");
    }
    let (ok, saved) = call(
        &fx.runtime,
        &tunnel,
        ToolTransport::Mcp,
        "plan_save",
        json!({"plan_id": plan_id, "parent_revision": 0, "content": "plan", "acceptance": [{"name": "t", "argv": ["true"]}]}),
    )
    .await;
    assert!(ok, "{saved}");
    let db = fx.runtime.communication_db.clone().unwrap();
    db.plan_approve(
        &plan_id,
        1,
        saved["revision"]["content_sha256"].as_str().unwrap(),
        PlanApprovalBinding {
            project_id: fx.project.clone(),
            runner_client_id: "client-a".to_string(),
            project_root: fx.root.clone(),
            base_commit: "abc".to_string(),
        },
        "operator:alice",
        DEFAULT_PLAN_APPROVAL_TTL_MS,
        crate::tool_runtime::plan_mode::now_unix_ms(),
    )
    .unwrap();
    fx.runtime.plan_guard.reload();
    let (ok, started) = call(&fx.runtime, &tunnel, ToolTransport::Mcp, "goal_run_start", json!({"plan_id": plan_id})).await;
    assert!(ok, "{started}");
    let run_id = started["goal_run"]["goal_run_id"].as_str().unwrap();
    let record = db.goal_run_read(PlanReader::Operator, run_id).unwrap().run;
    let dispatch = &record.dispatch_auth;
    assert_eq!(dispatch["kind"], json!("api_token"));
    assert_eq!(dispatch["token_kind"], json!("goal_run"));
    assert_eq!(dispatch["username"], json!("alice"), "Runner owner, not bootstrap");
    let restored = crate::tool_runtime::plan_mode::goal_dispatch_auth(dispatch).unwrap().unwrap();
    assert!(!restored.is_bootstrap);
    assert!(!restored.is_admin());
    assert!(restored.has_scope(crate::auth::SCOPE_JOB_RUN));
    assert!(!restored.has_scope(crate::auth::SCOPE_COMPUTER_CONTROL));
}
