use super::*;

fn adaptive_direct_auth() -> crate::auth::AuthContext {
    let mut auth = crate::auth::shared_key_context("adaptive-direct-test");
    auth.scopes.extend([
        crate::auth::SCOPE_PLUGIN_INSPECT.to_string(),
        crate::auth::SCOPE_PLUGIN_INVOKE.to_string(),
        crate::auth::SCOPE_PLUGIN_MANAGE.to_string(),
    ]);
    auth
}

fn tool_names(value: &Value) -> Vec<&str> {
    value["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("tool name"))
        .collect()
}

#[test]
fn plan_mode_wait_suggestion_schema_and_gateway_projection_match_the_runtime_contract() {
    let plan_id = "wc_plan_abcdefghijkl";
    let canonical = webcodex_tool_contracts::output_schema_for_tool("plan_save");
    let suggested_schema = &canonical["properties"]["output"]["properties"]["suggested_call"];
    let targets = suggested_schema["anyOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|schema| webcodex_tool_contracts::suggested_tool_call_schema_target(schema).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(targets, ["plan_wait_approval", "goal_run_list"]);
    let plan_wait_schema = &suggested_schema["anyOf"][0];
    let arguments_schema = &plan_wait_schema["properties"]["arguments"];
    assert_eq!(arguments_schema["required"], json!(["plan_id", "wait_secs"]));
    assert_eq!(arguments_schema["properties"]["plan_id"]["pattern"], "^wc_plan_[A-Za-z0-9_-]{12,32}$");
    assert_eq!(arguments_schema["properties"]["wait_secs"]["const"], 45);

    let waiting = webcodex_tool_contracts::output_schema_for_tool("plan_wait_approval");
    assert_eq!(
        waiting["properties"]["output"]["allOf"][0]["then"]["required"],
        json!(["suggested_call"])
    );
    let mut result = crate::tool_runtime::ToolResult::ok(json!({
        "suggested_call": {
            "tool": "plan_wait_approval",
            "arguments": {"plan_id": plan_id, "wait_secs": 45}
        }
    }));
    crate::model_surface::project_tool_result_suggested_calls(
        "plan_save",
        &mut result,
        &|target| crate::model_surface::suggested_tool_call_route(target, false),
    );
    assert_eq!(
        result.output["suggested_call"],
        json!({
            "tool": "call_runtime_tool",
            "arguments": {
                "tool": "plan_wait_approval",
                "arguments": {"plan_id": plan_id, "wait_secs": 45}
            }
        })
    );

    let mut goal_result = crate::tool_runtime::ToolResult::ok(json!({
        "plan": {"origin": "goal"},
        "suggested_call": {"tool": "goal_run_list", "arguments": {}}
    }));
    crate::model_surface::project_tool_result_suggested_calls(
        "plan_save",
        &mut goal_result,
        &|target| crate::model_surface::suggested_tool_call_route(target, false),
    );
    assert_eq!(
        goal_result.output["suggested_call"],
        json!({
            "tool": "call_runtime_tool",
            "arguments": {"tool": "goal_run_list", "arguments": {}}
        })
    );

    let mut goal_origin_wait = crate::tool_runtime::ToolResult::err_with_output(
        "Goal-origin requests do not use Plan approval polling".to_string(),
        json!({
            "error_kind": "not_a_plan",
            "suggested_call": {"tool": "goal_run_list", "arguments": {}}
        }),
    );
    crate::model_surface::project_tool_result_suggested_calls(
        "plan_wait_approval",
        &mut goal_origin_wait,
        &|target| crate::model_surface::suggested_tool_call_route(target, false),
    );
    assert_eq!(
        goal_origin_wait.output["suggested_call"],
        json!({
            "tool": "call_runtime_tool",
            "arguments": {"tool": "goal_run_list", "arguments": {}}
        })
    );
}

#[tokio::test]
async fn adaptive_mcp_plan_save_projects_complete_gateway_wait_arguments() {
    let (_tmp, db) = crate::test_support::test_db();
    let mut auth = adaptive_direct_auth();
    auth.scopes.extend([
        crate::auth::SCOPE_RUNTIME_READ.to_string(),
        crate::auth::SCOPE_PROJECT_READ.to_string(),
        crate::auth::SCOPE_PROJECT_WRITE.to_string(),
    ]);
    let owner = crate::tool_runtime::plan_mode::plan_principal_key(Some(&auth));
    let now = crate::tool_runtime::plan_mode::now_unix_ms();
    let plan = db
        .plan_start(&owner, "agent:client-a:demo", "Gateway projection", now)
        .unwrap()
        .plan;
    let runtime = ToolRuntime::new_for_tests().with_communication_database(db);

    let outcome = handle_mcp_request(
        &runtime,
        rpc(
            "tools/call",
            Some(json!(617)),
            mcp_2026_params(json!({
                "name": "call_runtime_tool",
                "arguments": {
                    "tool": "plan_save",
                    "arguments": {
                        "plan_id": plan.plan_id.clone(),
                        "parent_revision": 0,
                        "content": "Save and wait",
                        "acceptance": [{"name": "unit", "argv": ["true"]}]
                    }
                }
            })),
        ),
        Some(&auth),
    )
    .await;
    let McpOutcome::Ok(value) = outcome else {
        panic!("gateway plan_save must produce an MCP result: {outcome:?}");
    };
    let projected = &value["result"]["structuredContent"]["output"]["suggested_call"];
    assert_eq!(projected["tool"], "call_runtime_tool");
    assert_eq!(projected["arguments"]["tool"], "plan_wait_approval");
    assert_eq!(
        projected["arguments"]["arguments"],
        json!({"plan_id": plan.plan_id, "wait_secs": 45})
    );
}

// `handle_mcp_request` pins the compact default, so no env guard is needed.
#[tokio::test]
async fn adaptive_tools_list_exposes_ranked_direct_tools_and_gateway() {
    let runtime = test_runtime();
    let auth = adaptive_direct_auth();
    let outcome = handle_mcp_request(
        &runtime,
        rpc("tools/list", Some(json!(60)), mcp_2026_params(json!({}))),
        Some(&auth),
    )
    .await;
    let McpOutcome::Ok(value) = outcome else {
        panic!("Adaptive tools/list must succeed");
    };
    let names = tool_names(&value);
    assert!(names.contains(&crate::mcp::tools::ADAPTIVE_RUNTIME_GATEWAY_TOOL_NAME));
    #[cfg(feature = "experimental-code-mode")]
    {
        const MAX_EXPERIMENTAL_CODE_MODE_TOOL_BYTES: usize = 4 * 1024;
        let code_mode = value["result"]["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .find(|tool| tool["name"] == "code_mode_exec")
            .expect("experimental Code Mode feature must expose code_mode_exec directly");
        let code_mode_bytes = serde_json::to_vec(code_mode).unwrap().len();
        assert!(
            code_mode_bytes <= MAX_EXPERIMENTAL_CODE_MODE_TOOL_BYTES,
            "experimental code_mode_exec compact schema cost {code_mode_bytes} exceeded {MAX_EXPERIMENTAL_CODE_MODE_TOOL_BYTES} bytes"
        );
    }
    assert!(
        !names.contains(&"apply_patch"),
        "long-tail tool leaked direct"
    );
    assert!(
        !names.contains(&"run_script"),
        "long-tail tool leaked direct"
    );
    for required in [
        "work_on_project",
        "runtime_status",
        "tool_manifest",
        "read_files",
        "search_project_texts",
        "apply_text_edits",
        "run_process",
        "run_shell",
        "observe_jobs",
        "show_changes",
        "finish_coding_task",
        "import_conversation_files_to_project",
        "project_artifact",
        "plugin_tool",
        "session_handoff_summary",
        "present_goal_plan",
        "present_work_result",
        "present_changes",
        "present_agent_continuation",
        "wait_for_agent_events",
    ] {
        assert!(
            names.contains(&required),
            "missing Adaptive direct tool {required}"
        );
        assert!(
            crate::tool_runtime::tool_definition::is_adaptive_runtime_direct_tool(required),
            "{required} must be admitted by the Phase 16B startup surface"
        );
    }
    for demoted in [
        "cargo_check",
        "cargo_test",
        "git_review_summary",
        "git_diff_hunks",
        "workspace_hygiene_check",
        "run_detached_process",
        "skill_load",
    ] {
        assert!(
            !names.contains(&demoted),
            "{demoted} should be gateway-only on the Phase 16B initial surface"
        );
        assert!(
            !crate::tool_runtime::tool_definition::is_adaptive_runtime_direct_tool(demoted),
            "{demoted} should not be effectively direct"
        );
    }
    for plan_goal_gateway_tool in [
        "plan_save",
        "plan_get",
        "plan_list",
        "plan_wait_approval",
        "goal_run_status",
        "goal_run_list",
        "goal_run_control",
        "goal_check",
    ] {
        assert!(
            !names.contains(&plan_goal_gateway_tool),
            "Plan/Goal long-tail tool {plan_goal_gateway_tool} must stay off initial tools/list"
        );
    }
    for entry_tool in ["plan_start", "goal_run_start"] {
        assert!(
            names.contains(&entry_tool),
            "Plan/Goal entry tool {entry_tool} must be present on initial tools/list"
        );
    }
    // 30 + the two Plan/Goal Mode entry points (plan_start, goal_run_start).
    assert!(
        names.len() <= 32,
        "Phase 16B non-UI startup surface regressed to {} direct callables",
        names.len()
    );

    let work_on_project = value["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "work_on_project")
        .expect("work_on_project direct tool");
    let work_on_project_definition =
        crate::tool_runtime::tool_definition::lookup_tool_definition("work_on_project").unwrap();
    let compact_description = work_on_project["description"].as_str().unwrap();
    assert_eq!(
        compact_description,
        work_on_project_definition.gpt_action_description().unwrap()
    );
    assert!(
        compact_description.len()
            < work_on_project_definition
                .model_spec
                .expect("work_on_project model spec")
                .description
                .len(),
        "compact MCP should use the shorter model-facing description"
    );
    assert_eq!(
        work_on_project["inputSchema"]["properties"]["recording_session_id"]["description"],
        "Optional Workflow Session id for call recording only; grants no authority."
    );

    let surface_bytes = serde_json::to_vec(&value["result"]["tools"])
        .expect("serialize Phase 16B startup surface")
        .len();
    eprintln!(
        "phase16b_initial_surface tools={} bytes={surface_bytes}",
        names.len()
    );
    let surface_budget = if cfg!(feature = "experimental-code-mode") {
        128 * 1024
    } else {
        112 * 1024
    };
    assert!(
        surface_bytes <= surface_budget,
        "Phase 16B compact startup surface regressed to {surface_bytes} bytes"
    );
}

// `handle_mcp_request` pins the compact default, so no env guard is needed.
#[tokio::test]
async fn phase16b_ui_surface_stays_bounded_and_preserves_presentation_tools() {
    let runtime = test_runtime();
    let auth = adaptive_direct_auth();
    let outcome = handle_mcp_request(
        &runtime,
        rpc(
            "tools/list",
            Some(json!(600)),
            mcp_2026_ui_params(json!({})),
        ),
        Some(&auth),
    )
    .await;
    let McpOutcome::Ok(value) = outcome else {
        panic!("UI tools/list must succeed");
    };
    let names = tool_names(&value);
    for app_tool in [
        "goal_plan_state",
        "work_result_state",
        "changes_file_diff",
        "agent_wait_state",
        "agent_continuation_bind",
        "agent_continuation_state",
    ] {
        assert!(names.contains(&app_tool), "missing app-only tool {app_tool}");
    }
    let app_only_count = value["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| tool.pointer("/_meta/ui/visibility") == Some(&json!(["app"])))
        .count();
    assert!(app_only_count >= 6, "expected app-only tool projection");
    // 22 Adaptive direct tools (including the Computer observe/control pair), the
    // gateway, six operator-extension Skill/Memory tools and eleven app-only tools.
    // +2 for the Plan/Goal Mode entry points (plan_start, goal_run_start).
    let max_tools = if cfg!(feature = "experimental-code-mode") {
        45
    } else {
        42
    };
    assert!(
        names.len() <= max_tools,
        "Phase 16B UI startup surface regressed to {} direct callables: {names:?}",
        names.len()
    );
    let surface_bytes = serde_json::to_vec(&value["result"]["tools"])
        .expect("serialize Phase 16B UI startup surface")
        .len();
    eprintln!(
        "phase16b_ui_initial_surface tools={} bytes={surface_bytes}",
        names.len()
    );
    let surface_budget = if cfg!(feature = "experimental-code-mode") {
        168 * 1024
    } else {
        152 * 1024
    };
    assert!(
        surface_bytes <= surface_budget,
        "Phase 16B compact UI startup surface regressed to {surface_bytes} bytes"
    );
}

#[tokio::test]
async fn long_tail_manifest_routes_through_call_runtime_tool() {
    let runtime = test_runtime();
    for (id, tool_name) in [
        (61, "apply_patch"),
        (612, "cargo_check"),
        (615, "plan_wait_approval"),
    ] {
        let outcome = handle_mcp_request(
            &runtime,
            rpc(
                "tools/call",
                Some(json!(id)),
                mcp_2026_params(json!({
                    "name": "tool_manifest",
                    "arguments": {"tool_name": tool_name}
                })),
            ),
            None,
        )
        .await;
        let McpOutcome::Ok(value) = outcome else {
            panic!("tool_manifest must succeed for {tool_name}");
        };
        let output = &value["result"]["structuredContent"]["output"];
        assert_eq!(
            output["route"]["mode"], "gateway",
            "{tool_name} must remain discoverable through the gateway"
        );
        assert_eq!(
            output["route"]["via"], "call_runtime_tool",
            "{tool_name} gateway route"
        );
    }

    for (id, tool_name) in [(613, "read_files"), (614, "runtime_status")] {
        let direct = handle_mcp_request(
            &runtime,
            rpc(
                "tools/call",
                Some(json!(id)),
                mcp_2026_params(json!({
                    "name": "tool_manifest",
                    "arguments": {"tool_name": tool_name}
                })),
            ),
            None,
        )
        .await;
        let McpOutcome::Ok(value) = direct else {
            panic!("tool_manifest must succeed for {tool_name}");
        };
        assert_eq!(
            value["result"]["structuredContent"]["output"]["route"]["mode"],
            "direct",
            "{tool_name}"
        );
    }

    let save = handle_mcp_request(
        &runtime,
        rpc(
            "tools/call",
            Some(json!(616)),
            mcp_2026_params(json!({
                "name": "tool_manifest",
                "arguments": {"tool_name": "plan_save"}
            })),
        ),
        None,
    )
    .await;
    let McpOutcome::Ok(value) = save else {
        panic!("tool_manifest must succeed for plan_save");
    };
    assert_eq!(
        value["result"]["structuredContent"]["output"]["route"]["mode"],
        "gateway",
        "plan_save remains reachable through the gateway when omitted from the compact initial list"
    );
    assert_eq!(
        value["result"]["structuredContent"]["output"]["route"]["via"],
        "call_runtime_tool"
    );
}

#[tokio::test]
async fn long_tail_direct_call_is_rejected_with_gateway_guidance() {
    let runtime = test_runtime();
    let outcome = handle_mcp_request(
        &runtime,
        rpc(
            "tools/call",
            Some(json!(62)),
            mcp_2026_params(json!({
                "name": "apply_patch",
                "arguments": {
                    "project": "missing-project",
                    "patch": "*** Begin Patch\n*** Add File: route-probe.txt\n+probe\n*** End Patch",
                    "dry_run": true
                }
            })),
        ),
        None,
    )
    .await;
    let McpOutcome::BadRequest(value) = outcome else {
        panic!("direct long-tail invocation must be rejected");
    };
    let message = value["error"]["message"].as_str().unwrap();
    assert!(message.contains("call_runtime_tool"), "{message}");
    assert!(
        message.contains("not directly callable on Adaptive Runtime"),
        "{message}"
    );
}

#[tokio::test]
async fn long_tail_and_direct_targets_are_both_admitted_through_gateway() {
    let runtime = test_runtime();
    for (id, tool, arguments) in [
        (
            63,
            "apply_patch",
            json!({
                "project": "missing-project",
                "patch": "*** Begin Patch\n*** Add File: route-probe.txt\n+probe\n*** End Patch",
                "dry_run": true
            }),
        ),
        (
            64,
            "read_files",
            json!({"project": "missing-project", "items": [{"path": "x"}]}),
        ),
        (
            65,
            "plan_wait_approval",
            json!({"plan_id": "wc_plan_abcdefghijkl", "wait_secs": 0}),
        ),
    ] {
        let outcome = handle_mcp_request(
            &runtime,
            rpc(
                "tools/call",
                Some(json!(id)),
                mcp_2026_params(json!({
                    "name": "call_runtime_tool",
                    "arguments": {"tool": tool, "arguments": arguments}
                })),
            ),
            None,
        )
        .await;
        assert!(
            matches!(outcome, McpOutcome::Ok(_)),
            "gateway must admit canonical target {tool}: {outcome:?}"
        );
    }
}

#[test]
fn hidden_protocol_extensions_require_protocol_admission() {
    assert!(!crate::tool_runtime::tool_definition::is_model_visible_tool_name("skill_list"));
    assert!(
        !crate::mcp::tools::adaptive_runtime_gateway_target_admitted_for_test("skill_list", false,)
    );
    assert!(
        crate::mcp::tools::adaptive_runtime_gateway_target_admitted_for_test("skill_list", true,)
    );
    assert!(
        !crate::mcp::tools::adaptive_runtime_gateway_target_admitted_for_test(
            "definitely_unknown_tool",
            true,
        )
    );
}

#[tokio::test]
async fn call_runtime_tool_cannot_target_itself() {
    let runtime = test_runtime();
    let outcome = handle_mcp_request(
        &runtime,
        rpc(
            "tools/call",
            Some(json!(65)),
            mcp_2026_params(json!({
                "name": "call_runtime_tool",
                "arguments": {
                    "tool": "call_runtime_tool",
                    "arguments": {"tool": "read_files", "arguments": {}}
                }
            })),
        ),
        None,
    )
    .await;
    let McpOutcome::BadRequest(value) = outcome else {
        panic!("recursive gateway call must be rejected");
    };
    assert!(value["error"]["message"]
        .as_str()
        .unwrap()
        .contains("cannot target itself"));
}
