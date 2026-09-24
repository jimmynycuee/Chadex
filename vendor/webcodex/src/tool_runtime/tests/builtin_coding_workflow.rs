use crate::tool_runtime::registry;
use crate::tool_runtime::startup_brief::{
    builtin_coding_workflow_projection, validate_schema_instance_for_test,
    BUILTIN_CODING_WORKFLOW_MAX_GUIDANCE_ITEMS,
};
use serde_json::{json, Value};

fn workflow_schema() -> Value {
    registry::output_schema_for_tool("work_on_project")["properties"]["output"]["properties"]
        ["workflow"]
        .clone()
}

#[test]
fn builtin_coding_workflow_defaults_are_required_and_bounded() {
    let workflow = builtin_coding_workflow_projection();
    let schema = workflow_schema();
    validate_schema_instance_for_test(&workflow, &schema).unwrap();

    let mut missing = workflow.clone();
    missing.as_object_mut().unwrap().remove("guidance");
    assert!(validate_schema_instance_for_test(&missing, &schema).is_err());

    for guidance in [
        json!([]),
        json!(vec!["rule"; BUILTIN_CODING_WORKFLOW_MAX_GUIDANCE_ITEMS + 1]),
        json!(["x".repeat(321)]),
    ] {
        let mut invalid = workflow.clone();
        invalid["guidance"] = guidance;
        assert!(validate_schema_instance_for_test(&invalid, &schema).is_err());
    }

    let mut legacy_role = workflow.clone();
    let review_role = legacy_role["roles"]["independent_review"].clone();
    legacy_role["roles"]["implementation_owner"] = review_role;
    assert!(validate_schema_instance_for_test(&legacy_role, &schema).is_err());
}

#[test]
fn builtin_coding_workflow_defaults_cover_unnamed_tasks_without_granting_authority() {
    let workflow = builtin_coding_workflow_projection();
    assert_eq!(workflow["version"], 13);
    assert_eq!(workflow["authority"], "model_guidance_only");
    let role_selection = workflow["role_selection"].as_str().unwrap();
    assert!(role_selection.contains("Ordinary implementation uses default guidance"));
    assert!(role_selection
        .contains("Use independent_review only for an explicit independent review pass"));
    assert!(role_selection.contains("Roles never grant authority"));
    let defaults = workflow["guidance"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_str().unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    for boundary in [
        "concrete, reviewable completion",
        "guidance grants no authority",
        "Recovery/compaction/exact Session resume is continuation",
        "reuse still-current Git/read/validation/Job facts",
        "explicit action/target",
        "user answer/Job/validation/result",
        "continue independent work",
        "wait only on real dependencies",
        "Ordinary implementation is default",
        "map cross-layer changes end to end",
        "compiler/schema/exhaustiveness failures",
        "avoid speculative redesign",
        "simplest sufficient primitive",
        "correctness/authority/evidence/durability/recovery/portability",
        "Native commands are first-class",
        "bounded deterministic Python/run_shell",
        "Batch predetermined observations",
        "adaptive follow-ups stay sequential",
        "bounded targeted reads",
        "files/count/small-context search",
        "native rg is first-class",
        "Validation failure is evidence, not queue cleanliness",
        "Reuse assertion_name",
        "outcome_unknown fails closed",
        "one execution/Job",
        "exact continuation",
        "wait_secs=100,wake_on=terminal",
        "not for visibility",
        "sufficient fresh validation",
        "Formatting is finalization",
        "After Rust stabilizes, format once",
        "before final diff/closeout",
        "rerun only after later Rust edits",
    ] {
        assert!(defaults.contains(boundary), "missing guidance: {boundary}");
    }
}

#[test]
fn builtin_coding_workflow_routes_persistent_shell_to_ssh_state_not_local_command_count() {
    let workflow = builtin_coding_workflow_projection();
    let guidance = workflow["model_protocol"]["persistent_shell"]
        .as_str()
        .expect("persistent shell guidance");

    for boundary in [
        "run_process=literal argv",
        "run_shell=shell grammar/short chains",
        "run_script=program-like scripts",
        "specialize for added semantics",
        "repeated named-SSH state",
        "local same-process state",
    ] {
        assert!(
            guidance.contains(boundary),
            "missing routing boundary: {boundary}"
        );
    }
    assert!(!guidance.contains("For repeated commands in one Workflow Session"));
    assert!(!guidance.contains("structured tools -> run_process/run_script -> run_shell"));
}

#[test]
fn builtin_coding_workflow_review_does_not_implicitly_authorize_edits() {
    let workflow = builtin_coding_workflow_projection();
    assert!(workflow["roles"]
        .as_object()
        .is_some_and(|roles| !roles.contains_key("implementation_owner")));
    let review = workflow["roles"]["independent_review"]["guidance"]
        .as_array()
        .unwrap();
    assert!(review.iter().any(|item| {
        let text = item.as_str().unwrap();
        text.contains("review-only")
            && text.contains("do not edit")
            && text.contains("only when the task authorizes corrections")
    }));
}
