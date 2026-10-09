use super::*;

fn action_branch<'a>(schema: &'a Value, action: &str) -> &'a Value {
    schema["oneOf"]
        .as_array()
        .expect("Computer gateway oneOf")
        .iter()
        .find(|branch| branch["properties"]["action"]["const"] == action)
        .unwrap_or_else(|| panic!("missing Computer action branch {action}"))
}

fn action_properties<'a>(schema: &'a Value, action: &str) -> &'a serde_json::Map<String, Value> {
    action_branch(schema, action)["properties"]
        .as_object()
        .expect("Computer action properties")
}

#[test]
fn tool_specs_generic_sync_wait_is_runtime_clamped_and_scoped_to_process_and_script() {
    let specs = registered_tool_specs();
    for name in ["run_process", "run_script"] {
        let spec = spec_named(&specs, name);
        let props = spec.input_schema["properties"].as_object().unwrap();
        let sync_wait = &props["sync_wait_secs"];
        assert_eq!(sync_wait["type"], "integer", "{name}");
        assert_eq!(sync_wait["minimum"], 1, "{name}");
        assert!(sync_wait.get("maximum").is_none(), "{name}");
        assert!(!required_fields(spec).contains(&"sync_wait_secs".to_string()));
    }
}

#[test]
fn computer_primary_surface_is_three_canonical_tools() {
    let names = registered_tool_specs()
        .into_iter()
        .filter(|spec| spec.name.starts_with("computer_"))
        .map(|spec| spec.name)
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        vec![
            "computer_observe".to_string(),
            "computer_control".to_string(),
            "computer_save_snapshot".to_string(),
        ]
    );
}

