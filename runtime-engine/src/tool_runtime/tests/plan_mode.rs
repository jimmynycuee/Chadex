//! Plan Mode backend enforcement, plan revisions/approval, and Goal admission
//! through the real tool kernel.

use super::super::*;
use super::support::*;
use crate::auth::{AuthContext, AuthKind};
use crate::db::{PlanApprovalBinding, PlanReader, DEFAULT_PLAN_APPROVAL_TTL_MS};
use crate::runner_protocol::{
    RunnerCapabilities, RunnerResultPayload, RunnerResultRequest, ShellCommandExecutionState,
};
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

/// Service Git requests through the fake Runner while a Plan/Goal tool call
/// re-enters the kernel for read-only repository checks.
async fn call_with_fake_runner(
    fx: &Fixture,
    auth: &AuthContext,
    transport: ToolTransport,
    tool: &str,
    arguments: Value,
) -> (bool, Value) {
    let runtime = fx.runtime.clone();
    let auth = auth.clone();
    let tool = tool.to_string();
    let task = tokio::spawn(async move {
        call(&runtime, &auth, transport, &tool, arguments).await
    });
    loop {
        if task.is_finished() {
            return task.await.unwrap();
        }
        if let Some(request) = probe_patch_agent_request(&fx.runtime, "client-a").await {
            let (exit_code, stdout, stderr) = run_runner_shell_request_locally(&request);
            fx.runtime
                .runner_registry
                .complete(RunnerResultPayload {
                    result: RunnerResultRequest {
                        client_id: "client-a".to_string(),
                        runner_instance_id: "inst".to_string(),
                        request_id: request.request_id,
                        exit_code: Some(exit_code),
                        stdout: Some(stdout),
                        stderr: Some(stderr),
                        stdout_truncated: false,
                        stderr_truncated: false,
                        duration_ms: Some(1),
                        error: None,
                    },
                    command_execution_state: Some(ShellCommandExecutionState::Completed),
                    mcp_gateway: None,
                    plugin_gateway: None,
                    coding_agent: None,
                })
                .await
                .unwrap();
        } else {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }
    }
}

/// What the Chadex app's Start does after approving (its one-step path also
/// needs a Runner that executes git): start the run as the plan's owner with
/// the principal recorded for the latest revision.
fn operator_start(fx: &Fixture, plan_id: &str, provider: &str) -> Result<crate::db::GoalRunRecord, crate::db::PlanStoreError> {
    let db = fx.runtime.communication_db.clone().unwrap();
    let now = crate::tool_runtime::plan_mode::now_unix_ms();
    let context = db.plan_start_context(plan_id, now).unwrap();
    db.goal_run_start(
        &context.owner_key,
        plan_id,
        &crate::db::GoalStartEnvironment {
            runner_client_id: "client-a".to_string(),
            project_root: fx.root.clone(),
        },
        &context.dispatch.expect("principal recorded for the latest revision"),
        provider,
        now,
    )
    .map(|outcome| outcome.run)
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
async fn goal_operator_clear_board_passes_optional_project_scope_through_the_kernel() {
    let demo = crate::tool_runtime::runner_project_runtime_id("client-a", "demo");
    let fx = app_fixture(Some(&demo), true).await;
    let db = fx.runtime.communication_db.clone().unwrap();
    let now = crate::tool_runtime::plan_mode::now_unix_ms();
    let scoped = db
        .plan_start("seeded-scoped-owner", &fx.demo, "Scoped board", now)
        .unwrap()
        .plan;
    let other = db
        .plan_start("seeded-other-owner", &fx.other, "Other board", now)
        .unwrap()
        .plan;

    let (ok, cleared) = call(
        &fx.runtime,
        &operator("alice"),
        ToolTransport::Api,
        "goal_operator",
        json!({"action": "clear_board", "project": fx.demo.clone()}),
    )
    .await;
    assert!(ok, "{cleared}");
    assert_eq!(cleared["closed_plans"], json!(1));
    assert_eq!(
        db.plan_read(PlanReader::Operator, &scoped.plan_id, None, now)
            .unwrap()
            .plan
            .state,
        crate::db::PlanState::Closed
    );
    assert_eq!(
        db.plan_read(PlanReader::Operator, &other.plan_id, None, now)
            .unwrap()
            .plan
            .state,
        crate::db::PlanState::Drafting,
        "a project-scoped clear must leave another project's open Plan untouched"
    );

    let (ok, all_cleared) = call(
        &fx.runtime,
        &operator("alice"),
        ToolTransport::Api,
        "goal_operator",
        json!({"action": "clear_board"}),
    )
    .await;
    assert!(ok, "{all_cleared}");
    assert_eq!(all_cleared["closed_plans"], json!(1));
    assert_eq!(
        db.plan_read(PlanReader::Operator, &other.plan_id, None, now)
            .unwrap()
            .plan
            .state,
        crate::db::PlanState::Closed,
        "omitting project keeps the all-project cleanup behavior"
    );
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

    // Even with an approval, the model cannot start a Goal itself; a Plan
    // Mode plan is pointed back at plan_wait_approval, not at Start Goal.
    let (ok, output) = call(&fx.runtime, &alice, ToolTransport::Mcp, "goal_run_start", json!({"plan_id": plan_id})).await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "not_a_goal");
    assert_eq!(output["next_tool"], json!("plan_wait_approval"));
    let started = operator_start(&fx, &plan_id, "codex").unwrap();
    assert_eq!(started.revision, 2);
    let run_id = started.goal_run_id.clone();
    // After the app started it, the model finds that run.
    let (ok, again) = call(&fx.runtime, &alice, ToolTransport::Mcp, "goal_run_start", json!({"plan_id": plan_id})).await;
    assert!(ok, "{again}");
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
    let run_id = operator_start(&fx, &plan_id, "codex").unwrap().goal_run_id;
    let record = db.goal_run_read(PlanReader::Operator, &run_id).unwrap().run;
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

#[tokio::test]
async fn review_m4_plan_mode_blocks_resuming_goals() {
    let fx = fixture().await;
    let alice = chatgpt("alice");
    start_plan(&fx, &alice).await;
    let (ok, output) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "goal_run_control",
        json!({"goal_run_id": "wc_grun_AAAAAAAAAAAAAAAA", "action": "resume"}),
    )
    .await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "plan_mode_read_only");
}

