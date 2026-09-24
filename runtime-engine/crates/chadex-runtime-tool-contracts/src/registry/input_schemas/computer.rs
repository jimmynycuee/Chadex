use super::common::OPTIONAL_EXPLICIT_SESSION_ID_DESCRIPTION;
use serde_json::{json, Value};

pub fn computer_list_windows_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact Runner client_id whose desktop is observed."},
            "limit": {"type": "integer", "minimum": 1, "description": "Optional bounded window count; values above 64 are accepted and clamped to 64."}
        },
        "required": ["client_id"]
    })
}

pub fn computer_list_displays_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact Runner client_id whose full displays are observed."},
            "limit": {"type": "integer", "minimum": 1, "description": "Optional bounded display count; defaults to 16 and values above 16 are accepted and clamped to 16."}
        },
        "required": ["client_id"]
    })
}

pub fn computer_list_applications_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact macOS or Windows Runner client_id whose installed applications are discovered."},
            "limit": {"type": "integer", "minimum": 1, "description": "Optional bounded application count; defaults to 64 and values above 64 are accepted and clamped to 64."}
        },
        "required": ["client_id"]
    })
}

pub fn computer_launch_application_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact macOS or Windows Runner client_id that produced the application_id."},
            "application_id": {"type": "string", "pattern": "^application_[A-Za-z0-9_-]{16}$", "maxLength": 128, "description": "Fresh opaque process-local application_id returned by computer_observe(action=applications)."}
        },
        "required": ["client_id", "application_id"]
    })
}

pub fn computer_accessibility_status_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact Runner client_id whose Accessibility trust is queried without prompting."}
        },
        "required": ["client_id"]
    })
}

pub fn computer_accessibility_tree_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact Runner client_id whose desktop is inspected."},
            "surface_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Opaque process-local surface_id returned by computer_observe(action=windows)."},
            "max_depth": {"type": "integer", "minimum": 0, "description": "Maximum AX descendant depth; values above 8 are accepted and clamped to 8."},
            "max_nodes": {"type": "integer", "minimum": 1, "description": "Maximum semantic AX elements returned; values above 256 are accepted and clamped to 256."}
        },
        "required": ["client_id", "surface_id"]
    })
}

pub fn computer_find_elements_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact Runner client_id whose Accessibility surface is searched."},
            "surface_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Opaque process-local surface_id returned by computer_observe(action=windows)."},
            "role": {"type": "string", "minLength": 1, "maxLength": 256, "description": "Optional exact Accessibility role match."},
            "subrole": {"type": "string", "minLength": 1, "maxLength": 256, "description": "Optional exact Accessibility subrole match."},
            "label": {"type": "string", "minLength": 1, "maxLength": 256, "description": "Optional case-sensitive literal substring matched only against title, description, or placeholder; AXValue is never searched."},
            "focused": {"type": "boolean", "description": "Optional exact focused-state match; unknown/null state does not match."},
            "enabled": {"type": "boolean", "description": "Optional exact enabled-state match; unknown/null state does not match."},
            "limit": {"type": "integer", "minimum": 1, "description": "Maximum matching elements returned; defaults to 8 and values above 32 are accepted and clamped to 32."}
        },
        "required": ["client_id", "surface_id"]
    })
}

pub fn computer_element_state_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact Runner client_id whose observed element is revalidated."},
            "surface_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact opaque process-local surface_id that owns the element."},
            "element_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact ephemeral element_id returned by computer_observe(action=accessibility_tree) or computer_observe(action=find_elements)."}
        },
        "required": ["client_id", "surface_id", "element_id"]
    })
}

pub fn computer_activate_window_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact Runner client_id whose already-observed window is activated."},
            "surface_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact opaque process-local surface_id returned by computer_observe(action=windows)."}
        },
        "required": ["client_id", "surface_id"]
    })
}

fn computer_element_control_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact Runner client_id whose macOS Accessibility element is controlled."},
            "surface_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact opaque surface_id used to obtain the element."},
            "element_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Opaque process-local element_id returned by computer_observe(action=accessibility_tree) or computer_observe(action=find_elements)."},
            "action": {"type": "string", "enum": ["press", "focus"], "description": "Bounded control action. CU-AX2 supports only press and focus."}
        },
        "required": ["client_id", "surface_id", "element_id", "action"]
    })
}

pub fn computer_scroll_to_element_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact Runner client_id whose macOS Accessibility or Windows UIA element is scrolled into view."},
            "surface_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact opaque surface_id used to obtain the element."},
            "element_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Opaque process-local element_id returned by computer_observe(action=accessibility_tree) or computer_observe(action=find_elements)."}
        },
        "required": ["client_id", "surface_id", "element_id"]
    })
}

