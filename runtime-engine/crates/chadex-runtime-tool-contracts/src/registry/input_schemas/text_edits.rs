use serde_json::{json, Value};

use super::common::OPTIONAL_EXPLICIT_SESSION_ID_DESCRIPTION;

pub fn write_project_file_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "project": {"type": "string", "description": "Runner-registered project id."},
            "path": {"type": "string", "description": "Project-relative file path."},
            "content": {"type": "string", "description": "UTF-8 file content (no NUL)."},
            "overwrite": {
                "type": "boolean",
                "description": "Allow intentional replacement of an existing file (default false); true requires expected_read_revision."
            },
            "expected_read_revision": {
                "type": "integer",
                "minimum": 1,
                "maximum": 9007199254740991_u64,
                "description": "Current read_revision returned by read_files for this exact Project/path snapshot. Required with overwrite=true; omit for new-file creation. ToolRuntime resolves it to the Runner wire SHA guard."
            },
            "session_id": {
                "type": "string",
                "description": OPTIONAL_EXPLICIT_SESSION_ID_DESCRIPTION
            }
        },
        "required": ["project", "path", "content"],
        "additionalProperties": false,
        "allOf": [
            {
                "if": {"properties": {"overwrite": {"const": true}}, "required": ["overwrite"]},
                "then": {"required": ["expected_read_revision"]}
            },
            {
                "if": {"required": ["expected_read_revision"]},
                "then": {"properties": {"overwrite": {"const": true}}, "required": ["overwrite"]}
            }
        ]
    })
}