#[tokio::test]
async fn direct_goal_request_waits_for_the_operator_and_never_engages_plan_mode() {
    let fx = fixture().await;
    let tunnel = chatgpt_via_tunnel();
    let request = json!({
        "project": fx.project,
        "title": "Fix add",
        "objective": "add() must add; keep the API",
        "acceptance": [{"name": "unit", "argv": ["python3", "-m", "unittest"]}],
    });
    // Mixing the two forms, or omitting acceptance, is refused.
    let mut mixed = request.clone();
    mixed["plan_id"] = json!("wc_plan_aaaaaaaaaaaa");
    let (ok, output) = call(&fx.runtime, &tunnel, ToolTransport::Mcp, "goal_run_start", mixed).await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "invalid_arguments");
    let mut no_checks = request.clone();
    no_checks.as_object_mut().unwrap().remove("acceptance");
    let (ok, output) = call(&fx.runtime, &tunnel, ToolTransport::Mcp, "goal_run_start", no_checks).await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "plan_missing_acceptance");

    let (ok, created) = call(&fx.runtime, &tunnel, ToolTransport::Mcp, "goal_run_start", request.clone()).await;
    assert!(ok, "{created}");
    assert_eq!(created["state"], json!("awaiting_start"));
    assert_eq!(created["plan"]["origin"], json!("goal"));
    assert_eq!(created["plan"]["plan_mode_active"], json!(false));
    let plan_id = created["plan"]["plan_id"].as_str().unwrap().to_string();
    // The connection stays fully usable: no read-only lock.
    assert!(fx.runtime.plan_mode_denial(Some(&tunnel), "run_process", Some(&fx.project)).is_none());
    // The model can neither start it nor use the operator's one-step start.
    let (ok, output) = call(&fx.runtime, &tunnel, ToolTransport::Mcp, "goal_run_start", json!({"plan_id": plan_id})).await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "goal_not_started");
    for transport in [ToolTransport::Mcp, ToolTransport::Api] {
        let (ok, output) = call(
            &fx.runtime,
            &tunnel,
            transport,
            "plan_review",
            json!({"action": "approve_and_start", "plan_id": plan_id, "revision": 1, "content_sha256": created["revision"]["content_sha256"]}),
        )
        .await;
        assert!(!ok);
        assert_eq!(error_kind(&output), "operator_only");
    }
    // The recorded dispatch principal is the restricted Runner-owner one.
    let db = fx.runtime.communication_db.clone().unwrap();
    let dispatch = db
        .plan_start_context(&plan_id, crate::tool_runtime::plan_mode::now_unix_ms())
        .unwrap()
        .dispatch
        .unwrap();
    assert_eq!(dispatch["token_kind"], json!("goal_run"));
    assert_eq!(dispatch["username"], json!("alice"));

    // After the app approves and starts it, the model finds the run.
    db.plan_approve(
        &plan_id,
        1,
        created["revision"]["content_sha256"].as_str().unwrap(),
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
    let run_id = operator_start(&fx, &plan_id, "codex").unwrap().goal_run_id;
    let (ok, started) = call(&fx.runtime, &tunnel, ToolTransport::Mcp, "goal_run_start", json!({"plan_id": plan_id})).await;
    assert!(ok, "{started}");
    assert_eq!(started["goal_run"]["goal_run_id"], json!(run_id));

    // Status wait returns as soon as the run settles.
    let (ok, _) = call(&fx.runtime, &tunnel, ToolTransport::Mcp, "goal_run_control", json!({"goal_run_id": run_id, "action": "cancel"})).await;
    assert!(ok);
    let begin = std::time::Instant::now();
    let (ok, status) = call(&fx.runtime, &tunnel, ToolTransport::Mcp, "goal_run_status", json!({"goal_run_id": run_id, "wait_secs": 45})).await;
    assert!(ok, "{status}");
    assert_eq!(status["goal_run"]["state"], json!("cancelled"));
    assert!(begin.elapsed() < std::time::Duration::from_secs(5));
}

#[tokio::test]
async fn direct_goal_request_is_blocked_while_plan_mode_is_active() {
    let fx = fixture().await;
    let alice = chatgpt("alice");
    let _plan = start_plan(&fx, &alice).await;
    let (ok, output) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "goal_run_start",
        json!({"project": fx.project, "title": "x", "objective": "y", "acceptance": [{"name": "t", "argv": ["true"]}]}),
    )
    .await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "plan_mode_read_only");
}

#[tokio::test]
async fn plan_save_records_the_goal_principal_for_one_step_start() {
    let fx = fixture().await;
    let alice = chatgpt("alice");
    let plan_id = start_plan(&fx, &alice).await;
    let (ok, saved) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "plan_save",
        json!({"plan_id": plan_id, "parent_revision": 0, "content": "plan", "acceptance": [{"name": "t", "argv": ["true"]}]}),
    )
    .await;
    assert!(ok, "{saved}");
    let db = fx.runtime.communication_db.clone().unwrap();
    let context = db
        .plan_start_context(&plan_id, crate::tool_runtime::plan_mode::now_unix_ms())
        .unwrap();
    assert_eq!(context.owner_key, crate::tool_runtime::plan_mode::plan_principal_key(Some(&alice)));
    let dispatch = context.dispatch.unwrap();
    let scopes: Vec<_> = dispatch["scopes"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(scopes.contains(&"job:run"));
    assert!(!scopes.iter().any(|scope| scope.starts_with("computer")));
}