pub fn computer_key_input_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact Runner client_id whose already-focused macOS or Windows window receives the key."},
            "surface_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact opaque process-local surface_id that must still be the frontmost focused window."},
            "key": {"type": "string", "enum": ["enter", "escape", "tab", "arrow_up", "arrow_down", "arrow_left", "arrow_right", "page_up", "page_down", "home", "end"], "description": "Closed navigation/action key vocabulary. Ordinary text must use computer_control(action=input_text)."},
            "modifiers": {"type": "array", "maxItems": 4, "uniqueItems": true, "items": {"type": "string", "enum": ["shift", "control", "option", "command"]}, "description": "Optional bounded modifier set for this call only. On Windows, option maps to Alt and command fails closed before input."}
        },
        "required": ["client_id", "surface_id", "key"]
    })
}

pub fn computer_read_clipboard_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact macOS or Windows Runner whose global plain Unicode-text clipboard is observed."}
        },
        "required": ["client_id"]
    })
}

pub fn computer_write_clipboard_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact macOS or Windows Runner whose global clipboard is replaced with plain Unicode text."},
            "text": {"type": "string", "minLength": 1, "maxLength": 16384, "description": "Unicode text replacement. Runtime enforces non-empty, NUL-free UTF-8 of at most 16 KiB."}
        },
        "required": ["client_id", "text"]
    })
}

pub fn computer_input_text_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact Runner client_id whose already-focused macOS Accessibility text element is mutated."},
            "surface_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact opaque surface_id used to obtain the element."},
            "element_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Opaque process-local element_id returned by computer_observe(action=accessibility_tree) or computer_observe(action=find_elements)."},
            "text": {"type": "string", "minLength": 1, "maxLength": 2048, "description": "Caller text written verbatim with AXValue. Runtime enforces a 2048-byte UTF-8 ceiling and rejects NUL; the target must already be focused and empty."}
        },
        "required": ["client_id", "surface_id", "element_id", "text"]
    })
}

pub fn computer_snapshot_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact Runner client_id whose desktop is observed."},
            "surface_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Opaque process-local surface_id returned by computer_observe(action=windows)."},
            "region": {
                "type": "object",
                "additionalProperties": false,
                "description": "Optional rectangle in the revalidated surface coordinate space. It must fit fully inside the exact surface.",
                "properties": {
                    "x": {"type": "integer", "minimum": 0, "maximum": 4294967295u64},
                    "y": {"type": "integer", "minimum": 0, "maximum": 4294967295u64},
                    "width": {"type": "integer", "minimum": 1, "maximum": 4294967295u64},
                    "height": {"type": "integer", "minimum": 1, "maximum": 4294967295u64}
                },
                "required": ["x", "y", "width", "height"]
            },
            "max_width": {"type": "integer", "minimum": 1, "description": "Optional upper bound on encoded output width. Values above 4096 are clamped to 4096. Never upscales."},
            "max_height": {"type": "integer", "minimum": 1, "description": "Optional upper bound on encoded output height. Values above 4096 are clamped to 4096. Never upscales."}
        },
        "required": ["client_id", "surface_id"]
    })
}

pub fn computer_snapshot_display_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact Runner client_id that produced the display_id."},
            "display_id": {"type": "string", "pattern": "^display_[A-Za-z0-9_-]{16}$", "maxLength": 128, "description": "Fresh opaque process-local display_id returned by computer_observe(action=displays)."},
            "max_width": {"type": "integer", "minimum": 1, "description": "Optional upper bound on encoded output width. Values above 4096 are clamped to 4096. Never upscales."},
            "max_height": {"type": "integer", "minimum": 1, "description": "Optional upper bound on encoded output height. Values above 4096 are clamped to 4096. Never upscales."}
        },
        "required": ["client_id", "display_id"]
    })
}

pub fn computer_pointer_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact Runner client_id that produced the display snapshot."},
            "display_id": {"type": "string", "pattern": "^display_[A-Za-z0-9_-]{16}$", "maxLength": 128, "description": "Exact opaque process-local display_id bound to snapshot_generation."},
            "snapshot_generation": {"type": "integer", "minimum": 1, "maximum": 4294967295u64, "description": "Latest unspent successful full-display snapshot generation for this display."},
            "x": {"type": "integer", "minimum": 0, "maximum": 4294967295u64, "description": "Display-local source-space x coordinate; must be less than the bound snapshot source_width."},
            "y": {"type": "integer", "minimum": 0, "maximum": 4294967295u64, "description": "Display-local source-space y coordinate; must be less than the bound snapshot source_height."}
        },
        "required": ["client_id", "display_id", "snapshot_generation", "x", "y"]
    })
}