#[test]
fn computer_observe_schema_is_closed_read_only_action_union() {
    let specs = registered_tool_specs();
    let spec = spec_named(&specs, "computer_observe");
    assert_eq!(spec.annotations["readOnlyHint"], true);
    assert_eq!(spec.annotations["destructiveHint"], false);
    let expected = [
        "targets",
        "windows",
        "displays",
        "applications",
        "readiness",
        "accessibility_status",
        "accessibility_tree",
        "find_elements",
        "element_state",
        "snapshot_window",
        "snapshot_display",
        "read_clipboard",
    ];
    let branches = spec.input_schema["oneOf"].as_array().unwrap();
    assert_eq!(branches.len(), expected.len());
    for action in expected {
        let branch = action_branch(&spec.input_schema, action);
        assert_eq!(branch["additionalProperties"], false, "{action}");
        assert!(branch["required"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field == "action"));
    }

    let readiness = action_properties(&spec.input_schema, "readiness");
    assert_schema_fields!(
        readiness,
        "readiness action",
        present: ["action", "client_id"],
        absent: ["surface_id", "display_id", "text", "application_id"]
    );

    let find = action_properties(&spec.input_schema, "find_elements");
    assert_schema_fields!(
        find,
        "find_elements action",
        present: ["action", "client_id", "surface_id", "root_element_id", "role", "subrole", "label", "value", "focused", "enabled", "limit", "max_depth"],
        absent: ["text", "application_id", "display_id"]
    );
    assert_eq!(find["limit"]["minimum"], 1);
    assert_eq!(find["max_depth"]["minimum"], 1);
    assert_eq!(find["value"]["maxLength"], 256);
    assert_eq!(find["root_element_id"]["minLength"], 9);
    assert_eq!(find["root_element_id"]["maxLength"], 128);

    let tree = action_properties(&spec.input_schema, "accessibility_tree");
    assert_schema_fields!(
        tree,
        "accessibility_tree action",
        present: ["action", "client_id", "surface_id", "root_element_id", "max_depth", "max_nodes"],
        absent: ["role", "value", "label", "limit", "element_id"]
    );
    assert_eq!(tree["root_element_id"]["minLength"], 9);

    let snapshot = action_properties(&spec.input_schema, "snapshot_window");
    assert_schema_fields!(
        snapshot,
        "snapshot_window action",
        present: ["action", "client_id", "surface_id", "region", "max_width", "max_height"],
        absent: ["display_id", "format", "quality", "save"]
    );
    assert_eq!(snapshot["region"]["additionalProperties"], false);

    let display = action_properties(&spec.input_schema, "snapshot_display");
    assert_schema_fields!(
        display,
        "snapshot_display action",
        present: ["action", "client_id", "display_id", "max_width", "max_height"],
        absent: ["surface_id", "region", "x", "y", "pointer", "click"]
    );

    for value in [
        json!({"action":"targets"}),
        json!({"action":"readiness","client_id":"special"}),
        json!({"action":"windows","client_id":"special","limit":9999}),
        json!({"action":"snapshot_window","client_id":"special","surface_id":"surface_test","max_width":10000}),
        json!({"action":"snapshot_display","client_id":"special","display_id":"display_iavN7wEjRWeJq83v","max_height":u32::MAX}),
        json!({"action":"read_clipboard","client_id":"special"}),
        json!({"action":"accessibility_tree","client_id":"special","surface_id":"surface_test","root_element_id":"element_abc123","max_depth":4,"max_nodes":64}),
        json!({"action":"find_elements","client_id":"special","surface_id":"surface_test","value":"needle","root_element_id":"element_abc123","max_depth":99,"limit":5}),
        json!({"action":"find_elements","client_id":"special","surface_id":"surface_test","value":"needle"}),
    ] {
        test_support::validate_schema_instance(&value, &spec.input_schema).unwrap();
    }
    for invalid in [
        json!({"action":"unknown","client_id":"special"}),
        json!({"action":"windows"}),
        json!({"action":"windows","client_id":"special","text":"nope"}),
        json!({"action":"snapshot_display","client_id":"special","display_id":"display_iavN7wEjRWeJq83v","surface_id":"surface_nope"}),
        json!({"action":"accessibility_tree","client_id":"special","surface_id":"surface_test","root_element_id":7}),
        json!({"action":"accessibility_tree","client_id":"special","surface_id":"surface_test","value":"nope"}),
        json!({"action":"find_elements","client_id":"special","surface_id":"surface_test","value":1}),
        json!({"action":"find_elements","client_id":"special","surface_id":"surface_test","value":"x","max_depth":"deep"}),
        json!({"action":"find_elements","client_id":"special","surface_id":"surface_test","value":"x","max_depth":0}),
        json!({"action":"find_elements","client_id":"special","surface_id":"surface_test","value":"x","root_element_id":["element_a"]}),
        json!({"action":"windows","client_id":"special","root_element_id":"element_abc123"}),
    ] {
        assert!(
            test_support::validate_schema_instance(&invalid, &spec.input_schema).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn computer_control_schema_is_closed_action_union() {
    let specs = registered_tool_specs();
    let spec = spec_named(&specs, "computer_control");
    assert_eq!(spec.annotations["readOnlyHint"], false);
    assert_eq!(spec.annotations["destructiveHint"], true);
    let expected = [
        "launch_application",
        "activate_window",
        "press",
        "focus",
        "scroll_to_element",
        "key",
        "input_text",
        "pointer_move",
        "pointer_click",
        "write_clipboard",
    ];
    let branches = spec.input_schema["oneOf"].as_array().unwrap();
    assert_eq!(branches.len(), expected.len());
    for action in expected {
        let branch = action_branch(&spec.input_schema, action);
        assert_eq!(branch["additionalProperties"], false, "{action}");
    }

    let launch = action_properties(&spec.input_schema, "launch_application");
    assert_schema_fields!(
        launch,
        "launch_application action",
        present: ["action", "client_id", "application_id"],
        absent: ["path", "argv", "cwd", "environment", "command", "script", "url"]
    );
    let pointer = action_properties(&spec.input_schema, "pointer_click");
    assert_schema_fields!(
        pointer,
        "pointer_click action",
        present: ["action", "client_id", "display_id", "snapshot_generation", "x", "y"],
        absent: ["surface_id", "global_x", "global_y", "button", "double_click"]
    );
    assert_eq!(pointer["snapshot_generation"]["minimum"], 1);
    let write = action_properties(&spec.input_schema, "write_clipboard");
    assert_eq!(write["text"]["minLength"], 1);
    assert_eq!(write["text"]["maxLength"], 16384);

    for value in [
        json!({"action":"launch_application","client_id":"special","application_id":"application_iavN7wEjRWeJq83v"}),
        json!({"action":"press","client_id":"special","surface_id":"surface_test","element_id":"element_test"}),
        json!({"action":"key","client_id":"special","surface_id":"surface_test","key":"enter"}),
        json!({"action":"pointer_click","client_id":"special","display_id":"display_iavN7wEjRWeJq83v","snapshot_generation":3,"x":400,"y":220}),
        json!({"action":"write_clipboard","client_id":"special","text":"hello"}),
    ] {
        test_support::validate_schema_instance(&value, &spec.input_schema).unwrap();
    }
    for invalid in [
        json!({"action":"pointer_click","client_id":"special","display_id":"display_iavN7wEjRWeJq83v","snapshot_generation":0,"x":1,"y":1}),
        json!({"action":"launch_application","client_id":"special","application_id":"application_iavN7wEjRWeJq83v","argv":["--unsafe"]}),
        json!({"action":"write_clipboard","client_id":"special"}),
        json!({"action":"targets"}),
    ] {
        assert!(
            test_support::validate_schema_instance(&invalid, &spec.input_schema).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn computer_gateway_outputs_cover_preserved_observation_and_control_shapes() {
    let specs = registered_tool_specs();
    let observe = spec_named(&specs, "computer_observe");
    let observe_output = observe.output_schema["properties"]["output"]["properties"]
        .as_object()
        .unwrap();
    for field in [
        "targets",
        "windows",
        "displays",
        "applications",
        "nodes",
        "elements",
        "observation_generation",
        "available",
        "text",
        "snapshot_generation",
        "content_base64",
        "suggested_call",
        "reconcile_with",
    ] {
        assert!(
            observe_output.contains_key(field),
            "observe output missing {field}"
        );
    }
    assert_eq!(
        observe.output_schema["properties"]["output"]["additionalProperties"],
        false
    );

    let control = spec_named(&specs, "computer_control");
    let control_output = control.output_schema["properties"]["output"]["properties"]
        .as_object()
        .unwrap();
    for field in [
        "application_id",
        "surface_id",
        "element_id",
        "display_id",
        "snapshot_generation",
        "x",
        "y",
        "text_bytes",
        "success",
        "execution_state",
        "state_changed",
        "suggested_call",
        "reconcile_with",
    ] {
        assert!(
            control_output.contains_key(field),
            "control output missing {field}"
        );
    }
    assert_eq!(
        control.output_schema["properties"]["output"]["additionalProperties"],
        false
    );
}

fn sample_found_element(depth: Value, ancestors: Value) -> Value {
    json!({
        "element_id": "element_abc",
        "role": "AXButton",
        "subrole": null,
        "title": "Share",
        "description": null,
        "placeholder": null,
        "enabled": true,
        "focused": false,
        "depth": depth,
        "ancestors": ancestors
    })
}

#[test]
fn computer_observe_output_schema_accepts_deep_find_subtree_and_tree_filter_shapes() {
    let specs = registered_tool_specs();
    let schema = &spec_named(&specs, "computer_observe").output_schema;
    let wrap = |output: Value| json!({"success": true, "output": output});

    let deep = json!({
        "platform": "macos",
        "surface_id": "surface_test",
        "observation_generation": 4,
        "search_mode": "deep",
        "root_element_id": "element_root",
        "web_accessibility": "enabled",
        "elements": [sample_found_element(json!(17), json!("AXWebArea “Page” › AXGroup"))],
        "count": 1,
        "scanned_nodes": 4000,
        "truncated": true,
        "stop_reason": "visit_budget"
    });
    let fallback = json!({
        "platform": "windows",
        "surface_id": "surface_test",
        "observation_generation": 4,
        "search_mode": "tree_filter",
        "root_element_id": null,
        "web_accessibility": null,
        "elements": [sample_found_element(Value::Null, Value::Null)],
        "count": 1,
        "scanned_nodes": 256,
        "truncated": false,
        "stop_reason": null
    });
    let legacy_find = json!({
        "platform": "macos",
        "surface_id": "surface_test",
        "observation_generation": 4,
        "elements": [],
        "count": 0,
        "scanned_nodes": 12,
        "truncated": false
    });
    let node = json!({
        "element_id": "element_root",
        "parent_element_id": null,
        "depth": 0,
        "role": "AXWebArea",
        "subrole": null,
        "title": "Page",
        "description": null,
        "value": null,
        "placeholder": null,
        "enabled": true,
        "focused": false,
        "child_count": 0
    });
    let subtree = json!({
        "platform": "macos",
        "surface_id": "surface_test",
        "observation_generation": 5,
        "nodes": [node],
        "node_count": 1,
        "truncated": false,
        "max_depth": 4,
        "max_nodes": 64,
        "root": {"element_id": "element_root", "absolute_depth": 14},
        "web_accessibility": "pending"
    });
    let legacy_tree = json!({
        "platform": "macos",
        "surface_id": "surface_test",
        "observation_generation": 5,
        "nodes": [node],
        "node_count": 1,
        "truncated": false,
        "max_depth": 4,
        "max_nodes": 64
    });
    for (label, output) in [
        ("deep find", deep.clone()),
        ("tree_filter fallback", fallback),
        ("legacy find (older server shape)", legacy_find),
        ("subtree", subtree.clone()),
        ("legacy tree", legacy_tree),
    ] {
        test_support::validate_schema_instance(&wrap(output), schema)
            .unwrap_or_else(|error| panic!("{label}: {error}"));
    }

    let mut bad_state = deep.clone();
    bad_state["web_accessibility"] = json!("on");
    let mut bad_reason = deep.clone();
    bad_reason["stop_reason"] = json!("whenever");
    let mut too_deep = deep.clone();
    too_deep["elements"][0]["depth"] = json!(65);
    let mut leaked_value = deep.clone();
    leaked_value["elements"][0]["value"] = json!("secret");
    let mut bad_root = subtree.clone();
    bad_root["root"]["extra"] = json!(1);
    for (label, output) in [
        ("unknown web state", bad_state),
        ("unknown stop reason", bad_reason),
        ("depth above 64", too_deep),
        ("value on found element", leaked_value),
        ("root extra key", bad_root),
    ] {
        assert!(
            test_support::validate_schema_instance(&wrap(output), schema).is_err(),
            "{label}"
        );
    }
}

#[test]
fn computer_save_snapshot_remains_separate_create_only_project_write() {
    let specs = registered_tool_specs();
    let spec = spec_named(&specs, "computer_save_snapshot");
    assert_eq!(
        required_fields(spec),
        vec![
            "project".to_string(),
            "path".to_string(),
            "client_id".to_string(),
            "surface_id".to_string(),
        ]
    );
    let props = spec.input_schema["properties"].as_object().unwrap();
    assert_schema_fields!(
        props,
        "computer_save_snapshot input schema",
        present: ["project", "path", "client_id", "surface_id", "region", "max_width", "max_height", "session_id"],
        absent: ["overwrite", "format", "quality", "mime_type", "content_base64", "save"]
    );
    assert_eq!(props["region"]["additionalProperties"], false);
    let output = spec.output_schema["properties"]["output"]["properties"]
        .as_object()
        .unwrap();
    assert_schema_fields!(
        output,
        "computer_save_snapshot output schema",
        present: ["project", "path", "client_id", "surface_id", "source_width", "source_height", "region", "width", "height", "mime_type", "file_bytes", "sha256", "saved"],
        absent: ["content_base64", "captured_at_unix_ms"]
    );
}

/// `computer_observe` input schema size before the Chromium web accessibility fields
/// (`root_element_id`, `value`, `find_elements.max_depth`) were added: 4175 bytes. The
/// guard leaves 1024 bytes of headroom over it so further growth is a conscious choice.
const COMPUTER_OBSERVE_INPUT_SCHEMA_BASELINE_BYTES: usize = 4175;

#[test]
fn computer_observe_model_schema_size_is_bounded() {
    let specs = registered_tool_specs();
    let spec = spec_named(&specs, "computer_observe");
    let input_schema_bytes = serde_json::to_vec(&spec.input_schema).unwrap().len();
    let description_chars = spec.description.chars().count();
    eprintln!(
        "computer_observe surface: input_schema={input_schema_bytes} description={description_chars}"
    );
    assert!(
        input_schema_bytes <= COMPUTER_OBSERVE_INPUT_SCHEMA_BASELINE_BYTES + 1024,
        "computer_observe input schema grew to {input_schema_bytes} bytes"
    );
    // Gateway branches strip per-field prose, so the new fields are explained by one
    // short sentence in the tool description, which must stay under the model limit.
    assert!(description_chars <= MODEL_TOOL_DESCRIPTION_MAX_CHARS);
    assert!(spec.description.contains("root_element_id"));
    assert!(spec.description.contains("AXValue (never returned)"));
}