#[tokio::test]
async fn requested_changes_are_returned_to_the_model_and_steer_blocked_writes_to_plan_save() {
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
    assert_eq!(v1["addressed_change_request"], Value::Null);
    let (_, detail) = call(&fx.runtime, &alice, ToolTransport::Mcp, "plan_get", json!({"plan_id": plan_id})).await;
    assert_eq!(detail["pending_change_request"], Value::Null);

    let db = fx.runtime.communication_db.clone().unwrap();
    db.plan_reject(
        &plan_id,
        1,
        v1["revision"]["content_sha256"].as_str().unwrap(),
        "operator:alice",
        "Add a rollback step",
        crate::tool_runtime::plan_mode::now_unix_ms(),
    )
    .unwrap();
    fx.runtime.plan_guard.reload();

    let (ok, detail) = call(&fx.runtime, &alice, ToolTransport::Mcp, "plan_get", json!({"plan_id": plan_id})).await;
    assert!(ok, "{detail}");
    assert_eq!(detail["pending_change_request"]["feedback"], json!("Add a rollback step"));
    assert_eq!(detail["pending_change_request"]["revision"], json!(1));

    // Re-entering Plan Mode shows the feedback too.
    let (ok, started) = call(&fx.runtime, &alice, ToolTransport::Mcp, "plan_start", json!({"project": fx.project, "title": "Add retries"})).await;
    assert!(ok, "{started}");
    assert_eq!(started["pending_change_request"]["feedback"], json!("Add a rollback step"));

    // A blocked write points at plan_save with the feedback, not at unlocking.
    let denied = fx
        .runtime
        .plan_mode_denial(Some(&alice), "run_process", Some(&fx.project))
        .expect("still in Plan Mode");
    let message = denied.error.clone().unwrap_or_default();
    assert!(message.contains("plan_save") && message.contains("Add a rollback step"), "{message}");
    assert!(!message.contains("exit Plan Mode"), "{message}");
    assert_eq!(denied.output["pending_change_request"]["feedback"], json!("Add a rollback step"));

    // A save that fails keeps the feedback visible; a successful one reports it addressed.
    let (ok, stale) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "plan_save",
        json!({"plan_id": plan_id, "parent_revision": 0, "content": "step one\nrollback"}),
    )
    .await;
    assert!(!ok);
    assert_eq!(stale["pending_change_request"]["feedback"], json!("Add a rollback step"));
    let (ok, v2) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "plan_save",
        json!({"plan_id": plan_id, "parent_revision": 1, "content": "step one\nrollback"}),
    )
    .await;
    assert!(ok, "{v2}");
    assert_eq!(v2["addressed_change_request"]["feedback"], json!("Add a rollback step"));
    let (_, detail) = call(&fx.runtime, &alice, ToolTransport::Mcp, "plan_get", json!({"plan_id": plan_id})).await;
    assert_eq!(detail["pending_change_request"], Value::Null);
    let denied = fx.runtime.plan_mode_denial(Some(&alice), "run_process", Some(&fx.project)).unwrap();
    assert!(denied.error.unwrap_or_default().contains("exit Plan Mode"));
}

#[tokio::test]
async fn approve_and_start_refuses_before_approving_and_records_nothing() {
    let fx = fixture().await;
    let tunnel = chatgpt_via_tunnel();
    let op = operator("alice");
    let db = fx.runtime.communication_db.clone().unwrap();
    let request = json!({
        "project": fx.project, "title": "Fix add", "objective": "make add add",
        "acceptance": [{"name": "unit", "argv": ["true"]}],
    });
    let (ok, created) = call(&fx.runtime, &tunnel, ToolTransport::Mcp, "goal_run_start", request).await;
    assert!(ok, "{created}");
    let plan_id = created["plan"]["plan_id"].as_str().unwrap().to_string();
    let start = json!({"action": "approve_and_start", "plan_id": plan_id, "revision": 1,
                       "content_sha256": created["revision"]["content_sha256"]});
    let no_decisions = |db: &crate::db::Database| {
        db.plan_read(PlanReader::Operator, &plan_id, None, crate::tool_runtime::plan_mode::now_unix_ms())
            .unwrap()
            .decisions
            .is_empty()
    };

    // Emergency stop: refused before any approval exists.
    db.goal_kill_switch_set(true, "test", crate::tool_runtime::plan_mode::now_unix_ms()).unwrap();
    let (ok, output) = call(&fx.runtime, &op, ToolTransport::Api, "plan_review", start.clone()).await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "goal_emergency_stop_engaged");
    assert!(no_decisions(&db));
    db.goal_kill_switch_set(false, "test", crate::tool_runtime::plan_mode::now_unix_ms()).unwrap();

    // Owner now planning something else in Plan Mode: refused.
    let _other = start_plan(&fx, &tunnel).await;
    let (ok, output) = call(&fx.runtime, &op, ToolTransport::Api, "plan_review", start.clone()).await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "goal_owner_in_plan_mode");
    assert!(no_decisions(&db));
}

#[test]
fn approve_and_start_replays_consumed_revision_without_new_approval_or_mode_switch() {
    const SERVER_STACK_SIZE: usize = 8 * 1024 * 1024;
    std::thread::Builder::new()
        .name("plan-goal-replay-test".to_string())
        .stack_size(SERVER_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(SERVER_STACK_SIZE)
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let fx = fixture().await;
                let git_init = std::process::Command::new("git")
                    .args(["init", "-q"])
                    .current_dir(&fx.root)
                    .output()
                    .unwrap();
                assert!(
                    git_init.status.success(),
                    "{}",
                    String::from_utf8_lossy(&git_init.stderr)
                );
                let git_commit = std::process::Command::new("git")
                    .args([
                        "-c",
                        "user.name=Chadex Test",
                        "-c",
                        "user.email=chadex-test@example.invalid",
                        "commit",
                        "--allow-empty",
                        "-qm",
                        "fixture base",
                    ])
                    .current_dir(&fx.root)
                    .output()
                    .unwrap();
                assert!(
                    git_commit.status.success(),
                    "{}",
                    String::from_utf8_lossy(&git_commit.stderr)
                );

                let alice = chatgpt("alice");
                let op = operator("alice");
                let (plan_ok, started) = call_with_fake_runner(
                    &fx,
                    &alice,
                    ToolTransport::Mcp,
                    "plan_start",
                    json!({"project": fx.project, "title": "Add retries"}),
                )
                .await;
                assert!(plan_ok, "{started}");
                let plan_id = started["plan"]["plan_id"].as_str().unwrap().to_string();
                let (saved_ok, saved) = call_with_fake_runner(
                    &fx,
                    &alice,
                    ToolTransport::Mcp,
                    "plan_save",
                    json!({
                        "plan_id": plan_id,
                        "parent_revision": 0,
                        "content": "Add retries with bounded backoff",
                        "acceptance": [{"name": "unit", "argv": ["true"]}],
                    }),
                )
                .await;
                assert!(saved_ok, "{saved}");
                let revision = saved["revision"]["revision"].as_i64().unwrap();
                let content_sha256 = saved["revision"]["content_sha256"].clone();
                let start = |mode: &str| {
                    json!({
                        "action": "approve_and_start",
                        "plan_id": plan_id,
                        "revision": revision,
                        "content_sha256": content_sha256,
                        "mode": mode,
                    })
                };

                let (first_ok, first) = call_with_fake_runner(
                    &fx,
                    &op,
                    ToolTransport::Api,
                    "plan_review",
                    start("codex"),
                )
                .await;
                assert!(first_ok, "{first}");
                assert_eq!(first["created"], json!(true));
                let original_run_id = first["goal_run"]["goal_run_id"].clone();
                let original_approval_id = first["decision"]["approval_id"].clone();
                let original_provider = first["goal_run"]["provider"].clone();
                let original_mode = first["goal_run"]["mode"].clone();

                let (second_ok, second) = call_with_fake_runner(
                    &fx,
                    &op,
                    ToolTransport::Api,
                    "plan_review",
                    start("chatgpt"),
                )
                .await;
                assert!(second_ok, "{second}");
                assert_eq!(second["created"], json!(false));
                assert_eq!(second["goal_run"]["goal_run_id"], original_run_id);
                assert_eq!(second["goal_run"]["provider"], original_provider);
                assert_eq!(second["goal_run"]["mode"], original_mode);

                let db = fx.runtime.communication_db.clone().unwrap();
                let detail = db
                    .plan_read(PlanReader::Operator, &plan_id, None, crate::tool_runtime::plan_mode::now_unix_ms())
                    .unwrap();
                assert_eq!(detail.decisions.len(), 1, "retry must not allocate another approval ID");
                assert_eq!(detail.decisions[0].approval_id, original_approval_id);
                assert_eq!(detail.decisions[0].state, crate::db::PlanDecisionState::Consumed);
                assert_eq!(detail.decisions[0].consumed_by_goal_run_id.as_deref(), original_run_id.as_str());
                assert_eq!(db.goal_run_list(PlanReader::Operator, 20).unwrap().len(), 1);
            });
        })
        .expect("spawn server-sized regression test")
        .join()
        .expect("engine replay regression test thread panicked");
}