pub fn computer_save_snapshot_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "project": {"type": "string", "minLength": 1, "description": "Target project that will receive the create-only snapshot artifact."},
            "path": {"type": "string", "minLength": 1, "maxLength": 4096, "description": "Project-relative artifact path. The first version is create-only and never overwrites an existing file."},
            "client_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Exact Runner client_id whose desktop is observed."},
            "surface_id": {"type": "string", "minLength": 1, "maxLength": 128, "description": "Opaque process-local surface_id returned by computer_observe(action=windows)."},
            "region": {
                "type": "object",
                "additionalProperties": false,
                "description": "Optional rectangle in the revalidated surface coordinate space. It must fit fully inside the exact surface.",
                "properties": {
                    "x": {"type": "integer", "minimum": 0, "maximum": 4294967295u64},
                    "y": {"type": "integer", "minimum": 0, "maximum": 4294967295u64},
                    "width": {"type": "integer", "minimum": 1, "maximum": 4294967295u64},
                    "height": {"type": "integer", "minimum": 1, "maximum": 4294967295u64}
                },
                "required": ["x", "y", "width", "height"]
            },
            "max_width": {"type": "integer", "minimum": 1, "description": "Optional upper bound on encoded output width. Values above 4096 are clamped to 4096. Never upscales."},
            "max_height": {"type": "integer", "minimum": 1, "description": "Optional upper bound on encoded output height. Values above 4096 are clamped to 4096. Never upscales."},
            "session_id": {"type": "string", "minLength": 1, "description": OPTIONAL_EXPLICIT_SESSION_ID_DESCRIPTION}
        },
        "required": ["project", "path", "client_id", "surface_id"]
    })
}

fn strip_gateway_field_descriptions(schema: &mut Value) {
    match schema {
        Value::Object(object) => {
            object.remove("description");
            for value in object.values_mut() {
                strip_gateway_field_descriptions(value);
            }
        }
        Value::Array(values) => {
            for value in values {
                strip_gateway_field_descriptions(value);
            }
        }
        _ => {}
    }
}

fn computer_action_schema(mut schema: Value, action: &'static str) -> Value {
    // The gateway ToolDefinition carries the shared semantics. Keep each oneOf
    // branch structural and bounded instead of repeating legacy per-tool prose.
    strip_gateway_field_descriptions(&mut schema);
    let object = schema
        .as_object_mut()
        .expect("Computer input schema object");
    let properties = object
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .expect("Computer input schema properties");
    properties.insert(
        "action".to_string(),
        json!({"type": "string", "const": action}),
    );
    let required = object
        .entry("required".to_string())
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .expect("Computer input schema required");
    required.insert(0, Value::String("action".to_string()));
    schema
}

fn computer_targets_action_schema() -> Value {
    computer_action_schema(
        json!({"type": "object", "additionalProperties": false, "properties": {}}),
        "targets",
    )
}

fn computer_element_action_schema(action: &'static str) -> Value {
    let mut schema = computer_element_control_input_schema();
    let object = schema
        .as_object_mut()
        .expect("Computer element control schema");
    object
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .expect("Computer element control properties")
        .remove("action");
    object
        .get_mut("required")
        .and_then(Value::as_array_mut)
        .expect("Computer element control required")
        .retain(|field| field.as_str() != Some("action"));
    computer_action_schema(schema, action)
}

fn computer_gateway_schema(branches: Vec<Value>) -> Value {
    let mut properties = serde_json::Map::new();
    for branch in &branches {
        for field in branch["properties"]
            .as_object()
            .expect("Computer action properties")
            .keys()
        {
            properties.entry(field.clone()).or_insert_with(|| json!({}));
        }
    }
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": properties,
        "required": ["action"],
        "oneOf": branches
    })
}

pub fn computer_observe_input_schema() -> Value {
    computer_gateway_schema(vec![
        computer_targets_action_schema(),
        computer_action_schema(computer_list_windows_input_schema(), "windows"),
        computer_action_schema(computer_list_displays_input_schema(), "displays"),
        computer_action_schema(computer_list_applications_input_schema(), "applications"),
        computer_action_schema(
            computer_accessibility_status_input_schema(),
            "accessibility_status",
        ),
        computer_action_schema(
            computer_accessibility_tree_input_schema(),
            "accessibility_tree",
        ),
        computer_action_schema(computer_find_elements_input_schema(), "find_elements"),
        computer_action_schema(computer_element_state_input_schema(), "element_state"),
        computer_action_schema(computer_snapshot_input_schema(), "snapshot_window"),
        computer_action_schema(computer_snapshot_display_input_schema(), "snapshot_display"),
        computer_action_schema(computer_read_clipboard_input_schema(), "read_clipboard"),
    ])
}

pub fn computer_control_input_schema() -> Value {
    computer_gateway_schema(vec![
        computer_action_schema(
            computer_launch_application_input_schema(),
            "launch_application",
        ),
        computer_action_schema(computer_activate_window_input_schema(), "activate_window"),
        computer_element_action_schema("press"),
        computer_element_action_schema("focus"),
        computer_action_schema(
            computer_scroll_to_element_input_schema(),
            "scroll_to_element",
        ),
        computer_action_schema(computer_key_input_input_schema(), "key"),
        computer_action_schema(computer_input_text_input_schema(), "input_text"),
        computer_action_schema(computer_pointer_input_schema(), "pointer_move"),
        computer_action_schema(computer_pointer_input_schema(), "pointer_click"),
        computer_action_schema(computer_write_clipboard_input_schema(), "write_clipboard"),
    ])
}
