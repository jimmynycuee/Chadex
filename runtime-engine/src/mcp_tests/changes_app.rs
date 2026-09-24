use super::*;

fn tool<'a>(payload: &'a Value, name: &str) -> Option<&'a Value> {
    payload["tools"]
        .as_array()?
        .iter()
        .find(|tool| tool["name"] == name)
}

async fn handle_with_server_apps_enabled(
    runtime: &ToolRuntime,
    request: JsonRpcRequest,
    auth: Option<&crate::auth::AuthContext>,
    enabled: bool,
) -> McpOutcome {
    let protocol_era = super::super::inferred_protocol_era(&request);
    super::super::handle_mcp_request_with_lifecycle(
        runtime,
        request,
        auth,
        protocol_era,
        super::super::HostFileImportTrust::Untrusted,
        None,
        None,
        None,
        crate::model_surface::effective_mcp_compact_schemas(
            crate::config::mcp_compact_schemas_override(),
        ),
        enabled,
        None,
    )
    .await
}

#[tokio::test]
async fn final_changes_descriptor_is_explicit_v3_and_lazy_diff_is_app_only() {
    assert_eq!(MCP_CHANGES_UI_RESOURCE_URI, "ui://webcodex/changes/v3");
    let runtime = test_runtime();

    let ui = handle_with_server_apps_enabled(
        &runtime,
        rpc(
            "tools/list",
            Some(json!(5201)),
            mcp_2026_ui_params(json!({})),
        ),
        None,
        true,
    )
    .await;
    let McpOutcome::Ok(ui) = ui else {
        panic!("expected UI-capable adaptive tools/list");
    };
    let present = tool(&ui["result"], "present_changes").expect("present_changes");
    assert_eq!(
        present.pointer("/_meta/ui/resourceUri"),
        Some(&json!(MCP_CHANGES_UI_RESOURCE_URI))
    );
    assert!(present.pointer("/_meta/ui/visibility").is_none());
    let diff = tool(&ui["result"], "changes_file_diff").expect("app-only changes_file_diff");
    assert_eq!(diff.pointer("/_meta/ui/visibility"), Some(&json!(["app"])));
    assert!(diff.pointer("/_meta/ui/resourceUri").is_none());
    assert_eq!(
        diff["inputSchema"]["required"],
        json!(["project", "session_id", "snapshot_id", "path"])
    );

    for name in [
        "show_changes",
        "finish_coding_task",
        "list_jobs",
        "observe_jobs",
        "cargo_check",
        "cargo_test",
    ] {
        let descriptor = tool(&ui["result"], name).unwrap_or_else(|| panic!("missing {name}"));
        assert!(
            descriptor.pointer("/_meta/ui/resourceUri").is_none(),
            "{name} must not create a Final Changes card"
        );
    }

    let plain = handle_with_server_apps_enabled(
        &runtime,
        rpc("tools/list", Some(json!(5202)), mcp_2026_params(json!({}))),
        None,
        true,
    )
    .await;
    let McpOutcome::Ok(plain) = plain else {
        panic!("ordinary tools/list failed");
    };
    assert!(tool(&plain["result"], "present_changes").is_some());
    assert!(tool(&plain["result"], "present_changes")
        .unwrap()
        .pointer("/_meta/ui/resourceUri")
        .is_none());
    assert!(tool(&plain["result"], "changes_file_diff").is_none());

    let disabled = handle_with_server_apps_enabled(
        &runtime,
        rpc(
            "tools/list",
            Some(json!(5203)),
            mcp_2026_ui_params(json!({})),
        ),
        None,
        false,
    )
    .await;
    let McpOutcome::Ok(disabled) = disabled else {
        panic!("Apps-disabled tools/list failed");
    };
    assert!(tool(&disabled["result"], "changes_file_diff").is_none());
    assert!(tool(&disabled["result"], "present_changes")
        .unwrap()
        .pointer("/_meta/ui/resourceUri")
        .is_none());

    assert!(!registered_tool_specs()
        .iter()
        .any(|spec| spec.name == "changes_file_diff"));
    assert!(
        !super::super::tools::adaptive_runtime_gateway_target_admitted_for_test(
            "changes_file_diff",
            true
        )
    );
}