#[tokio::test]
async fn approve_and_start_needs_a_principal_recorded_for_the_latest_revision() {
    let fx = fixture().await;
    let alice = chatgpt("alice");
    let op = operator("alice");
    let plan_id = start_plan(&fx, &alice).await;
    let (ok, saved) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "plan_save",
        json!({"plan_id": plan_id, "parent_revision": 0, "content": "plan", "acceptance": [{"name": "t", "argv": ["true"]}]}),
    )
    .await;
    assert!(ok, "{saved}");
    // A later revision saved by a connection that can no longer run Goals
    // clears the recorded principal instead of keeping the older one.
    let mut weaker = chatgpt("alice");
    weaker.scopes.retain(|scope| scope != crate::auth::SCOPE_JOB_RUN);
    let (ok, v2) = call(
        &fx.runtime,
        &weaker,
        ToolTransport::Mcp,
        "plan_save",
        json!({"plan_id": plan_id, "parent_revision": 1, "content": "plan v2"}),
    )
    .await;
    assert!(ok, "{v2}");
    let (ok, output) = call(
        &fx.runtime,
        &op,
        ToolTransport::Api,
        "plan_review",
        json!({"action": "approve_and_start", "plan_id": plan_id, "revision": 2,
               "content_sha256": v2["revision"]["content_sha256"]}),
    )
    .await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "goal_dispatch_unknown");
}

#[test]
fn git_readiness_blocks_goal_start_without_a_commit_and_teaches_how_to_commit() {
    use crate::tool_runtime::plan_mode::git_readiness_from_outputs;
    let root = "/Users/me/投資手機介面";
    // git_status output: `git status --porcelain`.
    let repo = |files: usize| {
        let stdout: String = (0..files).map(|n| format!("?? file{n}.py\n")).collect();
        json!({"stdout": stdout, "stderr": "", "exit_code": 0})
    };

    // Repository without any commit: git_log reports an unborn HEAD.
    let unborn = git_readiness_from_outputs(&repo(16), Some(&json!({"head_commit": null})), root).unwrap();
    assert_eq!(unborn["blocks_goal_start"], json!(true));
    assert_eq!(unborn["has_commit"], json!(false));
    assert_eq!(unborn["uncommitted_files"], json!(16));
    assert_eq!(
        unborn["how_to_commit"],
        json!([
            "git -C \"/Users/me/投資手機介面\" add -A",
            "git -C \"/Users/me/投資手機介面\" commit -m \"Initial commit\"",
        ])
    );
    assert!(unborn["message"].as_str().unwrap().contains("no Git commit"));
    assert!(!unborn["tips"].as_array().unwrap().is_empty());

    // Not a repository at all: init first.
    let not_repo = json!({"stdout": "", "stderr": "fatal: not a git repository (or any of the parent directories): .git\n", "exit_code": 128});
    let plain = git_readiness_from_outputs(&not_repo, None, root).unwrap();
    assert_eq!(plain["is_git_repo"], json!(false));
    assert_eq!(plain["blocks_goal_start"], json!(true));
    assert_eq!(plain["how_to_commit"][0], json!("git -C \"/Users/me/投資手機介面\" init"));
    assert_eq!(plain["uncommitted_files"], Value::Null);

    // Committed with local changes: a hint only, never a block.
    let head = json!({"head_commit": "b8de8677ac56b7b63a2f46d624511e6cf01e92ba"});
    let dirty = git_readiness_from_outputs(&repo(3), Some(&head), root).unwrap();
    assert_eq!(dirty["blocks_goal_start"], json!(false));
    assert_eq!(dirty["uncommitted_files"], json!(3));
    assert_eq!(dirty["how_to_commit"].as_array().unwrap().len(), 2);
    assert!(dirty["message"].as_str().unwrap().contains("b8de8677ac56"));

    let clean = git_readiness_from_outputs(&repo(0), Some(&head), root).unwrap();
    assert_eq!(clean["blocks_goal_start"], json!(false));
    assert_eq!(clean["how_to_commit"], json!([]));
    assert_eq!(clean["tips"], json!([]));

    // Unknown state never warns or blocks.
    assert!(git_readiness_from_outputs(&json!({}), None, root).is_none());
    assert!(git_readiness_from_outputs(&json!({"stdout": "", "stderr": "fatal: permission denied", "exit_code": 128}), None, root).is_none());
    assert!(git_readiness_from_outputs(&repo(0), None, root).is_none());
    assert!(git_readiness_from_outputs(&repo(0), Some(&json!({})), root).is_none());

    // Paths are quoted for the shell.
    let odd = git_readiness_from_outputs(&repo(1), Some(&json!({"head_commit": null})), "/tmp/a \"b\" $c").unwrap();
    assert_eq!(odd["how_to_commit"][0], json!("git -C \"/tmp/a \\\"b\\\" \\$c\" add -A"));
}

