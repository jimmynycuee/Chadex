use super::support::*;
use crate::tool_runtime::sessions::{CodingSessionRequest, SessionGuards, SessionTransport};
use crate::tool_runtime::{SessionMode, ToolResult, ToolRuntime};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .stdin(Stdio::null())
        .current_dir(root)
        .output()
        .expect("run git fixture command");
    assert!(
        output.status.success(),
        "git {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn start_changes_session(
    runtime: &ToolRuntime,
    auth: &crate::auth::AuthContext,
    project: &str,
    baseline: String,
) -> crate::tool_runtime::SessionSummary {
    let authority_fingerprint =
        crate::tool_runtime::workflow_session_authority_fingerprint(Some(auth)).unwrap();
    runtime
        .sessions
        .ensure_coding_session_with_git_baseline(
            CodingSessionRequest {
                project: project.to_string(),
                authority_fingerprint,
                resume_session_id: None,
                instruction: Some("Final Changes integration".to_string()),
                mode: SessionMode::Normal,
                guards: SessionGuards::default(),
                execution_context: None,
                project_instructions: None,
                transport: SessionTransport::Mcp,
                context_refreshed: true,
                write_scope_verified: true,
            },
            Some(baseline),
        )
        .unwrap()
        .summary
}

fn record_first_class_edit(runtime: &ToolRuntime, session_id: &str, project: &str, path: &str) {
    let start = runtime.sessions.record_tool_call_started(
        Some(session_id),
        SessionTransport::Mcp,
        "apply_text_edits",
        &json!({
            "project": project,
            "changes": [{"kind": "edit", "path": path}]
        }),
        crate::tool_runtime::sessions::session_tool_contract("apply_text_edits"),
    );
    runtime.sessions.record_tool_call_finished(
        start,
        true,
        &json!({"applied": true, "state_changed": true}),
        None,
        None,
    );
    assert!(
        runtime
            .sessions
            .summary(session_id, None)
            .unwrap()
            .repository_edit_observed
    );
}