#[tokio::test]
async fn final_changes_resource_is_advertised_v3_while_legacy_changes_resources_stay_hidden() {
    const PUBLIC_URL: &str = "https://self-host.example";
    let runtime = test_runtime_with_public_url(PUBLIC_URL);
    let resources = handle_with_server_apps_enabled(
        &runtime,
        rpc(
            "resources/list",
            Some(json!(5210)),
            mcp_2026_ui_params(json!({})),
        ),
        None,
        true,
    )
    .await;
    let McpOutcome::Ok(resources) = resources else {
        panic!("resources/list failed");
    };
    let resources = resources["result"]["resources"].as_array().unwrap();
    let changes_resource = resources
        .iter()
        .find(|resource| resource["uri"] == MCP_CHANGES_UI_RESOURCE_URI)
        .expect("Final Changes V3 resource");
    assert!(changes_resource["description"]
        .as_str()
        .unwrap()
        .contains("frozen"));
    assert!(!resources
        .iter()
        .any(|resource| resource["uri"] == MCP_RESULT_UI_RESOURCE_URI));
    for legacy in MCP_RESULT_UI_RESOURCE_LEGACY_URIS {
        assert!(!resources.iter().any(|resource| resource["uri"] == *legacy));
    }

    let read = handle_with_server_apps_enabled(
        &runtime,
        rpc(
            "resources/read",
            Some(json!(5211)),
            mcp_2026_ui_params(json!({"uri": MCP_CHANGES_UI_RESOURCE_URI})),
        ),
        None,
        true,
    )
    .await;
    let McpOutcome::Ok(read) = read else {
        panic!("Changes resource read failed");
    };
    assert_eq!(read["result"]["contents"][0]["text"], MCP_CHANGES_APP_HTML);
    assert_eq!(
        read["result"]["contents"][0]["_meta"]["ui"]["domain"],
        PUBLIC_URL
    );
}

#[tokio::test]
async fn changes_file_diff_call_requires_app_protocol_capability() {
    let runtime = test_runtime();
    let args = json!({
        "name": "changes_file_diff",
        "arguments": {
            "project": "agent:missing:project",
            "session_id": format!("wc_sess_{}", "1".repeat(32)),
            "snapshot_id": format!("wc_changes_snapshot_{}", "2".repeat(32)),
            "path": "src/lib.rs"
        }
    });
    let app = handle_with_server_apps_enabled(
        &runtime,
        rpc(
            "tools/call",
            Some(json!(5220)),
            mcp_2026_ui_params(args.clone()),
        ),
        None,
        true,
    )
    .await;
    let McpOutcome::Ok(app) = app else {
        panic!("App-only diff call should reach runtime under App capability");
    };
    assert_eq!(app["result"]["structuredContent"]["success"], false);

    for params in [mcp_2026_params(args.clone()), mcp_2026_ui_params(args)] {
        let outcome = handle_with_server_apps_enabled(
            &runtime,
            rpc("tools/call", Some(json!(5221)), params),
            None,
            false,
        )
        .await;
        assert!(matches!(outcome, McpOutcome::BadRequest(_)));
    }
}

#[tokio::test]
async fn changes_file_diff_discards_unadvertised_recording_session_wrapper() {
    let runtime = test_runtime();
    let project = "agent:missing:changes".to_string();
    let session = runtime.sessions.start_session(
        Some(project.clone()),
        Some("Changes wrapper suppression".to_string()),
    );
    let before = runtime.sessions.summary(&session.session_id, None).unwrap();

    let outcome = handle_with_server_apps_enabled(
        &runtime,
        rpc(
            "tools/call",
            Some(json!(5222)),
            mcp_2026_ui_params(json!({
                "name": "changes_file_diff",
                "arguments": {
                    "project": project,
                    "session_id": session.session_id,
                    "snapshot_id": format!("wc_changes_snapshot_{}", "3".repeat(32)),
                    "path": "src/lib.rs",
                    "recording_session_id": session.session_id
                }
            })),
        ),
        None,
        true,
    )
    .await;
    assert!(matches!(
        outcome,
        McpOutcome::Ok(_) | McpOutcome::BadRequest(_)
    ));

    let after = runtime.sessions.summary(&session.session_id, None).unwrap();
    assert_eq!(after.events_total, before.events_total);
    assert_eq!(after.events.len(), before.events.len());
    assert_eq!(after.updated_at, before.updated_at);
}

#[test]
fn final_changes_html_is_bounded_lazy_display_only_ui() {
    for required in [
        "changes_file_diff",
        "ui/notifications/tool-input",
        "ui/notifications/tool-result",
        "wc_changes_snapshot_",
        "Frozen final workspace snapshot",
        "Show ${snapshot.files.length - visibleCount} more files",
        "ui/resource-teardown",
        "pagehide",
        "beforeunload",
        "WebCodex Changes",
    ] {
        assert!(
            MCP_CHANGES_APP_HTML.contains(required),
            "missing {required}"
        );
    }
    for forbidden in [
        "setInterval",
        "clearInterval",
        "visibilitychange",
        "localStorage",
        "indexedDB",
        "fetch(",
        "WebSocket",
        "baseline_tree",
        "final_tree",
        "authority_fingerprint",
    ] {
        assert!(
            !MCP_CHANGES_APP_HTML.contains(forbidden),
            "Final Changes App contains forbidden marker {forbidden}"
        );
    }
}