/// End to end through the kernel and a Runner that really runs git, so the
/// readiness check is exercised against the actual git_status/git_log output.
#[tokio::test]
async fn git_readiness_reads_real_git_state_through_the_runner() {
    async fn readiness_for(root: &std::path::Path, client_id: &str) -> Value {
        let runtime = test_runtime();
        register_agent_with_projects(
            &runtime,
            client_id,
            Some("alice"),
            RunnerCapabilities {
                shell: true,
                git: true,
                structured_process_argv: true,
                internal_posix_script: true,
                ..Default::default()
            },
            vec![registered_project("repo", &root.to_string_lossy())],
        )
        .await;
        let project = crate::tool_runtime::runner_project_runtime_id(client_id, "repo");
        let runtime_for_task = runtime.clone();
        let task = tokio::spawn(async move {
            runtime_for_task.plan_git_readiness(Some(&operator("alice")), &project).await
        });
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        while !task.is_finished() {
            assert!(tokio::time::Instant::now() < deadline, "readiness did not finish");
            if let Some(request) = probe_patch_agent_request(&runtime, client_id).await {
                complete_agent_request_by_running_locally(&runtime, client_id, request).await;
            } else {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }
        task.await.unwrap().unwrap_or(Value::Null)
    }

    let unborn = tempfile::tempdir().unwrap();
    init_git_repo(unborn.path());
    std::fs::write(unborn.path().join("notes.py"), "print(1)\n").unwrap();
    let readiness = readiness_for(unborn.path(), "readiness-unborn").await;
    assert_eq!(readiness["is_git_repo"], json!(true), "{readiness}");
    assert_eq!(readiness["has_commit"], json!(false), "{readiness}");
    assert_eq!(readiness["blocks_goal_start"], json!(true), "{readiness}");
    assert_eq!(readiness["uncommitted_files"], json!(1), "{readiness}");

    let committed = tempfile::tempdir().unwrap();
    init_git_repo(committed.path());
    commit_file(committed.path(), "notes.py", "print(1)\n", "initial");
    let readiness = readiness_for(committed.path(), "readiness-committed").await;
    assert_eq!(readiness["has_commit"], json!(true), "{readiness}");
    assert_eq!(readiness["blocks_goal_start"], json!(false), "{readiness}");
    assert_eq!(readiness["uncommitted_files"], json!(0), "{readiness}");

    let plain = tempfile::tempdir().unwrap();
    let readiness = readiness_for(plain.path(), "readiness-plain").await;
    assert_eq!(readiness["is_git_repo"], json!(false), "{readiness}");
    assert_eq!(readiness["blocks_goal_start"], json!(true), "{readiness}");
}

#[tokio::test]
async fn plan_wait_approval_hands_the_approved_plan_straight_to_the_model() {
    let fx = fixture().await;
    let alice = chatgpt("alice");
    let plan_id = start_plan(&fx, &alice).await;
    let (ok, saved) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "plan_save",
        json!({"plan_id": plan_id, "parent_revision": 0, "content": "plan", "acceptance": [{"name": "t", "argv": ["true"]}]}),
    )
    .await;
    assert!(ok, "{saved}");
    assert_eq!(
        saved["suggested_call"],
        json!({
            "tool": "plan_wait_approval",
            "arguments": {"plan_id": plan_id, "wait_secs": 45}
        })
    );
    // Callable while Plan Mode is active; nothing decided yet.
    let (ok, waiting) = call(&fx.runtime, &alice, ToolTransport::Mcp, "plan_wait_approval", json!({"plan_id": plan_id, "wait_secs": 0})).await;
    assert!(ok, "{waiting}");
    assert_eq!(waiting["status"], json!("waiting"));
    assert_eq!(
        waiting["suggested_call"],
        json!({
            "tool": "plan_wait_approval",
            "arguments": {"plan_id": plan_id, "wait_secs": 45}
        })
    );
    // Another principal cannot watch Alice's plan.
    let (ok, _) = call(&fx.runtime, &chatgpt("bob"), ToolTransport::Mcp, "plan_wait_approval", json!({"plan_id": plan_id, "wait_secs": 0})).await;
    assert!(!ok);

    // Approved in the app while the model waits: it returns approved and the
    // connection can write again at once.
    let db = fx.runtime.communication_db.clone().unwrap();
    let sha = saved["revision"]["content_sha256"].as_str().unwrap().to_string();
    let approve = {
        let db = db.clone();
        let project = fx.project.clone();
        let root = fx.root.clone();
        let plan_id = plan_id.clone();
        async move {
            tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            db.plan_approve(
                &plan_id,
                1,
                &sha,
                PlanApprovalBinding {
                    project_id: project,
                    runner_client_id: "client-a".to_string(),
                    project_root: root,
                    base_commit: crate::db::UNBORN_BASE_COMMIT.to_string(),
                },
                "operator:alice",
                DEFAULT_PLAN_APPROVAL_TTL_MS,
                crate::tool_runtime::plan_mode::now_unix_ms(),
            )
            .unwrap();
        }
    };
    let (_, (ok, approved)) = tokio::join!(
        approve,
        call(&fx.runtime, &alice, ToolTransport::Mcp, "plan_wait_approval", json!({"plan_id": plan_id, "wait_secs": 10}))
    );
    assert!(ok, "{approved}");
    assert_eq!(approved["status"], json!("approved"));
    assert!(approved.get("next").is_none());
    assert!(approved.get("suggested_call").is_none());
    assert!(!approved.to_string().contains("implement"));
    assert!(!approved.to_string().contains("plan_wait_approval"));
    assert!(fx.runtime.plan_mode_denial(Some(&alice), "write_project_file", Some(&fx.project)).is_none());
    // The model cannot turn this approval into a Goal, and even the app
    // cannot start one from an approval without a commit.
    let (ok, output) = call(&fx.runtime, &alice, ToolTransport::Mcp, "goal_run_start", json!({"plan_id": plan_id})).await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "not_a_goal");
    assert_eq!(operator_start(&fx, &plan_id, "codex").unwrap_err().code(), "plan_base_commit_unavailable");
}

