use serde_json::{json, Value};

use super::common::OPTIONAL_EXPLICIT_SESSION_ID_DESCRIPTION;

fn line_scope_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "Optional 1-based inclusive positional fence against the guarded source snapshot. The complete exact match or anchor must be contained. occurrence remains global source-order and is never renumbered within the scope. Because line_scope can disambiguate equal global candidates, using it requires expected_read_revision on the containing file change.",
        "properties": {
            "start_line": {"type": "integer", "minimum": 1},
            "end_line": {"type": "integer", "minimum": 1}
        },
        "required": ["start_line", "end_line"]
    })
}

fn apply_text_edit_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": {"type": "string", "enum": ["replace_exact"]},
                    "old_text": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Exact text to replace; must be non-empty and is unique by default unless occurrence is supplied."
                    },
                    "new_text": {
                        "type": "string",
                        "description": "Replacement text. May be empty; omitting it preserves the existing wire behavior of replacing with an empty string."
                    },
                    "occurrence": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Optional 1-based global source-order exact occurrence selector. Positional selection requires expected_read_revision on the containing file change; line_scope never renumbers occurrence."
                    },
                    "line_scope": line_scope_schema()
                },
                "required": ["kind", "old_text"]
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": {"type": "string", "enum": ["delete_exact"]},
                    "old_text": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Exact text to delete; must be non-empty and is unique by default unless occurrence is supplied."
                    },
                    "occurrence": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Optional 1-based global source-order exact occurrence selector. Positional selection requires expected_read_revision on the containing file change; line_scope never renumbers occurrence."
                    },
                    "line_scope": line_scope_schema()
                },
                "required": ["kind", "old_text"]
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": {"type": "string", "enum": ["insert_before"]},
                    "anchor_text": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Exact anchor before which new_text is inserted; unique by default unless occurrence is supplied."
                    },
                    "new_text": {
                        "type": "string",
                        "description": "Text inserted before anchor_text. Empty text is accepted as a provable no-op and ignored without invalidating other edits in the transaction."
                    },
                    "occurrence": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Optional 1-based global source-order exact occurrence selector. Positional selection requires expected_read_revision on the containing file change; line_scope never renumbers occurrence."
                    },
                    "line_scope": line_scope_schema()
                },
                "required": ["kind", "anchor_text", "new_text"]
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": {"type": "string", "enum": ["insert_after"]},
                    "anchor_text": {
                        "type": "string",
                        "minLength": 1,
                        "description": "Exact anchor after which new_text is inserted; unique by default unless occurrence is supplied."
                    },
                    "new_text": {
                        "type": "string",
                        "description": "Text inserted after anchor_text. Empty text is accepted as a provable no-op and ignored without invalidating other edits in the transaction."
                    },
                    "occurrence": {
                        "type": "integer",
                        "minimum": 1,
                        "description": "Optional 1-based global source-order exact occurrence selector. Positional selection requires expected_read_revision on the containing file change; line_scope never renumbers occurrence."
                    },
                    "line_scope": line_scope_schema()
                },
                "required": ["kind", "anchor_text", "new_text"]
            }
        ]
    })
}

fn read_revision_schema(description: &str) -> Value {
    json!({
        "type": "integer",
        "minimum": 1,
        "maximum": 9007199254740991_u64,
        "description": description
    })
}

fn project_path_schema(description: &str) -> Value {
    json!({
        "type": "string",
        "minLength": 1,
        "description": description
    })
}

fn exact_replace_shorthand_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "description": "One non-positional replace_exact; positional selectors use canonical edit form.",
        "properties": {
            "path": {"type": "string", "minLength": 1},
            "old_text": {"type": "string", "minLength": 1},
            "new_text": {"type": "string"},
            "expected_read_revision": {"type": "integer", "minimum": 1, "maximum": 9007199254740991_u64}
        },
        "required": ["path", "old_text", "new_text"]
    })
}

fn apply_file_change_schema() -> Value {
    json!({
        "oneOf": [
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": {"type": "string", "enum": ["edit"]},
                    "path": project_path_schema("Project-relative existing file to edit."),
                    "expected_read_revision": read_revision_schema("Optional full-file snapshot guard from read_files. Globally unique exact local edits may omit it; occurrence or line_scope requires it."),
                    "edits": {
                        "type": "array",
                        "minItems": 1,
                        "maxItems": 20,
                        "description": "One to 20 exact edits applied transactionally to this file.",
                        "items": apply_text_edit_schema()
                    }
                },
                "required": ["kind", "path", "edits"]
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": {"type": "string", "enum": ["create"]},
                    "path": project_path_schema("Project-relative new file path."),
                    "content": {
                        "type": "string",
                        "description": "Complete UTF-8 content for the new file; empty content is valid."
                    }
                },
                "required": ["kind", "path", "content"]
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": {"type": "string", "enum": ["delete"]},
                    "path": project_path_schema("Project-relative existing file to delete."),
                    "expected_read_revision": read_revision_schema("Required full-file snapshot guard from read_files for deletion.")
                },
                "required": ["kind", "path", "expected_read_revision"]
            },
            {
                "type": "object",
                "additionalProperties": false,
                "properties": {
                    "kind": {"type": "string", "enum": ["rename"]},
                    "path": project_path_schema("Project-relative existing source file."),
                    "to_path": project_path_schema("Project-relative destination path; must differ from path."),
                    "expected_read_revision": read_revision_schema("Required full-file snapshot guard from read_files for rename.")
                },
                "required": ["kind", "path", "to_path", "expected_read_revision"]
            },
            exact_replace_shorthand_schema()
        ]
    })
}

pub fn apply_text_edits_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "project": {
                "type": "string",
                "description": "Runner-registered project id."
            },
            "changes": {
                "type": "array",
                "minItems": 1,
                "maxItems": 16,
                "description": "Transactional list of 1..16 file changes. Use explicit kind forms, or path + old_text + new_text for one replace_exact; the whole batch is preflighted before mutation.",
                "items": apply_file_change_schema()
            },
            "dry_run": {
                "type": "boolean",
                "description": "If true, compute the plan without writing."
            },
            "session_id": {
                "type": "string",
                "description": OPTIONAL_EXPLICIT_SESSION_ID_DESCRIPTION
            }
        },
        "required": ["project", "changes"],
        "additionalProperties": false
    })
}