async fn service_agent_task(
    runtime: &ToolRuntime,
    client_id: &str,
    task: &tokio::task::JoinHandle<ToolResult>,
) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !task.is_finished() {
        assert!(Instant::now() < deadline, "Changes runtime task timed out");
        if let Some(request) = probe_patch_agent_request(runtime, client_id).await {
            complete_agent_request_by_running_locally(runtime, client_id, request).await;
        } else {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

async fn presentation_needed(
    runtime: &ToolRuntime,
    client_id: &str,
    project: &str,
    summary: crate::tool_runtime::SessionSummary,
) -> bool {
    let task = tokio::spawn({
        let runtime = runtime.clone();
        let project = project.to_string();
        async move {
            runtime
                .final_changes_presentation_needed(&project, &summary)
                .await
        }
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    while !task.is_finished() {
        assert!(Instant::now() < deadline, "Changes probe timed out");
        if let Some(request) = probe_patch_agent_request(runtime, client_id).await {
            complete_agent_request_by_running_locally(runtime, client_id, request).await;
        } else {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
    task.await.unwrap().unwrap()
}

async fn present(
    runtime: &ToolRuntime,
    client_id: &str,
    project: &str,
    session_id: &str,
    auth: &crate::auth::AuthContext,
) -> ToolResult {
    let task = tokio::spawn({
        let runtime = runtime.clone();
        let project = project.to_string();
        let session_id = session_id.to_string();
        let auth = auth.clone();
        async move {
            runtime
                .present_changes(project, session_id, Some(&auth))
                .await
        }
    });
    service_agent_task(runtime, client_id, &task).await;
    task.await.unwrap()
}

async fn file_diff(
    runtime: &ToolRuntime,
    client_id: &str,
    project: &str,
    session_id: &str,
    snapshot_id: &str,
    path: &str,
    auth: &crate::auth::AuthContext,
) -> ToolResult {
    let task = tokio::spawn({
        let runtime = runtime.clone();
        let project = project.to_string();
        let session_id = session_id.to_string();
        let snapshot_id = snapshot_id.to_string();
        let path = path.to_string();
        let auth = auth.clone();
        async move {
            runtime
                .changes_file_diff(project, session_id, snapshot_id, path, Some(&auth))
                .await
        }
    });
    service_agent_task(runtime, client_id, &task).await;
    task.await.unwrap()
}

fn file_by_path<'a>(result: &'a Value, path: &str) -> &'a Value {
    result["changes"]["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == path)
        .unwrap_or_else(|| panic!("missing {path}: {result}"))
}

#[tokio::test]
async fn final_changes_uses_startup_tree_whole_final_workspace_and_frozen_lazy_diff() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    let stable_lines = (0..40)
        .map(|index| format!("stable-{index}\n"))
        .collect::<String>();
    commit_file(tmp.path(), "README.md", "base readme\n", "readme");
    commit_file(tmp.path(), "rename_me.rs", &stable_lines, "rename source");
    commit_file(tmp.path(), "delete_me.rs", "delete me\n", "delete source");
    let baseline = git(tmp.path(), &["rev-parse", "HEAD^{tree}"]);

    // Startup dirt belongs to card content once this Session later performs a
    // real first-class Edit, even though the dirt predates the Session.
    fs::write(tmp.path().join("README.md"), "startup dirty\n").unwrap();

    let runtime = test_runtime();
    let auth = auth_context(None, true);
    let client_id = "changes-final";
    let project =
        register_runner_project_at_path_with_auth(&runtime, client_id, "demo", tmp.path(), &auth)
            .await;
    let session = start_changes_session(&runtime, &auth, &project, baseline);
    record_first_class_edit(&runtime, &session.session_id, &project, "rename_me.rs");

    fs::rename(
        tmp.path().join("rename_me.rs"),
        tmp.path().join("renamed.rs"),
    )
    .unwrap();
    let mut renamed = stable_lines.clone();
    renamed.push_str("one extra line\n");
    fs::write(tmp.path().join("renamed.rs"), renamed).unwrap();
    fs::remove_file(tmp.path().join("delete_me.rs")).unwrap();
    // This represents later shell-generated content: eligibility is still from
    // the first-class Edit, while content is the complete final Git workspace.
    fs::write(tmp.path().join("generated.rs"), "generated-v1\n").unwrap();
    fs::write(tmp.path().join("blob.bin"), [0_u8, 159, 146, 150, 255]).unwrap();
    fs::write(
        tmp.path().join("large.txt"),
        "line for bounded diff\n".repeat(5000),
    )
    .unwrap();

    let current = runtime.sessions.summary(&session.session_id, None).unwrap();
    assert!(presentation_needed(&runtime, client_id, &project, current).await);

    let result = present(&runtime, client_id, &project, &session.session_id, &auth).await;
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        file_by_path(&result.output, "README.md")["kind"],
        "modified"
    );
    assert_eq!(
        file_by_path(&result.output, "generated.rs")["kind"],
        "added"
    );
    assert_eq!(
        file_by_path(&result.output, "delete_me.rs")["kind"],
        "deleted"
    );
    let renamed = file_by_path(&result.output, "renamed.rs");
    assert_eq!(renamed["kind"], "renamed");
    assert_eq!(renamed["previous_path"], "rename_me.rs");
    assert_eq!(file_by_path(&result.output, "blob.bin")["binary"], true);
    assert!(result.output["changes"]["files_changed"].as_u64().unwrap() >= 6);
    assert_eq!(result.output["changes"]["files_truncated"], false);

    let snapshot_id = result.output["changes"]["snapshot_id"]
        .as_str()
        .unwrap()
        .to_string();
    fs::write(tmp.path().join("generated.rs"), "after-snapshot\n").unwrap();

    let frozen = file_diff(
        &runtime,
        client_id,
        &project,
        &session.session_id,
        &snapshot_id,
        "generated.rs",
        &auth,
    )
    .await;
    assert!(frozen.success, "{:?}", frozen.error);
    let patch = frozen.output["changes_file_diff"]["diff"].as_str().unwrap();
    assert!(patch.contains("generated-v1"));
    assert!(!patch.contains("after-snapshot"));

    let oversized = file_diff(
        &runtime,
        client_id,
        &project,
        &session.session_id,
        &snapshot_id,
        "large.txt",
        &auth,
    )
    .await;
    assert!(oversized.success, "{:?}", oversized.error);
    assert_eq!(oversized.output["changes_file_diff"]["truncated"], true);
    assert!(
        oversized.output["changes_file_diff"]["bytes_total"]
            .as_u64()
            .unwrap()
            > oversized.output["changes_file_diff"]["bytes_returned"]
                .as_u64()
                .unwrap()
    );

    let wrong_path = runtime
        .changes_file_diff(
            project.clone(),
            session.session_id.clone(),
            snapshot_id.clone(),
            "not-advertised.rs".to_string(),
            Some(&auth),
        )
        .await;
    assert!(!wrong_path.success);
    assert_eq!(
        wrong_path.output["error_kind"],
        "changes_snapshot_path_not_allowed"
    );

    let other_baseline = runtime
        .sessions
        .summary(&session.session_id, None)
        .unwrap()
        .git_baseline_tree
        .expect("other session baseline");
    let other_session = start_changes_session(&runtime, &auth, &project, other_baseline);
    let wrong_session = runtime
        .changes_file_diff(
            project.clone(),
            other_session.session_id,
            snapshot_id.clone(),
            "generated.rs".to_string(),
            Some(&auth),
        )
        .await;
    assert!(!wrong_session.success);
    assert_eq!(
        wrong_session.output["error_kind"],
        "changes_snapshot_identity_mismatch"
    );

    let wrong_snapshot = runtime
        .changes_file_diff(
            project,
            session.session_id,
            format!("wc_changes_snapshot_{}", "f".repeat(32)),
            "generated.rs".to_string(),
            Some(&auth),
        )
        .await;
    assert!(!wrong_snapshot.success);
    assert_eq!(
        wrong_snapshot.output["error_kind"],
        "changes_snapshot_unavailable"
    );
}

#[tokio::test]
async fn final_changes_neutralizes_repository_configured_clean_and_process_filters() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    fs::write(
        tmp.path().join(".gitattributes"),
        "clean.dat filter=cleaner\nprocess.dat filter=processor\n",
    )
    .unwrap();
    fs::write(tmp.path().join("clean.dat"), "base clean\n").unwrap();
    fs::write(tmp.path().join("process.dat"), "base process\n").unwrap();
    git(
        tmp.path(),
        &["add", ".gitattributes", "clean.dat", "process.dat"],
    );
    git(tmp.path(), &["commit", "-m", "filtered baseline"]);
    let baseline = git(tmp.path(), &["rev-parse", "HEAD^{tree}"]);

    let clean_script = tmp.path().join("clean-filter.sh");
    let process_script = tmp.path().join("process-filter.sh");
    let marker = tmp.path().join("filter-executed");
    fs::write(&clean_script, "printf 'clean\\n' >> \"$1\"\ncat\n").unwrap();
    fs::write(&process_script, "printf 'process\\n' >> \"$1\"\nexit 99\n").unwrap();
    let clean_command = format!("sh {} {}", clean_script.display(), marker.display());
    let process_command = format!("sh {} {}", process_script.display(), marker.display());
    git(
        tmp.path(),
        &["config", "filter.cleaner.clean", &clean_command],
    );
    git(tmp.path(), &["config", "filter.cleaner.required", "true"]);
    git(tmp.path(), &["config", "filter.processor.clean", "cat"]);
    git(
        tmp.path(),
        &["config", "filter.processor.process", &process_command],
    );
    git(tmp.path(), &["config", "filter.processor.required", "true"]);
    fs::write(tmp.path().join("clean.dat"), "changed clean\n").unwrap();
    fs::write(tmp.path().join("process.dat"), "changed process\n").unwrap();

    let runtime = test_runtime();
    let auth = auth_context(None, true);
    let client_id = "changes-filter-safe";
    let project =
        register_runner_project_at_path_with_auth(&runtime, client_id, "demo", tmp.path(), &auth)
            .await;
    let session = start_changes_session(&runtime, &auth, &project, baseline);
    record_first_class_edit(&runtime, &session.session_id, &project, "clean.dat");

    let summary = runtime.sessions.summary(&session.session_id, None).unwrap();
    assert!(presentation_needed(&runtime, client_id, &project, summary).await);
    assert!(
        !marker.exists(),
        "closeout Changes probe must not execute repository-configured filters"
    );

    let result = present(&runtime, client_id, &project, &session.session_id, &auth).await;
    assert!(result.success, "{:?}", result.error);
    assert!(
        !marker.exists(),
        "Changes snapshot must not execute repository-configured filters"
    );
    assert_eq!(
        file_by_path(&result.output, "clean.dat")["kind"],
        "modified"
    );
    assert_eq!(
        file_by_path(&result.output, "process.dat")["kind"],
        "modified"
    );
}

#[tokio::test]
async fn committed_final_tree_is_presentable_even_when_worktree_is_clean() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    commit_file(tmp.path(), "README.md", "base\n", "base");
    let baseline = git(tmp.path(), &["rev-parse", "HEAD^{tree}"]);
    let runtime = test_runtime();
    let auth = auth_context(None, true);
    let client_id = "changes-commit";
    let project =
        register_runner_project_at_path_with_auth(&runtime, client_id, "demo", tmp.path(), &auth)
            .await;
    let session = start_changes_session(&runtime, &auth, &project, baseline);
    record_first_class_edit(&runtime, &session.session_id, &project, "README.md");
    fs::write(tmp.path().join("README.md"), "committed task state\n").unwrap();
    git(tmp.path(), &["add", "README.md"]);
    git(tmp.path(), &["commit", "-m", "task commit"]);
    assert!(git(tmp.path(), &["status", "--porcelain"]).is_empty());

    let summary = runtime.sessions.summary(&session.session_id, None).unwrap();
    assert!(presentation_needed(&runtime, client_id, &project, summary).await);
    let result = present(&runtime, client_id, &project, &session.session_id, &auth).await;
    assert!(result.success, "{:?}", result.error);
    assert_eq!(result.output["changes"]["files_changed"], 1);
    assert_eq!(
        file_by_path(&result.output, "README.md")["kind"],
        "modified"
    );
}

#[tokio::test]
async fn shell_only_is_ineligible_and_reverted_first_class_edit_has_no_presentation() {
    let tmp = tempfile::tempdir().unwrap();
    init_git_repo(tmp.path());
    commit_file(tmp.path(), "README.md", "base\n", "base");
    let baseline = git(tmp.path(), &["rev-parse", "HEAD^{tree}"]);
    let runtime = test_runtime();
    let auth = auth_context(None, true);
    let client_id = "changes-revert";
    let project =
        register_runner_project_at_path_with_auth(&runtime, client_id, "demo", tmp.path(), &auth)
            .await;

    let shell_only = start_changes_session(&runtime, &auth, &project, baseline.clone());
    fs::write(tmp.path().join("shell-only.rs"), "shell write\n").unwrap();
    let summary = runtime
        .sessions
        .summary(&shell_only.session_id, None)
        .unwrap();
    assert!(!summary.repository_edit_observed);
    assert!(!runtime
        .final_changes_presentation_needed(&project, &summary)
        .await
        .unwrap());
    let denied = present(&runtime, client_id, &project, &shell_only.session_id, &auth).await;
    assert!(!denied.success);
    assert_eq!(
        denied.output["reason"],
        "session_has_no_first_class_repository_edit"
    );

    fs::remove_file(tmp.path().join("shell-only.rs")).unwrap();
    let reverted = start_changes_session(&runtime, &auth, &project, baseline);
    record_first_class_edit(&runtime, &reverted.session_id, &project, "README.md");
    fs::write(tmp.path().join("README.md"), "temporary\n").unwrap();
    git(tmp.path(), &["restore", "--source=HEAD", "--", "README.md"]);
    let summary = runtime
        .sessions
        .summary(&reverted.session_id, None)
        .unwrap();
    assert!(summary.repository_edit_observed);
    assert!(!presentation_needed(&runtime, client_id, &project, summary).await);
}