#[tokio::test]
async fn plan_wait_approval_rejects_goal_origin_without_suggesting_plan_or_implementation() {
    let demo = crate::tool_runtime::runner_project_runtime_id("client-a", "demo");
    let fx = app_fixture(Some(&demo), true).await;
    let auth = chatgpt_via_tunnel();
    let (ok, created) = call(
        &fx.runtime,
        &auth,
        ToolTransport::Mcp,
        "goal_run_start",
        json!({
            "title": "Goal origin guard",
            "objective": "Keep this as a Goal request",
            "acceptance": [{"name": "unit", "argv": ["true"]}],
        }),
    )
    .await;
    assert!(ok, "{created}");
    let plan_id = created["plan"]["plan_id"].as_str().unwrap();

    let (ok, rejected) = call(
        &fx.runtime,
        &auth,
        ToolTransport::Mcp,
        "plan_wait_approval",
        json!({"plan_id": plan_id, "wait_secs": 0}),
    )
    .await;
    assert!(!ok, "a Goal-origin request must not enter Plan approval polling");
    assert_eq!(error_kind(&rejected), "not_a_plan");
    assert_eq!(
        rejected["suggested_call"],
        json!({"tool": "goal_run_list", "arguments": {}})
    );
    assert!(!rejected.to_string().contains("implement"));
    assert!(!rejected.to_string().contains("plan_wait_approval"));
}

#[tokio::test]
async fn terminal_plan_wait_results_do_not_suggest_waiting_or_implementation() {
    let fx = fixture().await;
    let alice = chatgpt("alice");
    let closed_id = start_plan(&fx, &alice).await;
    let db = fx.runtime.communication_db.clone().unwrap();
    db.plan_close(&closed_id, "closed in test", crate::tool_runtime::plan_mode::now_unix_ms())
        .unwrap();
    let (ok, closed) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "plan_wait_approval",
        json!({"plan_id": closed_id, "wait_secs": 0}),
    )
    .await;
    assert!(ok, "{closed}");
    assert_eq!(closed["status"], json!("closed"));
    assert!(closed.get("suggested_call").is_none());
    assert!(!closed.to_string().contains("plan_wait_approval"));
    assert!(!closed.to_string().contains("implement"));

    let withdrawn_id = start_plan(&fx, &alice).await;
    let (ok, saved) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "plan_save",
        json!({
            "plan_id": withdrawn_id,
            "parent_revision": 0,
            "content": "Plan to withdraw",
            "acceptance": [{"name": "unit", "argv": ["true"]}]
        }),
    )
    .await;
    assert!(ok, "{saved}");
    let decision = db
        .plan_approve(
            &withdrawn_id,
            1,
            saved["revision"]["content_sha256"].as_str().unwrap(),
            PlanApprovalBinding {
                project_id: fx.project.clone(),
                runner_client_id: "client-a".to_string(),
                project_root: fx.root.clone(),
                base_commit: crate::db::UNBORN_BASE_COMMIT.to_string(),
            },
            "operator:alice",
            DEFAULT_PLAN_APPROVAL_TTL_MS,
            crate::tool_runtime::plan_mode::now_unix_ms(),
        )
        .unwrap();
    db.plan_revoke_approval(
        &decision.approval_id,
        crate::tool_runtime::plan_mode::now_unix_ms(),
    )
    .unwrap();
    let (ok, withdrawn) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "plan_wait_approval",
        json!({"plan_id": withdrawn_id, "wait_secs": 0}),
    )
    .await;
    assert!(ok, "{withdrawn}");
    assert_eq!(withdrawn["status"], json!("approval_withdrawn"));
    assert!(withdrawn.get("suggested_call").is_none());
    assert!(!withdrawn.to_string().contains("plan_wait_approval"));
    assert_eq!(
        withdrawn["next"],
        json!("The approval was withdrawn or expired. Do not implement; ask the user what they want.")
    );
}

#[tokio::test]
async fn goal_check_is_owner_only_chatgpt_mode_only_and_blocked_in_plan_mode() {
    let fx = fixture().await;
    let tunnel = chatgpt_via_tunnel();
    let db = fx.runtime.communication_db.clone().unwrap();
    let (ok, created) = call(
        &fx.runtime,
        &tunnel,
        ToolTransport::Mcp,
        "goal_run_start",
        json!({"project": fx.project, "title": "Fix", "objective": "fix it", "acceptance": [{"name": "t", "argv": ["true"]}]}),
    )
    .await;
    assert!(ok, "{created}");
    let plan_id = created["plan"]["plan_id"].as_str().unwrap().to_string();
    db.plan_approve(
        &plan_id,
        1,
        created["revision"]["content_sha256"].as_str().unwrap(),
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
    // The model picks no mode and starts nothing; the app starts ChatGPT mode.
    let run_id = operator_start(&fx, &plan_id, crate::db::CHATGPT_GOAL_PROVIDER).unwrap().goal_run_id;
    let (ok, started) = call(&fx.runtime, &tunnel, ToolTransport::Mcp, "goal_run_start", json!({"plan_id": plan_id})).await;
    assert!(ok, "{started}");
    assert_eq!(started["goal_run"]["mode"], json!("chatgpt"));


    // No worktree yet (no supervisor in this runtime): nothing is checked.
    let (ok, early) = call(&fx.runtime, &tunnel, ToolTransport::Mcp, "goal_check", json!({"goal_run_id": run_id, "wait_secs": 0})).await;
    assert!(ok, "{early}");
    assert_eq!(early["achieved"], json!(false));
    assert!(early["next"].as_str().unwrap().contains("preparing"));
    assert_eq!(db.goal_run_read(PlanReader::Operator, &run_id).unwrap().run.chatgpt_ready_iteration, 0);

    // Someone else cannot drive it.
    let (ok, output) = call(&fx.runtime, &chatgpt("bob"), ToolTransport::Mcp, "goal_check", json!({"goal_run_id": run_id, "wait_secs": 0})).await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "goal_run_not_found");

    // While the owner is in Plan Mode, goal_check is refused like any execution.
    let _plan = start_plan(&fx, &tunnel).await;
    let (ok, output) = call(&fx.runtime, &tunnel, ToolTransport::Mcp, "goal_check", json!({"goal_run_id": run_id, "wait_secs": 0})).await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "plan_mode_read_only");
}

/// Two primary projects and one Goal worktree of `demo` on one Runner, with an
/// optional Chadex app state file naming `app_project` (an agent project id).
struct AppFixture {
    _tmp: tempfile::TempDir,
    _roots: Vec<tempfile::TempDir>,
    _state_dir: tempfile::TempDir,
    runtime: ToolRuntime,
    demo: String,
    other: String,
    worktree: String,
}

