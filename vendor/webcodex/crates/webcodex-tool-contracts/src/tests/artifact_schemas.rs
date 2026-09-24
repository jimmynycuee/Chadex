use super::*;

#[test]
fn read_project_artifact_uses_only_canonical_length_bound() {
    let specs = registered_tool_specs();
    let spec = spec_named(&specs, "read_project_artifact");
    let props = spec.input_schema["properties"].as_object().unwrap();
    assert!(props.contains_key("length"));
    let expected_sha256 = &props["expected_sha256"];
    assert_eq!(expected_sha256["type"], "string");
    assert_eq!(expected_sha256["minLength"], 64);
    assert_eq!(expected_sha256["maxLength"], 64);
    assert_eq!(expected_sha256["pattern"], "^[0-9a-f]{64}$");
    assert!(
        !props.contains_key("max_bytes"),
        "read_project_artifact must not advertise the retired max_bytes alias"
    );
}

#[test]
fn read_project_artifact_metadata_schema_exposes_allow_missing() {
    let specs = registered_tool_specs();
    let spec = spec_named(&specs, "read_project_artifact_metadata");
    let props = spec.input_schema["properties"].as_object().unwrap();
    assert!(props.contains_key("allow_missing"));
    assert!(
        spec.description.contains("allow_missing=true")
            && spec.description.contains("exists=false"),
        "description should explain successful missing assertions: {}",
        spec.description
    );
}

#[test]
fn artifact_upload_followup_descriptions_explain_required_path_binding() {
    let specs = registered_tool_specs();
    for name in [
        "artifact_upload_chunk",
        "artifact_upload_finish",
        "artifact_upload_abort",
    ] {
        let spec = spec_named(&specs, name);
        assert!(
            spec.description.contains("path is required")
                && spec.description.contains("artifact_upload_begin")
                && spec.description.contains("binds upload_id"),
            "{name}: {}",
            spec.description
        );
        let path_desc = spec.input_schema["properties"]["path"]["description"]
            .as_str()
            .unwrap();
        assert!(
            path_desc.contains("Required")
                && path_desc.contains("must exactly match the path used in artifact_upload_begin")
                && path_desc.contains("bind upload_id"),
            "{name}: {path_desc}"
        );
    }
}

#[test]
fn project_artifact_is_compact_typed_project_read_facade() {
    let definition =
        lookup_tool_definition("project_artifact").expect("project_artifact definition");
    assert_eq!(definition.metadata.effect, ToolEffect::Observe);
    assert_eq!(definition.metadata.risk, ToolRisk::Read);
    assert_eq!(definition.metadata.approval, ToolApprovalPolicy::None);
    assert_eq!(definition.metadata.idempotency, ToolIdempotency::PureRead);
    assert_eq!(
        definition.metadata.authority,
        ToolAuthorityPolicy::Require(PROJECT_READ)
    );
    assert!(!definition.requires_permission());

    let specs = registered_tool_specs();
    let spec = spec_named(&specs, "project_artifact");
    let props = spec.input_schema["properties"].as_object().unwrap();
    assert_eq!(spec.input_schema["additionalProperties"], false);
    assert_eq!(
        spec.input_schema["required"],
        json!(["project", "path", "action"])
    );
    assert_eq!(
        props["action"]["enum"],
        json!(["metadata", "inspect", "image", "export"])
    );
    assert!(!props.contains_key("encoding"));
    assert_eq!(spec.input_schema["allOf"].as_array().unwrap().len(), 2);
    let output_props = spec.output_schema["properties"]["output"]["properties"]
        .as_object()
        .expect("project_artifact output properties");
    for field in [
        "path",
        "exists",
        "bytes",
        "file_bytes",
        "sha256",
        "mime_type",
        "content_base64",
        "content_delivery",
        "suggested_call",
    ] {
        assert!(
            output_props.contains_key(field),
            "missing output field {field}"
        );
    }
    let suggested = &output_props["suggested_call"];
    assert_eq!(suggested["properties"]["tool"]["const"], "project_artifact");
    assert_eq!(
        suggested["properties"]["arguments"]["properties"]["action"]["const"],
        "inspect"
    );
    assert_eq!(
        suggested["properties"]["arguments"]["required"],
        json!([
            "project",
            "path",
            "action",
            "offset",
            "length",
            "expected_sha256"
        ])
    );
    assert!(spec.description.contains("not repeated inspect"));
    assert!(spec
        .description
        .contains("import_conversation_files_to_project"));
}
