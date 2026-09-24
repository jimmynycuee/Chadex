use serde_json::{json, Value};

use webcodex_core::runtime_contract::{
    MAX_SKILL_LIST_LIMIT, MAX_SKILL_QUERY_CHARS, MAX_SKILL_READ_LINES,
    MAX_SKILL_RESOURCE_PATH_CHARS,
};
use webcodex_core::skill_metadata::MAX_SKILL_NAME_CHARS;
use webcodex_core::skill_store::{
    MAX_OPERATOR_SKILL_KEY_CHARS, MAX_SKILL_STORE_IDEMPOTENCY_KEY_CHARS,
    MAX_SKILL_STORE_VERSIONS_LIMIT,
};

pub fn skill_list_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "project": {"type": "string", "minLength": 1, "description": "Required authorized runtime Project id."},
            "query": {"type": "string", "maxLength": MAX_SKILL_QUERY_CHARS, "description": "Optional bounded case-insensitive substring filter over Skill name and description only."},
            "offset": {"type": "integer", "minimum": 0},
            "limit": {"type": "integer", "minimum": 1, "maximum": MAX_SKILL_LIST_LIMIT},
            "expected_catalog_revision": {"type": "string", "pattern": "^wc_skillcat_[A-Za-z0-9_-]{43}$", "description": "Optional catalog revision guard. If current discovery differs, the call fails with skill_catalog_changed rather than continuing an old offset."},
            "session_id": {"type": "string", "description": "Optional explicit Workflow Session for this tool call. No implicit current-Session fallback is used."}
        },
        "required": ["project"],
        "additionalProperties": false
    })
}

pub fn run_skill_resource_input_schema() -> Value {
    let mut schema = super::jobs::run_process_input_schema();
    let properties = schema["properties"]
        .as_object_mut()
        .expect("run_process properties");
    for key in [
        "stdin",
        "assertion_name",
        "result_expectation",
        "accepted_exit_codes",
    ] {
        properties.remove(key);
    }
    properties.insert(
        "skill_id".to_string(),
        json!({
            "type": "string",
            "pattern": "^wc_skill_[A-Za-z0-9_-]{21}[AQgw]$",
            "description": "Opaque Runner Skill identity returned by skill_load or skill_list."
        }),
    );
    properties.insert("path".to_string(), json!({
        "type": "string",
        "minLength": 9,
        "maxLength": MAX_SKILL_RESOURCE_PATH_CHARS,
        "pattern": "^scripts/",
        "description": "Skill-package-relative script path under scripts/. Absolute paths and traversal are rejected."
    }));
    properties.insert("expected_definition_revision".to_string(), json!({
        "type": "string",
        "pattern": "^[0-9a-f]{64}$",
        "description": "Required SKILL.md definition digest fence. For configured live Skills this fences the definition only; script resource bytes are read live at execution. Managed installed Skills additionally use expected_package_revision to fence the immutable package."
    }));
    properties.insert("expected_package_revision".to_string(), json!({
        "type": "string",
        "pattern": "^wc_skillpkg_[A-Za-z0-9_-]{43}$",
        "description": "Required for operator-installed Skills and forbidden for configured live Skills. Pins the immutable installed package revision."
    }));
    properties.remove("executable");
    properties["args"]["description"] = json!("Ordered literal script arguments. WebCodex selects the interpreter and stdin-reading invocation from the trusted Skill resource extension, then appends these values after the interpreter's script marker. The Skill script body is never present in model arguments.");
    schema["required"] = json!([
        "project",
        "skill_id",
        "path",
        "expected_definition_revision"
    ]);
    schema
}

pub fn skill_load_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "project": {"type": "string", "minLength": 1, "description": "Required authorized runtime Project id."},
            "name": {"type": "string", "minLength": 1, "maxLength": MAX_SKILL_NAME_CHARS, "description": "Exact Skill name to load. Matching uses Unicode case folding; substring and fuzzy matching are not used."},
            "session_id": {"type": "string", "description": "Optional explicit Workflow Session for this tool call. No implicit current-Session fallback is used."}
        },
        "required": ["project", "name"],
        "additionalProperties": false
    })
}