async fn app_fixture(app_project: Option<&str>, with_other: bool) -> AppFixture {
    let (tmp, db) = crate::test_support::test_db();
    let state_dir = tempfile::tempdir().unwrap();
    let state_file = state_dir.path().join("desktop-state.json");
    if let Some(project) = app_project {
        std::fs::write(
            &state_file,
            json!({"project": {"path": "/unused", "runtime_project_id": project}}).to_string(),
        )
        .unwrap();
    }
    let runtime = ToolRuntime::new_for_tests()
        .with_communication_database(db)
        .with_app_active_project_file(Some(state_file));
    let roots: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
    let path = |index: usize| roots[index].path().to_string_lossy().to_string();
    let mut worktree = registered_project("demo-wt", &path(2));
    worktree.lineage = Some(crate::runner_protocol::RunnerProjectLineage::ManagedWorktreeSource {
        source_project_id: "demo".to_string(),
        source_root_fingerprint: format!("wc_projroot_{}", "f".repeat(64)),
        base_sha: "a".repeat(40),
    });
    let mut projects = vec![registered_project("demo", &path(0)), worktree];
    if with_other {
        projects.push(registered_project("other", &path(1)));
    }
    register_agent_with_projects(
        &runtime,
        "client-a",
        Some("alice"),
        RunnerCapabilities {
            shell: true,
            structured_process_argv: true,
            ..Default::default()
        },
        projects,
    )
    .await;
    AppFixture {
        _tmp: tmp,
        _roots: roots,
        _state_dir: state_dir,
        runtime,
        demo: crate::tool_runtime::runner_project_runtime_id("client-a", "demo"),
        other: crate::tool_runtime::runner_project_runtime_id("client-a", "other"),
        worktree: crate::tool_runtime::runner_project_runtime_id("client-a", "demo-wt"),
    }
}

#[tokio::test]
async fn plan_start_without_project_uses_the_project_open_in_the_chadex_app() {
    let other = crate::tool_runtime::runner_project_runtime_id("client-a", "other");
    let fx = app_fixture(Some(&other), true).await;
    let alice = chatgpt("alice");
    let (ok, output) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "plan_start",
        json!({"title": "Finish G1"}),
    )
    .await;
    assert!(ok, "{output}");
    assert_eq!(output["plan"]["project_id"], json!(fx.other));
    assert_eq!(
        output["project_resolution"],
        json!({"project": fx.other, "source": "chadex_app"})
    );
    // The model is told to tell the user which project (name and path) it uses.
    let note = output["plan_mode"]["note"].as_str().unwrap();
    assert!(note.starts_with("This Chadex connection is now read-only"), "{note}");
    assert!(note.contains("the project open in the Chadex app: other ("), "{note}");
    assert!(note.contains(&fx._roots[1].path().file_name().unwrap().to_string_lossy().to_string()), "{note}");
    assert!(note.contains("Tell the user which project you are working in."), "{note}");
    // Plan Mode is locked to that project exactly as with an explicit id.
    assert!(fx
        .runtime
        .plan_mode_denial(Some(&alice), "read_files", Some(&fx.demo))
        .is_some());
    assert!(fx
        .runtime
        .plan_mode_denial(Some(&alice), "read_files", Some(&fx.other))
        .is_none());
    // Repeating it is the same idempotent start; an explicit project still wins.
    let (ok, again) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "plan_start",
        json!({"title": "Finish G1", "project": fx.other}),
    )
    .await;
    assert!(ok, "{again}");
    assert_eq!(again["created"], json!(false));
    assert_eq!(again["project_resolution"], Value::Null);
    assert!(!again["plan_mode"]["note"]
        .as_str()
        .unwrap()
        .contains("Tell the user which project"));
}

#[tokio::test]
async fn resent_plan_start_without_project_keeps_the_locked_project_after_the_app_switches() {
    let other = crate::tool_runtime::runner_project_runtime_id("client-a", "other");
    let fx = app_fixture(Some(&other), true).await;
    let alice = chatgpt("alice");
    let (ok, first) = call(&fx.runtime, &alice, ToolTransport::Mcp, "plan_start", json!({"title": "Finish G1"})).await;
    assert!(ok, "{first}");
    // The user now opens `demo` in the app.
    let state_file = fx.runtime.app_active_project_file.clone().unwrap();
    std::fs::write(
        &state_file,
        json!({"project": {"runtime_project_id": fx.demo}}).to_string(),
    )
    .unwrap();
    let (ok, again) = call(&fx.runtime, &alice, ToolTransport::Mcp, "plan_start", json!({"title": "Finish G1"})).await;
    assert!(ok, "{again}");
    assert_eq!(again["created"], json!(false));
    assert_eq!(again["plan"]["plan_id"], first["plan"]["plan_id"]);
    assert_eq!(
        again["project_resolution"],
        json!({"project": fx.other, "source": "active_plan"})
    );
    // Another principal without a lock follows the app.
    let (ok, other_user) = call(&fx.runtime, &chatgpt_via_tunnel(), ToolTransport::Mcp, "plan_start", json!({"title": "x"})).await;
    assert!(ok, "{other_user}");
    assert_eq!(other_user["plan"]["project_id"], json!(fx.demo));
}

#[tokio::test]
async fn only_the_runner_owner_or_local_operator_inherits_the_app_project() {
    let demo = crate::tool_runtime::runner_project_runtime_id("client-a", "demo");
    let fx = app_fixture(Some(&demo), true).await;
    // An admin of another user can see alice's Runner but must not inherit
    // alice's app selection.
    let mut admin = chatgpt("bob");
    admin.scopes.push(crate::auth::SCOPE_ADMIN.to_string());
    let listed = fx.runtime.list_projects(Some(&admin)).await;
    assert!(listed.success);
    assert_eq!(listed.output["count"], json!(3), "{}", listed.output);
    assert_eq!(listed.output["active_project"], Value::Null);
    assert!(listed.output["projects"]
        .as_array()
        .unwrap()
        .iter()
        .all(|project| project["active"] == json!(false)));
    let (ok, output) = call(&fx.runtime, &admin, ToolTransport::Mcp, "plan_start", json!({"title": "x"})).await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "project_required");
    assert_eq!(output["app_project_set"], json!(true));
    // The owner and the local tunnel (bootstrap) principal do.
    for auth in [chatgpt("alice"), chatgpt_via_tunnel()] {
        let listed = fx.runtime.list_projects(Some(&auth)).await;
        assert_eq!(listed.output["active_project"]["id"], json!(fx.demo));
    }
}

#[tokio::test]
async fn list_projects_marks_the_app_project_given_by_its_short_id() {
    // The resolver accepts the same short forms an explicit project does, so
    // marking and defaulting agree.
    let fx = app_fixture(Some("demo"), true).await;
    let alice = chatgpt("alice");
    let listed = fx.runtime.list_projects(Some(&alice)).await;
    assert_eq!(listed.output["active_project"]["id"], json!(fx.demo), "{}", listed.output);
    let (ok, output) = call(&fx.runtime, &alice, ToolTransport::Mcp, "plan_start", json!({"title": "x"})).await;
    assert!(ok, "{output}");
    assert_eq!(output["plan"]["project_id"], json!(fx.demo));
}

#[tokio::test]
async fn plan_start_without_project_and_no_app_project_lists_candidates_instead_of_guessing() {
    let fx = app_fixture(None, true).await;
    let alice = chatgpt("alice");
    let (ok, output) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "plan_start",
        json!({"title": "Finish G1"}),
    )
    .await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "project_required");
    assert_eq!(output["state_changed"], json!(false));
    let ids: Vec<&str> = output["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|candidate| candidate["id"].as_str().unwrap())
        .collect();
    // Goal worktrees are not offered as candidates.
    assert_eq!(ids, vec![fx.demo.as_str(), fx.other.as_str()]);
    // No lock was taken.
    assert!(fx
        .runtime
        .plan_mode_denial(Some(&alice), "run_process", Some(&fx.demo))
        .is_none());
}

#[tokio::test]
async fn plan_start_without_project_uses_the_only_primary_project_when_no_app_project_is_set() {
    let fx = app_fixture(None, false).await;
    let (ok, output) = call(
        &fx.runtime,
        &chatgpt("alice"),
        ToolTransport::Mcp,
        "plan_start",
        json!({"title": "Finish G1"}),
    )
    .await;
    assert!(ok, "{output}");
    assert_eq!(output["plan"]["project_id"], json!(fx.demo));
    assert_eq!(output["project_resolution"]["source"], json!("only_project"));
}

#[tokio::test]
async fn app_project_the_caller_cannot_use_is_an_error_not_a_fallback_or_a_grant() {
    // The app names a project on another user's Runner: alice's connection may
    // not use it, and must not silently fall back to one of her own projects.
    let fx = app_fixture(Some("agent:client-b:secret"), false).await;
    register_agent_with_projects(
        &fx.runtime,
        "client-b",
        Some("carol"),
        RunnerCapabilities::default(),
        vec![registered_project("secret", &fx._roots[1].path().to_string_lossy())],
    )
    .await;
    let alice = chatgpt("alice");
    let (ok, output) = call(
        &fx.runtime,
        &alice,
        ToolTransport::Mcp,
        "plan_start",
        json!({"title": "Finish G1"}),
    )
    .await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "project_required");
    // The app's project id is never echoed to a caller that may not use it.
    assert_eq!(output["app_project_set"], json!(true));
    assert!(!output.to_string().contains("secret"), "{output}");
    assert!(output["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .all(|candidate| candidate["id"] != json!("agent:client-b:secret")));
    // list_projects neither shows nor reports it to alice.
    let listed = fx.runtime.list_projects(Some(&alice)).await;
    assert!(listed.success);
    assert_eq!(listed.output["active_project"], Value::Null);
}

#[tokio::test]
async fn direct_goal_request_without_project_uses_the_project_open_in_the_chadex_app() {
    let demo = crate::tool_runtime::runner_project_runtime_id("client-a", "demo");
    let fx = app_fixture(Some(&demo), true).await;
    let (ok, created) = call(
        &fx.runtime,
        &chatgpt_via_tunnel(),
        ToolTransport::Mcp,
        "goal_run_start",
        json!({
            "title": "Fix add",
            "objective": "add() must add",
            "acceptance": [{"name": "unit", "argv": ["python3", "-m", "unittest"]}],
        }),
    )
    .await;
    assert!(ok, "{created}");
    assert_eq!(created["plan"]["project_id"], json!(fx.demo));
    assert_eq!(created["project_resolution"]["source"], json!("chadex_app"));
    let next = created["next"].as_str().unwrap();
    assert!(next.starts_with("Tell the user to press Start Goal"), "{next}");
    assert!(next.contains("Chadex used the project open in the Chadex app: demo ("), "{next}");

    // Without an app project and with two candidates it refuses.
    let fx = app_fixture(None, true).await;
    let (ok, output) = call(
        &fx.runtime,
        &chatgpt_via_tunnel(),
        ToolTransport::Mcp,
        "goal_run_start",
        json!({
            "title": "Fix add",
            "objective": "add() must add",
            "acceptance": [{"name": "unit", "argv": ["python3", "-m", "unittest"]}],
        }),
    )
    .await;
    assert!(!ok);
    assert_eq!(error_kind(&output), "project_required");
}

#[tokio::test]
async fn list_projects_marks_the_app_project_and_goal_worktrees() {
    let demo = crate::tool_runtime::runner_project_runtime_id("client-a", "demo");
    let fx = app_fixture(Some(&demo), true).await;
    let alice = chatgpt("alice");
    let listed = fx.runtime.list_projects(Some(&alice)).await;
    assert!(listed.success, "{:?}", listed.error);
    assert_eq!(listed.output["active_project"]["id"], json!(fx.demo));
    let by_id = |id: &str| {
        listed.output["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|project| project["id"] == json!(id))
            .cloned()
            .unwrap()
    };
    assert_eq!(by_id(&fx.demo)["active"], json!(true));
    assert_eq!(by_id(&fx.other)["active"], json!(false));
    assert_eq!(by_id(&fx.demo)["managed_worktree"], json!(false));
    assert_eq!(by_id(&fx.worktree)["managed_worktree"], json!(true));
    assert_eq!(by_id(&fx.worktree)["source_project"], json!(fx.demo));
    // Nothing is hidden.
    assert_eq!(listed.output["count"], json!(3));

    // A filtered page that leaves the app project out still reports it.
    let filtered = fx
        .runtime
        .list_projects_with_options(
            Some(&alice),
            crate::tool_runtime::projects::ListProjectsOptions {
                project: Some(fx.other.clone()),
                summary_only: true,
                ..Default::default()
            },
        )
        .await;
    assert!(filtered.success);
    assert_eq!(filtered.output["count"], json!(1));
    assert_eq!(filtered.output["projects"][0]["active"], json!(false));
    assert_eq!(filtered.output["active_project"]["id"], json!(fx.demo));
}