pub fn skill_versions_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "project": {"type": "string", "minLength": 1},
            "skill_key": {"type": "string", "minLength": 1, "maxLength": MAX_OPERATOR_SKILL_KEY_CHARS, "pattern": "^[A-Za-z0-9._-]+$"},
            "offset": {"type": "integer", "minimum": 0},
            "limit": {"type": "integer", "minimum": 1, "maximum": MAX_SKILL_STORE_VERSIONS_LIMIT},
            "session_id": {"type": "string"}
        },
        "required": ["project", "skill_key"],
        "additionalProperties": false
    })
}

pub fn skill_install_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "project": {"type": "string", "minLength": 1, "description": "Authorized source Project; target operator store is the exact Runner owning this Project."},
            "skill_key": {"type": "string", "minLength": 1, "maxLength": MAX_OPERATOR_SKILL_KEY_CHARS, "pattern": "^[A-Za-z0-9._-]+$", "description": "Stable logical operator Skill key. It is not a filesystem path."},
            "artifact_path": {"type": "string", "minLength": 1, "maxLength": 1024, "description": "Project-relative existing ZIP artifact path. Native paths and URLs are not accepted."},
            "expected_artifact_sha256": {"type": "string", "pattern": "^[0-9a-f]{64}$"},
            "idempotency_key": {"type": "string", "minLength": 1, "maxLength": MAX_SKILL_STORE_IDEMPOTENCY_KEY_CHARS},
            "activate": {"type": "boolean", "default": false},
            "expected_state_revision": {"type": "string", "pattern": "^wc_skillstate_[A-Za-z0-9_-]{43}$", "description": "CAS guard required when activating into an existing logical Skill state."},
            "session_id": {"type": "string"}
        },
        "required": ["project", "skill_key", "artifact_path", "expected_artifact_sha256", "idempotency_key"],
        "additionalProperties": false
    })
}

fn skill_state_mutation_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "project": {"type": "string", "minLength": 1},
            "skill_key": {"type": "string", "minLength": 1, "maxLength": MAX_OPERATOR_SKILL_KEY_CHARS, "pattern": "^[A-Za-z0-9._-]+$"},
            "package_revision": {"type": "string", "pattern": "^wc_skillpkg_[A-Za-z0-9_-]{43}$"},
            "expected_state_revision": {"type": "string", "pattern": "^wc_skillstate_[A-Za-z0-9_-]{43}$"},
            "idempotency_key": {"type": "string", "minLength": 1, "maxLength": MAX_SKILL_STORE_IDEMPOTENCY_KEY_CHARS},
            "session_id": {"type": "string"}
        },
        "required": ["project", "skill_key", "package_revision", "expected_state_revision", "idempotency_key"],
        "additionalProperties": false
    })
}

pub fn skill_activate_input_schema() -> Value {
    skill_state_mutation_schema()
}

pub fn skill_remove_revision_input_schema() -> Value {
    skill_state_mutation_schema()
}

pub fn skill_read_file_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "project": {"type": "string", "minLength": 1, "description": "Required authorized runtime Project id."},
            "skill_id": {"type": "string", "pattern": "^wc_skill_[A-Za-z0-9_-]{21}[AQgw]$", "description": "Opaque Skill identity returned by skills.catalog or skill_list; it selects one exact source/package without exposing native Runner paths."},
            "path": {"type": "string", "minLength": 1, "maxLength": MAX_SKILL_RESOURCE_PATH_CHARS, "description": "Skill-package-relative UTF-8 text resource path. Defaults to SKILL.md; absolute paths and traversal are forbidden."},
            "start_line": {"type": "integer", "minimum": 1},
            "limit": {"type": "integer", "minimum": 1, "maximum": MAX_SKILL_READ_LINES},
            "expected_definition_revision": {"type": "string", "pattern": "^[0-9a-f]{64}$", "description": "Optional SKILL.md content digest guard. A mismatch fails with skill_definition_changed and does not return resource text."},
            "expected_package_revision": {"type": "string", "pattern": "^wc_skillpkg_[A-Za-z0-9_-]{43}$", "description": "Operator-installed Skills only. Pins the current active immutable package revision; stale values fail with skill_package_changed before resource text is returned."},
            "session_id": {"type": "string", "description": "Optional explicit Workflow Session for this tool call. No implicit current-Session fallback is used."}
        },
        "required": ["project", "skill_id"],
        "additionalProperties": false
    })
}
