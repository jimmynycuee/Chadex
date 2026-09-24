//! Phase 9 deterministic Chadex Task Executor integration tests.

use super::super::*;
use super::support::*;
use crate::runner_protocol::{
    RunnerCapabilities, RunnerJobUpdateRequest, RunnerPollRequest, ShellCommandExecutionState,
    ShellJobActivity, ShellJobActivityPhase, ShellJobActivitySource, ShellJobActivityState,
};
use serde_json::{json, Value};
use std::fs;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};
use webcodex_tool_runtime_contracts::{
    ApplyFileChangeInput, ApplyFileChangeKind, ApplyTextEditInput, ApplyTextEditKind,
    ChadexTaskProcess,
};

const CLIENT: &str = "chadex-task-executor";

fn acceptance_without_closeout() -> ChadexTaskAcceptance {
    ChadexTaskAcceptance {
        require_validation_success: Some(false),
        require_review: Some(false),
    }
}

fn unfenced_exact_edit(path: &str, old_text: &str, new_text: &str) -> ApplyFileChangeInput {
    ApplyFileChangeInput {
        kind: ApplyFileChangeKind::Edit,
        path: path.to_string(),
        to_path: None,
        content: None,
        edits: vec![ApplyTextEditInput {
            kind: ApplyTextEditKind::ReplaceExact,
            old_text: Some(old_text.to_string()),
            new_text: Some(new_text.to_string()),
            anchor_text: None,
            occurrence: None,
            line_scope: None,
        }],
        expected_read_revision: None,
    }
}

async fn complete_task_mutation_request(
    runtime: &ToolRuntime,
    request: crate::runner_protocol::RunnerRequest,
) -> bool {
    assert_eq!(request.kind, "file_apply_text_edits");
    let payload: Value = serde_json::from_str(
        request
            .content
            .as_deref()
            .expect("apply_text_edits request content"),
    )
    .unwrap();
    let change = &payload["changes"][0];
    let path = change["path"].as_str().expect("edit path");
    let expected_sha = change["expected_sha256"]
        .as_str()
        .expect("Task Executor guard read must translate into expected_sha256");
    let root = Path::new(request.cwd.as_deref().expect("mutation cwd"));
    let full = root.join(path);
    let current = fs::read_to_string(&full).unwrap();
    let actual_sha = crate::tool_runtime::files::sha256_hex_bytes(current.as_bytes());
    assert_eq!(
        expected_sha, actual_sha,
        "guard revision must fence current file"
    );

    let edit = &change["edits"][0];
    assert_eq!(edit["kind"], "replace_exact");
    let old_text = edit["old_text"].as_str().unwrap();
    let new_text = edit["new_text"].as_str().unwrap_or_default();
    assert_eq!(current.matches(old_text).count(), 1);
    let next = current.replacen(old_text, new_text, 1);
    fs::write(&full, &next).unwrap();

    let stdout = json!({
        "dry_run": false,
        "applied_count": 1,
        "changed": true,
        "would_change": true,
        "files": [{"index": 0, "kind": "edit", "path": path}],
        "changed_paths": [path],
    })
    .to_string();
    complete_patch_agent_request(runtime, CLIENT, &request.request_id, 0, &stdout, "").await;
    true
}

fn test_git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("spawn test Git command");
    assert!(
        output.status.success(),
        "test Git command failed: git {:?}\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn test_git_with_index(root: &Path, index: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .env("GIT_INDEX_FILE", index)
        .output()
        .expect("spawn test Git alternate-index command");
    assert!(
        output.status.success(),
        "test Git alternate-index command failed: git {:?}\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

fn test_snapshot_tree(root: &Path, include_dirty: bool) -> String {
    let head = test_git(root, &["rev-parse", "HEAD"]);
    let source_dirty = !test_git(root, &["status", "--porcelain"]).is_empty();
    if !include_dirty || !source_dirty {
        return head;
    }
    let index = tempfile::NamedTempFile::new().unwrap();
    let index_path = index.path().to_path_buf();
    drop(index);
    let _ = fs::remove_file(&index_path);
    test_git_with_index(root, &index_path, &["read-tree", "HEAD"]);
    test_git_with_index(root, &index_path, &["add", "-A", "--", "."]);
    let tree = test_git_with_index(root, &index_path, &["write-tree"]);
    let commit = Command::new("git")
        .args([
            "commit-tree",
            &tree,
            "-p",
            &head,
            "-m",
            "Chadex test dirty snapshot",
        ])
        .current_dir(root)
        .env("GIT_INDEX_FILE", &index_path)
        .env("GIT_AUTHOR_NAME", "Chadex Test")
        .env("GIT_AUTHOR_EMAIL", "chadex-test@localhost")
        .env("GIT_COMMITTER_NAME", "Chadex Test")
        .env("GIT_COMMITTER_EMAIL", "chadex-test@localhost")
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .output()
        .expect("spawn test Git commit-tree command");
    assert!(commit.status.success(), "test dirty snapshot commit failed");
    let _ = fs::remove_file(&index_path);
    String::from_utf8_lossy(&commit.stdout).trim().to_string()
}

fn test_project_id(request_id: &str) -> String {
    let suffix = request_id
        .chars()
        .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
        .take(36)
        .collect::<String>();
    format!("phase11_{suffix}")
}

async fn complete_test_project_operation(
    runtime: &ToolRuntime,
    request: &crate::runner_protocol::RunnerRequest,
    worktree_parents: &mut Vec<tempfile::TempDir>,
) {
    let payload: Value =
        serde_json::from_str(request.stdin.as_deref().expect("project operation payload")).unwrap();
    match request.kind.as_str() {
        "prepare_managed_worktree" => {
            let source = Path::new(payload["path"].as_str().expect("source path"));
            let include_dirty = payload["include_dirty_snapshot"].as_bool().unwrap_or(false);
            let source_dirty = !test_git(source, &["status", "--porcelain"]).is_empty();
            let base_sha = test_snapshot_tree(source, include_dirty);
            let parent = tempfile::tempdir().unwrap();
            let worktree = parent.path().join("worktree");
            let worktree_string = worktree.to_string_lossy().to_string();
            test_git(
                source,
                &["worktree", "add", "--detach", &worktree_string, &base_sha],
            );
            let agent_project_id = test_project_id(&request.request_id);
            let revision = format!("sha256:{}", "a".repeat(64));
            let response = json!({
                "id": format!("agent:{CLIENT}:{agent_project_id}"),
                "agent_project_id": agent_project_id,
                "client_id": CLIENT,
                "name": "Phase 11 test worktree",
                "path": worktree_string,
                "kind": "auto_registered",
                "registration_source": "auto_registered",
                "allow_patch": true,
                "disabled": false,
                "revision": revision,
                "root_fingerprint": format!("wc_projroot_{}", "b".repeat(64)),
                "lineage": {
                    "kind": "managed_worktree_source",
                    "source_project_id": "fixture",
                    "source_root_fingerprint": format!("wc_projroot_{}", "c".repeat(64)),
                    "base_sha": base_sha,
                },
                "source": "managed_worktree",
                "outcome": "managed_worktree_created",
                "registered": true,
                "created_config": true,
                "changed": true,
                "recovered": false,
                "managed": true,
                "base_sha": base_sha,
                "source_dirty": source_dirty,
            });
            worktree_parents.push(parent);
            complete_patch_agent_request(
                runtime,
                CLIENT,
                &request.request_id,
                0,
                &response.to_string(),
                "",
            )
            .await;
        }
        "project_lifecycle_unregister" => {
            let response = json!({
                "outcome": "unregistered",
                "changed": true,
                "revision": payload["expected_revision"],
            });
            complete_patch_agent_request(
                runtime,
                CLIENT,
                &request.request_id,
                0,
                &response.to_string(),
                "",
            )
            .await;
        }
        other => panic!("unexpected Chadex test project operation: {other}"),
    }
}

async fn register_isolated_test_project(
    runtime: &ToolRuntime,
    project_id: &str,
    root: &Path,
) -> String {
    register_runner_project_at_path_with_capabilities(
        runtime,
        CLIENT,
        project_id,
        root,
        RunnerCapabilities {
            shell: true,
            git: true,
            file_read: true,
            file_write: true,
            internal_posix_script: true,
            managed_worktree: true,
            ..Default::default()
        },
    )
    .await
}

async fn register_isolated_async_test_project(
    runtime: &ToolRuntime,
    project_id: &str,
    root: &Path,
) -> String {
    register_runner_project_at_path_with_capabilities(
        runtime,
        CLIENT,
        project_id,
        root,
        RunnerCapabilities {
            shell: true,
            git: true,
            file_read: true,
            file_write: true,
            internal_posix_script: true,
            managed_worktree: true,
            async_jobs: true,
            async_shell_jobs: true,
            structured_process_argv: true,
            structured_execution_jobs: true,
            ..Default::default()
        },
    )
    .await
}

#[derive(Clone, Copy)]
enum SourceMutationMode {
    None,
    UnrelatedFile,
    IndexOnly,
    SameFileCompatible,
    SameFileConflict,
    GraphifyMetadata,
}

async fn drive_task(runtime: &ToolRuntime, call: ToolCall) -> (ToolResult, Vec<String>, bool) {
    drive_task_with_source_mutation_mode(runtime, call, None, SourceMutationMode::None).await
}

async fn update_task_process_job(
    runtime: &ToolRuntime,
    request: &crate::runner_protocol::RunnerRequest,
    status: &str,
    state: Option<ShellCommandExecutionState>,
    exit_code: Option<i32>,
) {
    let activity = (state.is_none() && status == "running").then_some(ShellJobActivity {
        state: ShellJobActivityState::Working,
        phase: ShellJobActivityPhase::ProcessRunning,
        source: ShellJobActivitySource::RunnerExecution,
    });
    runtime
        .runner_registry
        .update_job(RunnerJobUpdateRequest {
            client_id: CLIENT.to_string(),
            runner_instance_id: "inst".to_string(),
            job_id: request.job_id.clone().expect("structured Job id"),
            request_id: Some(request.request_id.clone()),
            update_seq: None,
            status: status.to_string(),
            stdout_chunk: state.map(|_| "terminal stdout\n".to_string()),
            stderr_chunk: None,
            stdout_tail: None,
            stderr_tail: None,
            log_snapshot: None,
            exit_code,
            duration_ms: state.map(|_| 150),
            error: None,
            command_execution_state: state,
            validation_progress: None,
            test_count_evidence: None,
            activity,
            finished: state.is_some(),
        })
        .await
        .unwrap();
}

async fn finish_task_process_job_stopped(
    runtime: &ToolRuntime,
    job_id: &str,
    start_request_id: &str,
) {
    runtime
        .runner_registry
        .update_job(RunnerJobUpdateRequest {
            client_id: CLIENT.to_string(),
            runner_instance_id: "inst".to_string(),
            job_id: job_id.to_string(),
            request_id: Some(start_request_id.to_string()),
            update_seq: None,
            status: "stopped".to_string(),
            stdout_chunk: None,
            stderr_chunk: Some("job stopped by request\n".to_string()),
            stdout_tail: None,
            stderr_tail: None,
            log_snapshot: None,
            exit_code: Some(-1),
            duration_ms: Some(200),
            error: Some("job stopped".to_string()),
            command_execution_state: Some(ShellCommandExecutionState::Completed),
            validation_progress: None,
            test_count_evidence: None,
            activity: None,
            finished: true,
        })
        .await
        .unwrap();
}

async fn drive_task_with_promoted_process_job(
    runtime: &ToolRuntime,
    call: ToolCall,
) -> (ToolResult, bool) {
    let task = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            let auth = auth_context(None, true);
            runtime.dispatch_with_auth(call, Some(&auth)).await
        }
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut worktree_parents = Vec::new();
    let mut saw_promoted_job = false;
    while !task.is_finished() {
        assert!(
            Instant::now() < deadline,
            "promoted Job task fixture timed out"
        );
        let request = runtime
            .runner_registry
            .poll(RunnerPollRequest {
                client_id: CLIENT.to_string(),
                runner_instance_id: "inst".to_string(),
            })
            .await
            .unwrap();
        let Some(request) = request else {
            tokio::time::sleep(Duration::from_millis(2)).await;
            continue;
        };
        if matches!(
            request.kind.as_str(),
            "prepare_managed_worktree" | "project_lifecycle_unregister"
        ) {
            complete_test_project_operation(runtime, &request, &mut worktree_parents).await;
        } else if request.kind == "start_process_job" {
            saw_promoted_job = true;
            update_task_process_job(runtime, &request, "running", None, None).await;
            tokio::time::sleep(Duration::from_millis(100)).await;
            update_task_process_job(
                runtime,
                &request,
                "completed",
                Some(ShellCommandExecutionState::Completed),
                Some(0),
            )
            .await;
        } else {
            let (exit_code, stdout, stderr) = run_runner_shell_request_locally(&request);
            complete_patch_agent_request(
                runtime,
                CLIENT,
                &request.request_id,
                exit_code,
                &stdout,
                &stderr,
            )
            .await;
        }
    }
    (task.await.unwrap(), saw_promoted_job)
}

async fn drive_cancelled_task_with_promoted_process_job(
    runtime: &ToolRuntime,
    call: ToolCall,
    project: &str,
    task_id: &str,
) -> (ToolResult, bool, bool) {
    let task = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            let auth = auth_context(None, true);
            runtime.dispatch_with_auth(call, Some(&auth)).await
        }
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut worktree_parents = Vec::new();
    let mut saw_promoted_job = false;
    let mut saw_stop_job = false;
    let mut active_job_id: Option<String> = None;
    let mut active_start_request_id: Option<String> = None;
    while !task.is_finished() {
        assert!(
            Instant::now() < deadline,
            "cancelled promoted Job fixture timed out"
        );
        let request = runtime
            .runner_registry
            .poll(RunnerPollRequest {
                client_id: CLIENT.to_string(),
                runner_instance_id: "inst".to_string(),
            })
            .await
            .unwrap();
        let Some(request) = request else {
            tokio::time::sleep(Duration::from_millis(2)).await;
            continue;
        };
        if matches!(
            request.kind.as_str(),
            "prepare_managed_worktree" | "project_lifecycle_unregister"
        ) {
            complete_test_project_operation(runtime, &request, &mut worktree_parents).await;
        } else if request.kind == "start_process_job" {
            saw_promoted_job = true;
            active_job_id = request.job_id.clone();
            active_start_request_id = Some(request.request_id.clone());
            update_task_process_job(runtime, &request, "running", None, None).await;
            let cancel = runtime.cancel_chadex_task(project, task_id);
            assert!(cancel.success, "{:?} {}", cancel.error, cancel.output);
            assert_eq!(cancel.output["status"], "cancelling");
        } else if request.kind == "stop_job" {
            saw_stop_job = true;
            assert_eq!(request.job_id, active_job_id);
            finish_task_process_job_stopped(
                runtime,
                active_job_id.as_deref().expect("active promoted Job id"),
                active_start_request_id
                    .as_deref()
                    .expect("active promoted Job request id"),
            )
            .await;
        } else {
            let (exit_code, stdout, stderr) = run_runner_shell_request_locally(&request);
            complete_patch_agent_request(
                runtime,
                CLIENT,
                &request.request_id,
                exit_code,
                &stdout,
                &stderr,
            )
            .await;
        }
    }
    (task.await.unwrap(), saw_promoted_job, saw_stop_job)
}

async fn drive_disconnected_task_with_promoted_process_job(
    runtime: &ToolRuntime,
    call: ToolCall,
    project: &str,
    task_id: &str,
) -> ToolResult {
    let mut request_task = Some(tokio::spawn({
        let runtime = runtime.clone();
        async move {
            let auth = auth_context(None, true);
            runtime.dispatch_with_auth(call, Some(&auth)).await
        }
    }));
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut worktree_parents = Vec::new();
    let mut request_dropped = false;

    loop {
        assert!(
            Instant::now() < deadline,
            "disconnected promoted Job fixture timed out"
        );
        if request_dropped {
            let observed = runtime.observe_chadex_task(project, task_id);
            if observed.success
                && observed
                    .output
                    .get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|status| {
                        matches!(
                            status,
                            "completed"
                                | "failed"
                                | "failed_validation"
                                | "blocked"
                                | "cancelled"
                                | "interrupted"
                                | "unknown"
                        )
                    })
            {
                return observed;
            }
        }

        let request = runtime
            .runner_registry
            .poll(RunnerPollRequest {
                client_id: CLIENT.to_string(),
                runner_instance_id: "inst".to_string(),
            })
            .await
            .unwrap();
        let Some(request) = request else {
            tokio::time::sleep(Duration::from_millis(2)).await;
            continue;
        };
        if matches!(
            request.kind.as_str(),
            "prepare_managed_worktree" | "project_lifecycle_unregister"
        ) {
            complete_test_project_operation(runtime, &request, &mut worktree_parents).await;
        } else if request.kind == "start_process_job" {
            update_task_process_job(runtime, &request, "running", None, None).await;
            let handle = request_task
                .take()
                .expect("request observer still attached");
            handle.abort();
            assert!(handle.await.unwrap_err().is_cancelled());
            request_dropped = true;
            tokio::time::sleep(Duration::from_millis(100)).await;
            update_task_process_job(
                runtime,
                &request,
                "completed",
                Some(ShellCommandExecutionState::Completed),
                Some(0),
            )
            .await;
        } else {
            let (exit_code, stdout, stderr) = run_runner_shell_request_locally(&request);
            complete_patch_agent_request(
                runtime,
                CLIENT,
                &request.request_id,
                exit_code,
                &stdout,
                &stderr,
            )
            .await;
        }
    }
}

async fn drive_task_with_source_mutation(
    runtime: &ToolRuntime,
    call: ToolCall,
    source_root: Option<&Path>,
) -> (ToolResult, Vec<String>, bool) {
    drive_task_with_source_mutation_mode(
        runtime,
        call,
        source_root,
        SourceMutationMode::UnrelatedFile,
    )
    .await
}

async fn drive_task_with_source_mutation_and_index_change(
    runtime: &ToolRuntime,
    call: ToolCall,
    source_root: Option<&Path>,
    mutate_index_only: bool,
) -> (ToolResult, Vec<String>, bool) {
    drive_task_with_source_mutation_mode(
        runtime,
        call,
        source_root,
        if mutate_index_only {
            SourceMutationMode::IndexOnly
        } else {
            SourceMutationMode::UnrelatedFile
        },
    )
    .await
}

async fn drive_task_with_source_mutation_mode(
    runtime: &ToolRuntime,
    call: ToolCall,
    source_root: Option<&Path>,
    mutation_mode: SourceMutationMode,
) -> (ToolResult, Vec<String>, bool) {
    let task = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            let auth = auth_context(None, true);
            runtime.dispatch_with_auth(call, Some(&auth)).await
        }
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut kinds = Vec::new();
    let mut saw_guarded_edit = false;
    let mut worktree_parents = Vec::new();
    let mut source_mutated = false;
    while !task.is_finished() {
        assert!(Instant::now() < deadline, "execute_task fixture timed out");
        let request = runtime
            .runner_registry
            .poll(RunnerPollRequest {
                client_id: CLIENT.to_string(),
                runner_instance_id: "inst".to_string(),
            })
            .await
            .unwrap();
        let Some(request) = request else {
            tokio::time::sleep(Duration::from_millis(2)).await;
            continue;
        };
        kinds.push(request.kind.clone());
        if matches!(
            request.kind.as_str(),
            "prepare_managed_worktree" | "project_lifecycle_unregister"
        ) {
            complete_test_project_operation(runtime, &request, &mut worktree_parents).await;
        } else if request.kind == "file_apply_text_edits" {
            saw_guarded_edit |= complete_task_mutation_request(runtime, request).await;
            if let Some(source_root) = source_root {
                if !source_mutated {
                    match mutation_mode {
                        SourceMutationMode::None => {}
                        SourceMutationMode::UnrelatedFile => {
                            fs::write(
                                source_root.join("user-change.txt"),
                                "changed while task ran\n",
                            )
                            .unwrap();
                        }
                        SourceMutationMode::IndexOnly => {
                            fs::write(source_root.join("index-only.txt"), "staged then removed\n")
                                .unwrap();
                            test_git(source_root, &["add", "index-only.txt"]);
                            fs::remove_file(source_root.join("index-only.txt")).unwrap();
                        }
                        SourceMutationMode::SameFileCompatible => {
                            let path = source_root.join("value.txt");
                            let mut content = fs::read_to_string(&path).unwrap();
                            content.push_str("user-tail\n");
                            fs::write(path, content).unwrap();
                        }
                        SourceMutationMode::SameFileConflict => {
                            fs::write(source_root.join("value.txt"), "user-version\n").unwrap();
                        }
                        SourceMutationMode::GraphifyMetadata => {
                            fs::create_dir_all(source_root.join("graphify-out/cache")).unwrap();
                            fs::write(
                                source_root.join("graphify-out/manifest.json"),
                                "{\"seen\":2}\n",
                            )
                            .unwrap();
                            fs::write(
                                source_root.join("graphify-out/cache/stat-index.json"),
                                "{\"seen\":2}\n",
                            )
                            .unwrap();
                        }
                    }
                    source_mutated = true;
                }
            }
        } else {
            let (exit_code, stdout, stderr) = run_runner_shell_request_locally(&request);
            complete_patch_agent_request(
                runtime,
                CLIENT,
                &request.request_id,
                exit_code,
                &stdout,
                &stderr,
            )
            .await;
        }
    }
    (task.await.unwrap(), kinds, saw_guarded_edit)
}

async fn drive_two_tasks(
    runtime: &ToolRuntime,
    call_a: ToolCall,
    call_b: ToolCall,
) -> (ToolResult, ToolResult, Vec<String>) {
    let task_a = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            let auth = auth_context(None, true);
            runtime.dispatch_with_auth(call_a, Some(&auth)).await
        }
    });
    let task_b = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            let auth = auth_context(None, true);
            runtime.dispatch_with_auth(call_b, Some(&auth)).await
        }
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut kinds = Vec::new();
    let mut worktree_parents = Vec::new();
    while !task_a.is_finished() || !task_b.is_finished() {
        assert!(
            Instant::now() < deadline,
            "concurrent execute_task fixture timed out"
        );
        let request = runtime
            .runner_registry
            .poll(RunnerPollRequest {
                client_id: CLIENT.to_string(),
                runner_instance_id: "inst".to_string(),
            })
            .await
            .unwrap();
        let Some(request) = request else {
            tokio::time::sleep(Duration::from_millis(2)).await;
            continue;
        };
        kinds.push(request.kind.clone());
        if matches!(
            request.kind.as_str(),
            "prepare_managed_worktree" | "project_lifecycle_unregister"
        ) {
            complete_test_project_operation(runtime, &request, &mut worktree_parents).await;
        } else if request.kind == "file_apply_text_edits" {
            complete_task_mutation_request(runtime, request).await;
        } else {
            let (exit_code, stdout, stderr) = run_runner_shell_request_locally(&request);
            complete_patch_agent_request(
                runtime,
                CLIENT,
                &request.request_id,
                exit_code,
                &stdout,
                &stderr,
            )
            .await;
        }
    }
    (task_a.await.unwrap(), task_b.await.unwrap(), kinds)
}

#[tokio::test]
async fn execute_task_runs_read_guarded_edit_validation_and_review() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("fixture");
    fs::create_dir_all(root.join("src")).unwrap();
    init_git_repo(&root);
    commit_file(
        &root,
        "src/lib.rs",
        "pub fn value() -> &'static str { \"before\" }\n",
        "seed",
    );

    let runtime = test_runtime();
    let project = register_isolated_test_project(&runtime, "fixture", &root).await;
    let task_id = "chadex_task_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string();
    let call = ToolCall::ExecuteTask {
        project: project.clone(),
        task_id: Some(task_id.clone()),
        goal: "Change the fixture marker, validate it, and review the final diff.".to_string(),
        preplan: None,
        package_count: None,
        steps: vec![
            ChadexTaskStep::Read {
                items: vec![ReadFilesItem {
                    path: "src/lib.rs".to_string(),
                    start_line: None,
                    limit: None,
                    expected_read_revision: None,
                }],
                with_line_numbers: Some(false),
                max_result_bytes: Some(16 * 1024),
            },
            ChadexTaskStep::Edit {
                changes: vec![unfenced_exact_edit("src/lib.rs", "before", "after")],
                dry_run: Some(false),
            },
            ChadexTaskStep::Validate {
                checks: vec![ChadexTaskProcess {
                    executable: "git".to_string(),
                    args: vec!["diff".to_string(), "--check".to_string()],
                    stdin: None,
                    cwd: None,
                    timeout_secs: Some(10),
                }],
            },
            ChadexTaskStep::Review {
                include_diff: Some(false),
                max_hunks: Some(8),
                max_hunk_lines: Some(80),
            },
        ],
        policy: Some(ChadexTaskPolicy {
            max_steps: Some(4),
            max_mutations: Some(1),
            max_changed_files: Some(1),
            timeout_secs: Some(30),
            max_result_bytes: Some(32 * 1024),
            ..Default::default()
        }),
        acceptance: None,
        session_id: None,
    };

    let (result, kinds, saw_guarded_edit) = drive_task(&runtime, call).await;
    assert!(result.success, "{:?} {}", result.error, result.output);
    assert_eq!(result.output["task_id"], task_id);
    assert_eq!(result.output["status"], "completed");
    assert_eq!(result.output["execution_state"], "succeeded");
    assert_eq!(result.output["completed_steps"], 4);
    assert_eq!(result.output["counters"]["completed_mutations"], 1);
    assert_eq!(result.output["validation"]["status"], "passed");
    assert_eq!(result.output["validation"]["checks_passed"], 1);
    assert_eq!(result.output["review"]["status"], "completed");
    assert_eq!(result.output["review"]["changed_file_count"], 1);
    assert_eq!(result.output["complexity"]["execution_task_count"], 1);
    assert_eq!(result.output["complexity"]["advisory_only"], false);
    assert!(result.output["complexity"]["recommended_task_count"]
        .as_u64()
        .is_some());
    assert_eq!(result.output["packaging"]["outer_execute_task_count"], 1);
    assert_eq!(result.output["packaging"]["effective_package_count"], 1);
    assert_eq!(
        result.output["packaging"]["graphify_status"],
        "skipped_single_package"
    );
    assert_eq!(
        result.output["packaging"]["packages"][0]["status"],
        "completed"
    );
    assert!(saw_guarded_edit);
    assert!(
        kinds
            .iter()
            .filter(|kind| kind.as_str() == "file_read")
            .count()
            >= 2
    );
    assert!(kinds.iter().any(|kind| kind == "run_process"));
    assert!(kinds.iter().any(|kind| kind == "run_internal_posix_script"));
    assert!(fs::read_to_string(root.join("src/lib.rs"))
        .unwrap()
        .contains("after"));

    let observed = runtime.observe_chadex_task(&project, &task_id);
    assert!(observed.success);
    assert_eq!(observed.output["status"], "completed");
    assert_eq!(observed.output["completed_steps"], 4);
}

#[tokio::test]
async fn execute_task_applies_with_unrelated_source_change() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("fixture");
    fs::create_dir_all(&root).unwrap();
    init_git_repo(&root);
    commit_file(&root, "value.txt", "before\n", "seed");

    let runtime = test_runtime();
    let project = register_isolated_test_project(&runtime, "reconcile", &root).await;
    let call = ToolCall::ExecuteTask {
        project,
        task_id: Some(format!("chadex_task_{}", "c".repeat(32))),
        goal: "Apply one isolated edit while preserving a user change made during execution."
            .to_string(),
        preplan: None,
        package_count: None,
        steps: vec![
            ChadexTaskStep::Edit {
                changes: vec![unfenced_exact_edit("value.txt", "before", "after")],
                dry_run: Some(false),
            },
            ChadexTaskStep::Validate {
                checks: vec![ChadexTaskProcess {
                    executable: "git".to_string(),
                    args: vec!["diff".to_string(), "--check".to_string()],
                    stdin: None,
                    cwd: None,
                    timeout_secs: Some(10),
                }],
            },
            ChadexTaskStep::Review {
                include_diff: Some(false),
                max_hunks: Some(8),
                max_hunk_lines: Some(80),
            },
        ],
        policy: Some(ChadexTaskPolicy {
            max_steps: Some(3),
            max_mutations: Some(1),
            max_changed_files: Some(1),
            timeout_secs: Some(30),
            max_result_bytes: Some(32 * 1024),
            ..Default::default()
        }),
        acceptance: None,
        session_id: None,
    };

    let (result, _, saw_guarded_edit) =
        drive_task_with_source_mutation(&runtime, call, Some(&root)).await;
    assert!(result.success, "{:?} {}", result.error, result.output);
    assert_eq!(result.output["status"], "completed");
    assert_eq!(result.output["execution_state"], "succeeded");
    assert_ne!(result.output["counters"]["reconciliation_applied"], true);
    assert!(saw_guarded_edit);
    assert_eq!(
        fs::read_to_string(root.join("value.txt")).unwrap(),
        "after\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("user-change.txt")).unwrap(),
        "changed while task ran\n"
    );
}

#[tokio::test]
async fn execute_task_applies_with_unrelated_source_index_change() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("fixture");
    fs::create_dir_all(&root).unwrap();
    init_git_repo(&root);
    commit_file(&root, "value.txt", "before\n", "seed");

    let runtime = test_runtime();
    let project = register_isolated_test_project(&runtime, "index-reconcile", &root).await;
    let call = ToolCall::ExecuteTask {
        project,
        task_id: Some(format!("chadex_task_{}", "d".repeat(32))),
        goal: "Apply an isolated edit while preserving an index-only user change.".to_string(),
        preplan: None,
        package_count: None,
        steps: vec![
            ChadexTaskStep::Edit {
                changes: vec![unfenced_exact_edit("value.txt", "before", "after")],
                dry_run: Some(false),
            },
            ChadexTaskStep::Validate {
                checks: vec![ChadexTaskProcess {
                    executable: "git".to_string(),
                    args: vec!["diff".to_string(), "--check".to_string()],
                    stdin: None,
                    cwd: None,
                    timeout_secs: Some(10),
                }],
            },
            ChadexTaskStep::Review {
                include_diff: Some(false),
                max_hunks: Some(8),
                max_hunk_lines: Some(80),
            },
        ],
        policy: Some(ChadexTaskPolicy {
            max_steps: Some(3),
            max_mutations: Some(1),
            max_changed_files: Some(1),
            timeout_secs: Some(30),
            max_result_bytes: Some(32 * 1024),
            ..Default::default()
        }),
        acceptance: None,
        session_id: None,
    };

    let (result, _, saw_guarded_edit) =
        drive_task_with_source_mutation_and_index_change(&runtime, call, Some(&root), true).await;
    assert!(result.success, "{:?} {}", result.error, result.output);
    assert_eq!(result.output["status"], "completed");
    assert_eq!(result.output["execution_state"], "succeeded");
    assert_ne!(result.output["counters"]["reconciliation_applied"], true);
    assert!(saw_guarded_edit);
    assert_eq!(
        fs::read_to_string(root.join("value.txt")).unwrap(),
        "after\n"
    );
    assert!(!root.join("index-only.txt").exists());
    assert!(test_git(&root, &["status", "--porcelain"]).contains("index-only.txt"));
}

#[tokio::test]
async fn execute_task_reconciles_and_applies_compatible_same_file_change() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("fixture");
    fs::create_dir_all(&root).unwrap();
    init_git_repo(&root);
    commit_file(&root, "value.txt", "header\nbefore\nmiddle\ntail\n", "seed");

    let runtime = test_runtime();
    let project = register_isolated_test_project(&runtime, "same-file-reconcile", &root).await;
    let call = ToolCall::ExecuteTask {
        project,
        task_id: Some(format!("chadex_task_{}", "1".repeat(32))),
        goal: "Reconcile a compatible same-file user edit and land the validated result."
            .to_string(),
        preplan: None,
        package_count: None,
        steps: vec![
            ChadexTaskStep::Edit {
                changes: vec![unfenced_exact_edit("value.txt", "before", "after")],
                dry_run: Some(false),
            },
            ChadexTaskStep::Validate {
                checks: vec![ChadexTaskProcess {
                    executable: "git".to_string(),
                    args: vec!["diff".to_string(), "--check".to_string()],
                    stdin: None,
                    cwd: None,
                    timeout_secs: Some(10),
                }],
            },
            ChadexTaskStep::Review {
                include_diff: Some(false),
                max_hunks: Some(8),
                max_hunk_lines: Some(80),
            },
        ],
        policy: Some(ChadexTaskPolicy {
            max_steps: Some(3),
            max_mutations: Some(1),
            max_changed_files: Some(1),
            timeout_secs: Some(30),
            max_result_bytes: Some(32 * 1024),
            ..Default::default()
        }),
        acceptance: None,
        session_id: None,
    };

    let (result, _, saw_guarded_edit) = drive_task_with_source_mutation_mode(
        &runtime,
        call,
        Some(&root),
        SourceMutationMode::SameFileCompatible,
    )
    .await;
    assert!(result.success, "{:?} {}", result.error, result.output);
    assert_eq!(result.output["status"], "completed");
    assert_eq!(result.output["execution_state"], "succeeded");
    assert_eq!(result.output["counters"]["reconciliation_applied"], true);
    assert!(saw_guarded_edit);
    assert_eq!(
        fs::read_to_string(root.join("value.txt")).unwrap(),
        "header\nafter\nmiddle\ntail\nuser-tail\n"
    );
}

#[tokio::test]
async fn execute_task_blocks_true_same_file_conflict_without_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("fixture");
    fs::create_dir_all(&root).unwrap();
    init_git_repo(&root);
    commit_file(&root, "value.txt", "before\n", "seed");

    let runtime = test_runtime();
    let project = register_isolated_test_project(&runtime, "same-file-conflict", &root).await;
    let call = ToolCall::ExecuteTask {
        project,
        task_id: Some(format!("chadex_task_{}", "2".repeat(32))),
        goal: "Fail closed when the user changes the same hunk during execution.".to_string(),
        preplan: None,
        package_count: None,
        steps: vec![
            ChadexTaskStep::Edit {
                changes: vec![unfenced_exact_edit("value.txt", "before", "after")],
                dry_run: Some(false),
            },
            ChadexTaskStep::Validate {
                checks: vec![ChadexTaskProcess {
                    executable: "git".to_string(),
                    args: vec!["diff".to_string(), "--check".to_string()],
                    stdin: None,
                    cwd: None,
                    timeout_secs: Some(10),
                }],
            },
            ChadexTaskStep::Review {
                include_diff: Some(false),
                max_hunks: Some(8),
                max_hunk_lines: Some(80),
            },
        ],
        policy: Some(ChadexTaskPolicy {
            max_steps: Some(3),
            max_mutations: Some(1),
            max_changed_files: Some(1),
            timeout_secs: Some(30),
            max_result_bytes: Some(32 * 1024),
            ..Default::default()
        }),
        acceptance: None,
        session_id: None,
    };

    let (result, _, saw_guarded_edit) = drive_task_with_source_mutation_mode(
        &runtime,
        call,
        Some(&root),
        SourceMutationMode::SameFileConflict,
    )
    .await;
    assert!(!result.success);
    assert_eq!(result.output["status"], "blocked");
    assert_eq!(result.output["failure"]["kind"], "workspace_source_overlap");
    assert!(saw_guarded_edit);
    assert_eq!(
        fs::read_to_string(root.join("value.txt")).unwrap(),
        "user-version\n"
    );
}

#[tokio::test]
async fn execute_task_ignores_unrelated_graphify_metadata_refresh() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("fixture");
    fs::create_dir_all(root.join("graphify-out/cache")).unwrap();
    init_git_repo(&root);
    commit_file(&root, "value.txt", "before\n", "seed");
    fs::write(root.join("graphify-out/manifest.json"), "{\"seen\":1}\n").unwrap();
    fs::write(
        root.join("graphify-out/cache/stat-index.json"),
        "{\"seen\":1}\n",
    )
    .unwrap();

    let runtime = test_runtime();
    let project = register_isolated_test_project(&runtime, "graphify-refresh", &root).await;
    let call = ToolCall::ExecuteTask {
        project,
        task_id: Some(format!("chadex_task_{}", "3".repeat(32))),
        goal: "Land a source edit despite unrelated Graphify metadata refresh.".to_string(),
        preplan: None,
        package_count: None,
        steps: vec![
            ChadexTaskStep::Edit {
                changes: vec![unfenced_exact_edit("value.txt", "before", "after")],
                dry_run: Some(false),
            },
            ChadexTaskStep::Validate {
                checks: vec![ChadexTaskProcess {
                    executable: "git".to_string(),
                    args: vec!["diff".to_string(), "--check".to_string()],
                    stdin: None,
                    cwd: None,
                    timeout_secs: Some(10),
                }],
            },
            ChadexTaskStep::Review {
                include_diff: Some(false),
                max_hunks: Some(8),
                max_hunk_lines: Some(80),
            },
        ],
        policy: Some(ChadexTaskPolicy {
            max_steps: Some(3),
            max_mutations: Some(1),
            max_changed_files: Some(1),
            timeout_secs: Some(30),
            max_result_bytes: Some(32 * 1024),
            ..Default::default()
        }),
        acceptance: None,
        session_id: None,
    };

    let (result, _, _) = drive_task_with_source_mutation_mode(
        &runtime,
        call,
        Some(&root),
        SourceMutationMode::GraphifyMetadata,
    )
    .await;
    assert!(result.success, "{:?} {}", result.error, result.output);
    assert_eq!(result.output["status"], "completed");
    assert_eq!(result.output["execution_state"], "succeeded");
    assert_ne!(result.output["counters"]["reconciliation_applied"], true);
    assert_eq!(
        fs::read_to_string(root.join("value.txt")).unwrap(),
        "after\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("graphify-out/manifest.json")).unwrap(),
        "{\"seen\":2}\n"
    );
}

#[tokio::test]
async fn concurrent_independent_tasks_both_apply_without_false_source_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("fixture");
    fs::create_dir_all(&root).unwrap();
    init_git_repo(&root);
    commit_file(&root, "a.txt", "a-before\n", "seed-a");
    commit_file(&root, "b.txt", "b-before\n", "seed-b");

    let runtime = test_runtime();
    let project = register_isolated_test_project(&runtime, "concurrent-independent", &root).await;
    let make_call = |task_id: String, path: &str, old: &str, new: &str| ToolCall::ExecuteTask {
        project: project.clone(),
        task_id: Some(task_id),
        goal: format!("Edit {path} independently while another task edits a disjoint file."),
        preplan: None,
        package_count: None,
        steps: vec![
            ChadexTaskStep::Edit {
                changes: vec![unfenced_exact_edit(path, old, new)],
                dry_run: Some(false),
            },
            ChadexTaskStep::Validate {
                checks: vec![ChadexTaskProcess {
                    executable: "git".to_string(),
                    args: vec!["diff".to_string(), "--check".to_string()],
                    stdin: None,
                    cwd: None,
                    timeout_secs: Some(10),
                }],
            },
            ChadexTaskStep::Review {
                include_diff: Some(false),
                max_hunks: Some(8),
                max_hunk_lines: Some(80),
            },
        ],
        policy: Some(ChadexTaskPolicy {
            max_steps: Some(3),
            max_mutations: Some(1),
            max_changed_files: Some(1),
            timeout_secs: Some(30),
            max_result_bytes: Some(32 * 1024),
            ..Default::default()
        }),
        acceptance: None,
        session_id: None,
    };
    let call_a = make_call(
        format!("chadex_task_{}", "4".repeat(32)),
        "a.txt",
        "a-before",
        "a-after",
    );
    let call_b = make_call(
        format!("chadex_task_{}", "5".repeat(32)),
        "b.txt",
        "b-before",
        "b-after",
    );

    let (result_a, result_b, kinds) = drive_two_tasks(&runtime, call_a, call_b).await;
    assert!(
        result_a.success,
        "A {:?} {}",
        result_a.error, result_a.output
    );
    assert!(
        result_b.success,
        "B {:?} {}",
        result_b.error, result_b.output
    );
    assert_eq!(result_a.output["status"], "completed");
    assert_eq!(result_b.output["status"], "completed");
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "a-after\n");
    assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "b-after\n");
    let a_start = result_a.output["started_at_ms"].as_i64().unwrap();
    let b_start = result_b.output["started_at_ms"].as_i64().unwrap();
    let a_finish = result_a.output["finished_at_ms"].as_i64().unwrap();
    let b_finish = result_b.output["finished_at_ms"].as_i64().unwrap();
    assert!(
        a_start < b_finish && b_start < a_finish,
        "tasks did not overlap"
    );
    assert!(
        kinds
            .iter()
            .filter(|kind| kind.as_str() == "prepare_managed_worktree")
            .count()
            >= 2
    );
}

#[tokio::test]
async fn execute_tasks_single_call_runs_independent_tasks_concurrently() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("fixture");
    fs::create_dir_all(&root).unwrap();
    init_git_repo(&root);
    commit_file(&root, "a.txt", "a-before\n", "seed-a");
    commit_file(&root, "b.txt", "b-before\n", "seed-b");

    let runtime = test_runtime();
    let project = register_isolated_test_project(&runtime, "batch-concurrent", &root).await;
    let make_item = |task_id: String, path: &str, old: &str, new: &str| ChadexTaskBatchItem {
        task_id: Some(task_id),
        goal: format!("Edit {path} independently in one execute_tasks batch."),
        preplan: None,
        package_count: None,
        steps: vec![
            ChadexTaskStep::Edit {
                changes: vec![unfenced_exact_edit(path, old, new)],
                dry_run: Some(false),
            },
            ChadexTaskStep::Validate {
                checks: vec![ChadexTaskProcess {
                    executable: "git".to_string(),
                    args: vec!["diff".to_string(), "--check".to_string()],
                    stdin: None,
                    cwd: None,
                    timeout_secs: Some(10),
                }],
            },
            ChadexTaskStep::Review {
                include_diff: Some(false),
                max_hunks: Some(8),
                max_hunk_lines: Some(80),
            },
        ],
        policy: Some(ChadexTaskPolicy {
            max_steps: Some(3),
            max_mutations: Some(1),
            max_changed_files: Some(1),
            timeout_secs: Some(30),
            max_result_bytes: Some(32 * 1024),
            ..Default::default()
        }),
        acceptance: None,
        session_id: None,
    };
    let call = ToolCall::ExecuteTasks {
        project,
        tasks: vec![
            make_item(
                format!("chadex_task_{}", "6".repeat(32)),
                "a.txt",
                "a-before",
                "a-after",
            ),
            make_item(
                format!("chadex_task_{}", "7".repeat(32)),
                "b.txt",
                "b-before",
                "b-after",
            ),
        ],
    };

    let (result, kinds, _) = drive_task(&runtime, call).await;
    assert!(result.success, "{:?} {}", result.error, result.output);
    assert_eq!(result.output["requested_count"], 2);
    assert_eq!(result.output["succeeded_count"], 2);
    assert_eq!(result.output["failed_count"], 0);
    let items = result.output["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|item| item["success"] == true));
    let a = &items[0]["output"];
    let b = &items[1]["output"];
    let a_start = a["started_at_ms"].as_i64().unwrap();
    let b_start = b["started_at_ms"].as_i64().unwrap();
    let a_finish = a["finished_at_ms"].as_i64().unwrap();
    let b_finish = b["finished_at_ms"].as_i64().unwrap();
    assert!(
        a_start < b_finish && b_start < a_finish,
        "batched tasks did not overlap"
    );
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "a-after\n");
    assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "b-after\n");
    assert!(
        kinds
            .iter()
            .filter(|kind| kind.as_str() == "prepare_managed_worktree")
            .count()
            >= 2
    );
}

#[tokio::test]
async fn execute_task_runs_independent_packages_through_isolated_integration() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("fixture");
    fs::create_dir_all(&root).unwrap();
    init_git_repo(&root);
    commit_file(&root, "a.txt", "a-before\n", "seed-a");
    commit_file(&root, "b.txt", "b-before\n", "seed-b");

    // Graphify is intentionally an uncommitted planning artifact: the source
    // HEAD remains the commit named by built_at_commit, while the dirty
    // snapshot carries the artifact into every isolated worktree.
    let head = test_git(&root, &["rev-parse", "HEAD"]);
    fs::create_dir_all(root.join("graphify-out")).unwrap();
    fs::write(
        root.join("graphify-out/graph.json"),
        json!({
            "built_at_commit": head,
            "nodes": [
                {"id": "a", "source_file": "a.txt"},
                {"id": "b", "source_file": "b.txt"}
            ],
            "links": []
        })
        .to_string(),
    )
    .unwrap();

    let runtime = test_runtime();
    let project = register_isolated_test_project(&runtime, "parallel-packages", &root).await;
    let validation = || ChadexTaskStep::Validate {
        checks: vec![ChadexTaskProcess {
            executable: "git".to_string(),
            args: vec!["diff".to_string(), "--check".to_string()],
            stdin: None,
            cwd: None,
            timeout_secs: Some(10),
        }],
    };
    let call = ToolCall::ExecuteTask {
        project,
        task_id: Some(format!("chadex_task_{}", "e".repeat(32))),
        goal: "Edit two independent files and validate the integrated result.".to_string(),
        preplan: Some(ChadexTaskPreplan {
            estimated_file_count: 4,
            estimated_subsystem_count: 4,
            estimated_language_count: 1,
            validation_domain_count: 3,
            cross_runtime_boundary: false,
            stateful_or_schema_change: true,
            concurrency_or_security_sensitive: true,
            graphify_informed: true,
        }),
        package_count: Some(2),
        steps: vec![
            ChadexTaskStep::Edit {
                changes: vec![unfenced_exact_edit("a.txt", "a-before", "a-after")],
                dry_run: Some(false),
            },
            ChadexTaskStep::RunProcess {
                executable: "/usr/bin/true".to_string(),
                args: Vec::new(),
                stdin: None,
                cwd: None,
                timeout_secs: Some(120),
                purpose: Some(ExecutionPurpose::Build),
            },
            validation(),
            ChadexTaskStep::Edit {
                changes: vec![unfenced_exact_edit("b.txt", "b-before", "b-after")],
                dry_run: Some(false),
            },
            ChadexTaskStep::RunProcess {
                executable: "/usr/bin/true".to_string(),
                args: Vec::new(),
                stdin: None,
                cwd: None,
                timeout_secs: Some(120),
                purpose: Some(ExecutionPurpose::Build),
            },
            validation(),
            ChadexTaskStep::Review {
                include_diff: Some(false),
                max_hunks: Some(8),
                max_hunk_lines: Some(80),
            },
        ],
        policy: Some(ChadexTaskPolicy {
            max_steps: Some(7),
            max_mutations: Some(2),
            max_changed_files: Some(2),
            timeout_secs: Some(45),
            max_result_bytes: Some(32 * 1024),
            ..Default::default()
        }),
        acceptance: None,
        session_id: None,
    };

    let (result, _, saw_guarded_edit) = drive_task(&runtime, call).await;
    assert!(result.success, "{:?} {}", result.error, result.output);
    assert_eq!(result.output["status"], "completed");
    assert_eq!(result.output["execution_state"], "succeeded");
    assert_eq!(result.output["packaging"]["graphify_status"], "used");
    assert_eq!(result.output["packaging"]["effective_package_count"], 2);
    assert_eq!(
        result.output["packaging"]["package_parallelism_proven"],
        true
    );
    assert_eq!(
        result.output["packaging"]["parallel_cost_gate_passed"],
        true
    );
    assert!(
        result.output["packaging"]["estimated_parallel_gain_pct"]
            .as_f64()
            .unwrap_or(0.0)
            >= 15.0
    );
    assert_eq!(result.output["workspace"]["mode"], "integration");
    assert_eq!(result.output["workspace"]["state"], "cleaned");
    assert!(saw_guarded_edit);
    assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "a-after\n");
    assert_eq!(fs::read_to_string(root.join("b.txt")).unwrap(), "b-after\n");
}

#[tokio::test]
async fn execute_task_preplan_defaults_to_recommended_package_count() {
    let dir = tempfile::tempdir().unwrap();
    init_git_repo(dir.path());
    commit_file(dir.path(), "a.txt", "a\n", "seed-a");
    commit_file(dir.path(), "b.txt", "b\n", "seed-b");
    commit_file(dir.path(), "c.txt", "c\n", "seed-c");
    commit_file(dir.path(), "d.txt", "d\n", "seed-d");

    let runtime = test_runtime();
    let project = register_isolated_test_project(&runtime, "preplan-default", dir.path()).await;
    let validation = || ChadexTaskStep::Validate {
        checks: vec![ChadexTaskProcess {
            executable: "git".to_string(),
            args: vec!["diff".to_string(), "--check".to_string()],
            stdin: None,
            cwd: None,
            timeout_secs: Some(10),
        }],
    };
    let read = |path: &str| ChadexTaskStep::Read {
        items: vec![ReadFilesItem {
            path: path.to_string(),
            start_line: None,
            limit: None,
            expected_read_revision: None,
        }],
        with_line_numbers: Some(false),
        max_result_bytes: Some(8 * 1024),
    };
    let call = ToolCall::ExecuteTask {
        project,
        task_id: None,
        goal: "Exercise pre-plan complexity without an explicit package override.".to_string(),
        preplan: Some(ChadexTaskPreplan {
            estimated_file_count: 4,
            estimated_subsystem_count: 4,
            estimated_language_count: 1,
            validation_domain_count: 3,
            cross_runtime_boundary: false,
            stateful_or_schema_change: true,
            concurrency_or_security_sensitive: true,
            graphify_informed: false,
        }),
        package_count: None,
        steps: vec![
            read("a.txt"),
            validation(),
            read("b.txt"),
            validation(),
            read("c.txt"),
            validation(),
            read("d.txt"),
            ChadexTaskStep::Review {
                include_diff: Some(false),
                max_hunks: Some(4),
                max_hunk_lines: Some(40),
            },
        ],
        policy: Some(ChadexTaskPolicy {
            max_steps: Some(8),
            max_mutations: Some(1),
            max_changed_files: Some(4),
            timeout_secs: Some(30),
            max_result_bytes: Some(32 * 1024),
            ..Default::default()
        }),
        acceptance: None,
        session_id: None,
    };

    let (result, _, _) = drive_task(&runtime, call).await;
    assert!(result.success, "{:?} {}", result.error, result.output);
    assert_eq!(result.output["complexity"]["score"], 7);
    assert_eq!(result.output["complexity"]["band"], "large");
    assert_eq!(result.output["complexity"]["recommended_task_count"], 3);
    assert_eq!(
        result.output["complexity"]["signals"]["estimation_source"],
        "preplan"
    );
    assert_eq!(result.output["packaging"]["requested_package_count"], 3);
    assert_eq!(result.output["packaging"]["recommended_package_count"], 3);
    assert_eq!(result.output["packaging"]["effective_package_count"], 3);
    assert_eq!(result.output["packaging"]["outer_execute_task_count"], 1);
}

#[tokio::test]
async fn execute_task_validation_pipeline_stops_on_first_failure() {
    let dir = tempfile::tempdir().unwrap();
    init_git_repo(dir.path());
    commit_file(dir.path(), "README.md", "fixture\n", "seed");
    let runtime = test_runtime();
    let project = register_isolated_test_project(&runtime, "validation", dir.path()).await;
    let call = ToolCall::ExecuteTask {
        project,
        task_id: None,
        goal: "Run the declared validation checks and stop on the first failure.".to_string(),
        preplan: None,
        package_count: None,
        steps: vec![ChadexTaskStep::Validate {
            checks: vec![
                ChadexTaskProcess {
                    executable: "git".to_string(),
                    args: vec![
                        "rev-parse".to_string(),
                        "--verify".to_string(),
                        "HEAD".to_string(),
                    ],
                    stdin: None,
                    cwd: None,
                    timeout_secs: Some(10),
                },
                ChadexTaskProcess {
                    executable: "git".to_string(),
                    args: vec![
                        "rev-parse".to_string(),
                        "--verify".to_string(),
                        "refs/heads/definitely-missing".to_string(),
                    ],
                    stdin: None,
                    cwd: None,
                    timeout_secs: Some(10),
                },
                ChadexTaskProcess {
                    executable: "git".to_string(),
                    args: vec!["status".to_string(), "--short".to_string()],
                    stdin: None,
                    cwd: None,
                    timeout_secs: Some(10),
                },
            ],
        }],
        policy: None,
        acceptance: Some(ChadexTaskAcceptance {
            require_validation_success: Some(true),
            require_review: Some(true),
        }),
        session_id: None,
    };

    let (result, kinds, _) = drive_task(&runtime, call).await;
    assert!(!result.success, "failed validation must fail execute_task");
    assert_eq!(result.output["status"], "failed_validation");
    assert_eq!(result.output["validation"]["status"], "failed");
    assert_eq!(result.output["validation"]["checks_passed"], 1);
    assert_eq!(result.output["validation"]["checks_failed"], 1);
    assert_eq!(result.output["failure"]["category"], "validation");
    assert_eq!(result.output["failure"]["failed_check"], 1);
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| kind.as_str() == "run_process")
            .count(),
        2,
        "third validation check must not run after failure"
    );
}

#[tokio::test]
async fn execute_task_resumes_same_promoted_process_job_through_review() {
    let dir = tempfile::tempdir().unwrap();
    init_git_repo(dir.path());
    commit_file(dir.path(), "README.md", "fixture\n", "seed");
    let runtime = test_runtime().with_structured_execution_sync_wait(Duration::from_millis(50));
    let project =
        register_isolated_async_test_project(&runtime, "promoted-process", dir.path()).await;
    let call = ToolCall::ExecuteTask {
        project,
        task_id: Some(format!("chadex_task_{}", "9".repeat(32))),
        goal: "Run a process that outlives the synchronous wait, then review the same task."
            .to_string(),
        preplan: None,
        package_count: None,
        steps: vec![
            ChadexTaskStep::RunProcess {
                executable: "/usr/bin/true".to_string(),
                args: Vec::new(),
                stdin: None,
                cwd: None,
                timeout_secs: Some(61),
                purpose: Some(ExecutionPurpose::Build),
            },
            ChadexTaskStep::Review {
                include_diff: Some(false),
                max_hunks: Some(4),
                max_hunk_lines: Some(40),
            },
        ],
        policy: Some(ChadexTaskPolicy {
            timeout_secs: Some(70),
            ..Default::default()
        }),
        acceptance: Some(ChadexTaskAcceptance {
            require_validation_success: Some(false),
            require_review: Some(true),
        }),
        session_id: None,
    };

    let (result, saw_promoted_job) = drive_task_with_promoted_process_job(&runtime, call).await;
    assert!(
        saw_promoted_job,
        "process must use the durable Job handoff path"
    );
    assert!(result.success, "{:?} {}", result.error, result.output);
    assert_eq!(result.output["status"], "completed");
    assert_eq!(result.output["execution_state"], "succeeded");
    assert_eq!(result.output["review"]["status"], "completed");
    assert_ne!(result.output["failure"]["kind"], "job_observation_required");
}

#[tokio::test]
async fn execute_task_cancel_stops_same_promoted_job_before_terminal_cancel() {
    let dir = tempfile::tempdir().unwrap();
    init_git_repo(dir.path());
    commit_file(dir.path(), "README.md", "fixture\n", "seed");
    let runtime = test_runtime().with_structured_execution_sync_wait(Duration::from_millis(50));
    let project =
        register_isolated_async_test_project(&runtime, "cancel-promoted-process", dir.path()).await;
    let task_id = format!("chadex_task_{}", "8".repeat(32));
    let call = ToolCall::ExecuteTask {
        project: project.clone(),
        task_id: Some(task_id.clone()),
        goal: "Cancel a promoted process and stop that exact Job before the task becomes terminal."
            .to_string(),
        preplan: None,
        package_count: None,
        steps: vec![
            ChadexTaskStep::RunProcess {
                executable: "/usr/bin/true".to_string(),
                args: Vec::new(),
                stdin: None,
                cwd: None,
                timeout_secs: Some(61),
                purpose: Some(ExecutionPurpose::Build),
            },
            ChadexTaskStep::Review {
                include_diff: Some(false),
                max_hunks: Some(4),
                max_hunk_lines: Some(40),
            },
        ],
        policy: Some(ChadexTaskPolicy {
            timeout_secs: Some(70),
            ..Default::default()
        }),
        acceptance: Some(ChadexTaskAcceptance {
            require_validation_success: Some(false),
            require_review: Some(true),
        }),
        session_id: None,
    };

    let (result, saw_promoted_job, saw_stop_job) =
        drive_cancelled_task_with_promoted_process_job(&runtime, call, &project, &task_id).await;
    assert!(saw_promoted_job, "process must promote before cancellation");
    assert!(
        saw_stop_job,
        "task cancellation must stop the same promoted Job"
    );
    assert!(!result.success);
    assert_eq!(result.output["status"], "cancelled");
    assert_eq!(result.output["execution_state"], "cancelled");
    assert_eq!(result.output["failure"]["reason"], "cancelled");
    assert_eq!(result.output["failure"]["category"], "cancelled");
    assert_eq!(result.output["failure"]["retry_strategy"], "no_retry");
    assert_eq!(result.output["review"]["status"], "not_run");
}

#[tokio::test]
async fn execute_task_survives_request_drop_and_finishes_same_execution() {
    let dir = tempfile::tempdir().unwrap();
    init_git_repo(dir.path());
    commit_file(dir.path(), "README.md", "fixture\n", "seed");
    let runtime = test_runtime().with_structured_execution_sync_wait(Duration::from_millis(50));
    let project =
        register_isolated_async_test_project(&runtime, "dropped-request-process", dir.path()).await;
    let task_id = format!("chadex_task_{}", "7".repeat(32));
    let call = ToolCall::ExecuteTask {
        project: project.clone(),
        task_id: Some(task_id.clone()),
        goal: "Finish the same execution after the request observer disconnects.".to_string(),
        preplan: None,
        package_count: None,
        steps: vec![
            ChadexTaskStep::RunProcess {
                executable: "/usr/bin/true".to_string(),
                args: Vec::new(),
                stdin: None,
                cwd: None,
                timeout_secs: Some(61),
                purpose: Some(ExecutionPurpose::Build),
            },
            ChadexTaskStep::Review {
                include_diff: Some(false),
                max_hunks: Some(4),
                max_hunk_lines: Some(40),
            },
        ],
        policy: Some(ChadexTaskPolicy {
            timeout_secs: Some(70),
            ..Default::default()
        }),
        acceptance: Some(ChadexTaskAcceptance {
            require_validation_success: Some(false),
            require_review: Some(true),
        }),
        session_id: None,
    };

    let observed =
        drive_disconnected_task_with_promoted_process_job(&runtime, call, &project, &task_id).await;
    assert_eq!(observed.output["task_id"], task_id);
    assert_eq!(observed.output["execution_id"], task_id);
    assert_eq!(observed.output["status"], "completed");
    assert_eq!(observed.output["execution_state"], "succeeded");
    assert_eq!(observed.output["review"]["status"], "completed");
}

#[tokio::test]
async fn execute_task_process_step_requires_review_before_runner_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = test_runtime();
    let project =
        register_runner_project_at_path(&runtime, CLIENT, "process-review", dir.path()).await;
    let auth = auth_context(None, true);
    let result = runtime
        .dispatch_with_auth(
            ToolCall::ExecuteTask {
                project,
                task_id: None,
                goal: "Run one native process without a final workspace review.".to_string(),
                preplan: None,
                package_count: None,
                steps: vec![ChadexTaskStep::RunProcess {
                    executable: "git".to_string(),
                    args: vec!["status".to_string(), "--short".to_string()],
                    stdin: None,
                    cwd: None,
                    timeout_secs: Some(10),
                    purpose: None,
                }],
                policy: None,
                acceptance: Some(acceptance_without_closeout()),
                session_id: None,
            },
            Some(&auth),
        )
        .await;
    assert!(!result.success);
    assert_eq!(result.output["error_kind"], "task_preflight_rejected");
    assert_eq!(result.output["reason_code"], "process_step_requires_review");
    assert_eq!(result.output["state_changed"], false);
    let pending = runtime
        .runner_registry
        .poll(RunnerPollRequest {
            client_id: CLIENT.to_string(),
            runner_instance_id: "inst".to_string(),
        })
        .await
        .unwrap();
    assert!(
        pending.is_none(),
        "rejected process task must not dispatch Runner work"
    );
}

#[tokio::test]
async fn execute_task_path_scope_rejects_escape_before_runner_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir_all(dir.path().join("Sources")).unwrap();
    fs::write(dir.path().join("Sources/App.swift"), "struct App {}\n").unwrap();
    let runtime = test_runtime();
    let project = register_runner_project_at_path(&runtime, CLIENT, "scope", dir.path()).await;
    let auth = auth_context(None, true);
    let result = runtime
        .dispatch_with_auth(
            ToolCall::ExecuteTask {
                project,
                task_id: None,
                goal: "Read one scoped source file.".to_string(),
                preplan: None,
                package_count: None,
                steps: vec![ChadexTaskStep::Read {
                    items: vec![ReadFilesItem {
                        path: "../secret.txt".to_string(),
                        start_line: None,
                        limit: None,
                        expected_read_revision: None,
                    }],
                    with_line_numbers: None,
                    max_result_bytes: None,
                }],
                policy: Some(ChadexTaskPolicy {
                    allowed_path_prefixes: vec!["Sources".to_string()],
                    ..Default::default()
                }),
                acceptance: Some(acceptance_without_closeout()),
                session_id: None,
            },
            Some(&auth),
        )
        .await;
    assert!(!result.success);
    assert_eq!(result.output["error_kind"], "task_preflight_rejected");
    assert_eq!(result.output["reason_code"], "read_path_outside_task_scope");
    assert_eq!(result.output["state_changed"], false);
    let pending = runtime
        .runner_registry
        .poll(RunnerPollRequest {
            client_id: CLIENT.to_string(),
            runner_instance_id: "inst".to_string(),
        })
        .await
        .unwrap();
    assert!(
        pending.is_none(),
        "preflight rejection must not dispatch Runner work"
    );
}

#[tokio::test]
async fn execute_task_cancellation_is_cooperative_between_steps() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "a\n").unwrap();
    fs::write(dir.path().join("b.txt"), "b\n").unwrap();
    let runtime = test_runtime();
    let project = register_runner_project_at_path(&runtime, CLIENT, "cancel", dir.path()).await;
    let task_id = "chadex_task_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string();
    let call = ToolCall::ExecuteTask {
        project: project.clone(),
        task_id: Some(task_id.clone()),
        goal: "Read two files unless cancelled.".to_string(),
        preplan: None,
        package_count: None,
        steps: vec![
            ChadexTaskStep::Read {
                items: vec![ReadFilesItem {
                    path: "a.txt".to_string(),
                    start_line: None,
                    limit: None,
                    expected_read_revision: None,
                }],
                with_line_numbers: None,
                max_result_bytes: None,
            },
            ChadexTaskStep::Read {
                items: vec![ReadFilesItem {
                    path: "b.txt".to_string(),
                    start_line: None,
                    limit: None,
                    expected_read_revision: None,
                }],
                with_line_numbers: None,
                max_result_bytes: None,
            },
        ],
        policy: None,
        acceptance: Some(acceptance_without_closeout()),
        session_id: None,
    };
    let task = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            let auth = auth_context(None, true);
            runtime.dispatch_with_auth(call, Some(&auth)).await
        }
    });

    let first = wait_for_patch_agent_request(&runtime, CLIENT).await;
    assert_eq!(first.kind, "file_read");
    let cancelling = runtime.cancel_chadex_task(&project, &task_id);
    assert!(cancelling.success);
    assert_eq!(cancelling.output["status"], "cancelling");
    assert_eq!(cancelling.output["execution_state"], "running");
    let (exit_code, stdout, stderr) = run_runner_shell_request_locally(&first);
    complete_patch_agent_request(
        &runtime,
        CLIENT,
        &first.request_id,
        exit_code,
        &stdout,
        &stderr,
    )
    .await;

    let result = task.await.unwrap();
    assert!(!result.success);
    assert_eq!(result.output["status"], "cancelled");
    assert_eq!(result.output["execution_state"], "cancelled");
    assert_eq!(result.output["completed_steps"], 1);
    assert_eq!(result.output["cancel_requested"], true);
    let pending = runtime
        .runner_registry
        .poll(RunnerPollRequest {
            client_id: CLIENT.to_string(),
            runner_instance_id: "inst".to_string(),
        })
        .await
        .unwrap();
    assert!(
        pending.is_none(),
        "second read must not start after cancellation"
    );
}

#[tokio::test]
async fn execute_task_runner_disconnect_with_generic_io_fails_closed_without_retry() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "a\n").unwrap();
    let runtime = test_runtime();
    let project =
        register_runner_project_at_path(&runtime, CLIENT, "retry-disconnect", dir.path()).await;
    let task_id = "chadex_task_cccccccccccccccccccccccccccccccc".to_string();
    let call = ToolCall::ExecuteTask {
        project: project.clone(),
        task_id: Some(task_id),
        goal: "Read once with only the bounded safe transient retry policy.".to_string(),
        preplan: None,
        package_count: None,
        steps: vec![ChadexTaskStep::Read {
            items: vec![ReadFilesItem {
                path: "a.txt".to_string(),
                start_line: None,
                limit: None,
                expected_read_revision: None,
            }],
            with_line_numbers: None,
            max_result_bytes: None,
        }],
        policy: None,
        acceptance: Some(acceptance_without_closeout()),
        session_id: None,
    };
    let task = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            let auth = auth_context(None, true);
            runtime.dispatch_with_auth(call, Some(&auth)).await
        }
    });

    let first = wait_for_patch_agent_request(&runtime, CLIENT).await;
    assert_eq!(first.kind, "file_read");
    runtime
        .runner_registry
        .reconcile_disconnect(CLIENT, "inst")
        .await;

    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("disconnect task should terminate")
        .unwrap();
    assert!(!result.success, "{:?} {}", result.error, result.output);
    assert_eq!(result.output["status"], "failed");
    assert_eq!(
        result.output["steps"][0]["attempts"], 1,
        "{}",
        result.output
    );
    assert_eq!(result.output["steps"][0]["retries"], 0);
    assert_eq!(result.output["counters"]["retries"], 0);
    assert_eq!(result.output["failure"]["error_kind"], "read_file_failed");
    assert_eq!(result.output["failure"]["reason_code"], "io_error");
    assert_eq!(result.output["failure"]["category"], "step");
}

#[tokio::test]
async fn execute_task_budget_timeout_blocks_before_next_step() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "a\n").unwrap();
    fs::write(dir.path().join("b.txt"), "b\n").unwrap();
    let runtime = test_runtime();
    let project =
        register_runner_project_at_path(&runtime, CLIENT, "budget-timeout", dir.path()).await;
    let call = ToolCall::ExecuteTask {
        project,
        task_id: None,
        goal: "Stop when the task-level time budget is exhausted.".to_string(),
        preplan: None,
        package_count: None,
        steps: vec![
            ChadexTaskStep::Read {
                items: vec![ReadFilesItem {
                    path: "a.txt".to_string(),
                    start_line: None,
                    limit: None,
                    expected_read_revision: None,
                }],
                with_line_numbers: None,
                max_result_bytes: None,
            },
            ChadexTaskStep::Read {
                items: vec![ReadFilesItem {
                    path: "b.txt".to_string(),
                    start_line: None,
                    limit: None,
                    expected_read_revision: None,
                }],
                with_line_numbers: None,
                max_result_bytes: None,
            },
        ],
        policy: Some(ChadexTaskPolicy {
            timeout_secs: Some(1),
            ..Default::default()
        }),
        acceptance: Some(acceptance_without_closeout()),
        session_id: None,
    };
    let task = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            let auth = auth_context(None, true);
            runtime.dispatch_with_auth(call, Some(&auth)).await
        }
    });

    let first = wait_for_patch_agent_request(&runtime, CLIENT).await;
    assert_eq!(first.kind, "file_read");
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let (exit_code, stdout, stderr) = run_runner_shell_request_locally(&first);
    complete_patch_agent_request(
        &runtime,
        CLIENT,
        &first.request_id,
        exit_code,
        &stdout,
        &stderr,
    )
    .await;

    let result = task.await.unwrap();
    assert!(!result.success);
    assert_eq!(result.output["status"], "blocked");
    assert_eq!(result.output["completed_steps"], 1);
    assert_eq!(result.output["failure"]["kind"], "task_timeout");
    assert_eq!(result.output["failure"]["category"], "timeout");
}

#[tokio::test]
async fn execute_task_never_retries_ambiguous_mutation_outcome() {
    let dir = tempfile::tempdir().unwrap();
    init_git_repo(dir.path());
    fs::write(dir.path().join("value.txt"), "before\n").unwrap();
    commit_file(dir.path(), "value.txt", "before\n", "seed");
    let runtime = test_runtime();
    let project = register_isolated_test_project(&runtime, "ambiguous-mutation", dir.path()).await;
    let call = ToolCall::ExecuteTask {
        project,
        task_id: None,
        goal: "Attempt one mutation and fail closed if its dispatched outcome becomes ambiguous."
            .to_string(),
        preplan: None,
        package_count: None,
        steps: vec![ChadexTaskStep::Edit {
            changes: vec![unfenced_exact_edit("value.txt", "before", "after")],
            dry_run: Some(false),
        }],
        policy: None,
        acceptance: Some(acceptance_without_closeout()),
        session_id: None,
    };
    let task = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            let auth = auth_context(None, true);
            runtime.dispatch_with_auth(call, Some(&auth)).await
        }
    });

    let mut worktree_parents = Vec::new();
    let guard = loop {
        let request = wait_for_patch_agent_request(&runtime, CLIENT).await;
        if request.kind == "file_read" {
            break request;
        }
        if matches!(
            request.kind.as_str(),
            "prepare_managed_worktree" | "project_lifecycle_unregister"
        ) {
            complete_test_project_operation(&runtime, &request, &mut worktree_parents).await;
        } else {
            let (exit_code, stdout, stderr) = run_runner_shell_request_locally(&request);
            complete_patch_agent_request(
                &runtime,
                CLIENT,
                &request.request_id,
                exit_code,
                &stdout,
                &stderr,
            )
            .await;
        }
    };
    let (exit_code, stdout, stderr) = run_runner_shell_request_locally(&guard);
    complete_patch_agent_request(
        &runtime,
        CLIENT,
        &guard.request_id,
        exit_code,
        &stdout,
        &stderr,
    )
    .await;

    let mutation = wait_for_patch_agent_request(&runtime, CLIENT).await;
    assert_eq!(mutation.kind, "file_apply_text_edits");
    runtime
        .runner_registry
        .reconcile_disconnect(CLIENT, "inst")
        .await;

    let result = tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("ambiguous mutation task should terminate")
        .unwrap();
    assert!(!result.success);
    assert_eq!(result.output["status"], "unknown");
    assert_eq!(result.output["execution_state"], "unknown");
    assert_eq!(result.output["steps"][0]["attempts"], 1);
    assert_eq!(result.output["steps"][0]["retries"], 0);
    assert_eq!(result.output["counters"]["retries"], 0);
    assert_eq!(result.output["failure"]["category"], "ambiguous_mutation");
}

#[tokio::test]
async fn phase12_small_read_fast_path_replay_and_late_cancel() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("small.txt"), "small\n").unwrap();
    let runtime = test_runtime();
    let project =
        register_runner_project_at_path(&runtime, CLIENT, "phase12-small", dir.path()).await;
    let id = "chadex_task_cccccccccccccccccccccccccccccccc";
    let call = || ToolCall::ExecuteTask {
        project: project.clone(),
        task_id: Some(id.into()),
        goal: "Read a small file".into(),
        preplan: None,
        package_count: None,
        steps: vec![ChadexTaskStep::Read {
            items: vec![ReadFilesItem {
                path: "small.txt".into(),
                start_line: None,
                limit: None,
                expected_read_revision: None,
            }],
            with_line_numbers: None,
            max_result_bytes: None,
        }],
        policy: None,
        acceptance: Some(acceptance_without_closeout()),
        session_id: None,
    };
    let (result, requests, _) = drive_task(&runtime, call()).await;
    assert!(result.success, "{result:?}");
    assert_eq!(result.output["execution_state"], "succeeded");
    assert_eq!(result.output["execution_id"], id);
    assert_eq!(result.output["workspace"]["mode"], "none");
    assert_eq!(result.output["packaging"]["effective_package_count"], 1);
    assert_eq!(requests, vec!["file_read"]);
    let auth = auth_context(None, true);
    let duplicate = runtime.dispatch_with_auth(call(), Some(&auth)).await;
    assert!(!duplicate.success);
    assert_eq!(duplicate.output["error_kind"], "execution_claim_rejected");
    // The dispatch adapter adds request-specific permission evidence. Compare
    // the canonical observation before/after cancellation, not that envelope.
    let before_cancel = runtime.observe_chadex_task(&project, id).output;
    assert_eq!(
        runtime.cancel_chadex_task(&project, id).output,
        before_cancel
    );
    assert_eq!(
        runtime.observe_chadex_task(&project, id).output,
        before_cancel
    );
    assert!(runtime
        .runner_registry
        .poll(RunnerPollRequest {
            client_id: CLIENT.into(),
            runner_instance_id: "inst".into(),
        })
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn phase12_cleanup_without_ownership_cannot_dispatch_a_removal() {
    let runtime = test_runtime();
    let mut workspace = super::super::execution_workspace::ExecutionWorkspace::read_only(
        "chadex_task_dddddddddddddddddddddddddddddddd".into(),
        "source".into(),
        "/unowned".into(),
    );
    workspace.mode = super::super::execution_workspace::IsolationMode::Task;
    workspace.execution_project = "other".into();
    assert_eq!(
        runtime
            .cleanup_execution_workspace(&mut workspace, None)
            .await
            .unwrap_err(),
        "execution_workspace_ownership_unknown"
    );
}

#[tokio::test]
async fn phase14b_cleanup_resumes_after_unregister_crash_without_losing_ownership() {
    use super::super::execution_workspace::{IsolationMode, WorkspaceCleanupPhase, WorkspaceState};

    let source = tempfile::tempdir().unwrap();
    init_git_repo(source.path());
    commit_file(source.path(), "README.md", "fixture\n", "seed");
    let runtime = test_runtime();
    let project = register_isolated_test_project(&runtime, "phase14b-cleanup", source.path()).await;
    let auth = auth_context(None, true);
    let task_id = "chadex_task_14141414141414141414141414141414";

    let prepare = tokio::spawn({
        let runtime = runtime.clone();
        let project = project.clone();
        let auth = auth.clone();
        async move {
            let resolved = runtime.resolve_project_input(&project).await.unwrap();
            runtime
                .prepare_execution_workspace(
                    &resolved,
                    task_id,
                    IsolationMode::Task,
                    None,
                    Some(&auth),
                )
                .await
        }
    });
    let mut worktree_parents = Vec::new();
    while !prepare.is_finished() {
        let request = runtime
            .runner_registry
            .poll(RunnerPollRequest {
                client_id: CLIENT.into(),
                runner_instance_id: "inst".into(),
            })
            .await
            .unwrap();
        let Some(request) = request else {
            tokio::time::sleep(Duration::from_millis(2)).await;
            continue;
        };
        if request.kind == "prepare_managed_worktree" {
            complete_test_project_operation(&runtime, &request, &mut worktree_parents).await;
        } else {
            let (exit_code, stdout, stderr) = run_runner_shell_request_locally(&request);
            complete_patch_agent_request(
                &runtime,
                CLIENT,
                &request.request_id,
                exit_code,
                &stdout,
                &stderr,
            )
            .await;
        }
    }
    let mut workspace = prepare.await.unwrap().unwrap();
    let worktree_path = workspace.root.clone();
    let revision = workspace.registered_revision.clone().unwrap();

    // Persisted cleanup intent exists before the unregister effect. Simulate a
    // process crash immediately after the unregister succeeds, before the next
    // phase can be written.
    workspace.cleanup_operation_id = Some("cleanup-after-unregister-crash".to_string());
    workspace.cleanup_phase = WorkspaceCleanupPhase::IntentPersisted;
    workspace.state = WorkspaceState::Preserved;

    let unregister = tokio::spawn({
        let runtime = runtime.clone();
        let execution_project = workspace.execution_project.clone();
        let auth = auth.clone();
        async move {
            runtime
                .unregister_project(execution_project, revision, Some(&auth))
                .await
        }
    });
    while !unregister.is_finished() {
        let request = runtime
            .runner_registry
            .poll(RunnerPollRequest {
                client_id: CLIENT.into(),
                runner_instance_id: "inst".into(),
            })
            .await
            .unwrap();
        let Some(request) = request else {
            tokio::time::sleep(Duration::from_millis(2)).await;
            continue;
        };
        assert_eq!(request.kind, "project_lifecycle_unregister");
        complete_test_project_operation(&runtime, &request, &mut worktree_parents).await;
    }
    let unregister = unregister.await.unwrap();
    assert!(unregister.success, "{:?}", unregister.error);
    assert!(runtime
        .resolve_project_for_auth(&workspace.execution_project, None)
        .await
        .is_err());

    let cleanup = tokio::spawn({
        let runtime = runtime.clone();
        let auth = auth.clone();
        async move {
            let result = runtime
                .cleanup_execution_workspace(&mut workspace, Some(&auth))
                .await;
            (result, workspace)
        }
    });
    while !cleanup.is_finished() {
        let request = runtime
            .runner_registry
            .poll(RunnerPollRequest {
                client_id: CLIENT.into(),
                runner_instance_id: "inst".into(),
            })
            .await
            .unwrap();
        let Some(request) = request else {
            tokio::time::sleep(Duration::from_millis(2)).await;
            continue;
        };
        let (exit_code, stdout, stderr) = run_runner_shell_request_locally(&request);
        complete_patch_agent_request(
            &runtime,
            CLIENT,
            &request.request_id,
            exit_code,
            &stdout,
            &stderr,
        )
        .await;
    }
    let (result, mut workspace) = cleanup.await.unwrap();
    result.unwrap();
    assert_eq!(workspace.cleanup_phase, WorkspaceCleanupPhase::Complete);
    assert_eq!(workspace.state, WorkspaceState::Cleaned);
    assert!(!Path::new(&worktree_path).exists());

    // A replay after another crash is a no-op and cannot dispatch a second
    // destructive removal.
    runtime
        .cleanup_execution_workspace(&mut workspace, Some(&auth))
        .await
        .unwrap();
    assert!(runtime
        .runner_registry
        .poll(RunnerPollRequest {
            client_id: CLIENT.into(),
            runner_instance_id: "inst".into(),
        })
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn phase14b_apply_success_with_cleanup_debt_remains_succeeded() {
    let dir = tempfile::tempdir().unwrap();
    init_git_repo(dir.path());
    commit_file(dir.path(), "value.txt", "before\n", "seed");
    let runtime = test_runtime();
    let project =
        register_isolated_test_project(&runtime, "phase14b-cleanup-debt", dir.path()).await;
    let call = ToolCall::ExecuteTask {
        project,
        task_id: Some("chadex_task_15151515151515151515151515151515".to_string()),
        goal: "Apply one edit and keep cleanup debt separate from the successful effect."
            .to_string(),
        preplan: None,
        package_count: None,
        steps: vec![ChadexTaskStep::Edit {
            changes: vec![unfenced_exact_edit("value.txt", "before", "after")],
            dry_run: Some(false),
        }],
        policy: None,
        acceptance: Some(acceptance_without_closeout()),
        session_id: None,
    };
    let task = tokio::spawn({
        let runtime = runtime.clone();
        async move {
            let auth = auth_context(None, true);
            runtime.dispatch_with_auth(call, Some(&auth)).await
        }
    });
    let mut worktree_parents = Vec::new();
    while !task.is_finished() {
        let request = runtime
            .runner_registry
            .poll(RunnerPollRequest {
                client_id: CLIENT.into(),
                runner_instance_id: "inst".into(),
            })
            .await
            .unwrap();
        let Some(request) = request else {
            tokio::time::sleep(Duration::from_millis(2)).await;
            continue;
        };
        if request.kind == "prepare_managed_worktree" {
            complete_test_project_operation(&runtime, &request, &mut worktree_parents).await;
        } else if request.kind == "file_apply_text_edits" {
            complete_task_mutation_request(&runtime, request).await;
        } else if request.kind == "project_lifecycle_unregister" {
            complete_patch_agent_request(
                &runtime,
                CLIENT,
                &request.request_id,
                1,
                "",
                "forced cleanup failure",
            )
            .await;
        } else {
            let (exit_code, stdout, stderr) = run_runner_shell_request_locally(&request);
            complete_patch_agent_request(
                &runtime,
                CLIENT,
                &request.request_id,
                exit_code,
                &stdout,
                &stderr,
            )
            .await;
        }
    }
    let result = task.await.unwrap();
    assert!(result.success, "{:?} {}", result.error, result.output);
    assert_eq!(result.output["status"], "completed");
    assert_eq!(result.output["execution_state"], "succeeded");
    assert_eq!(result.output["counters"]["cleanup_pending"], true);
    assert_eq!(result.output["workspace"]["cleanup_pending"], true);
    assert!(result.output.get("failure").is_none() || result.output["failure"].is_null());
    assert_eq!(
        fs::read_to_string(dir.path().join("value.txt")).unwrap(),
        "after\n"
    );
}

#[tokio::test]
async fn phase12_replayed_mutation_never_reaches_runner_twice() {
    let dir = tempfile::tempdir().unwrap();
    init_git_repo(dir.path());
    commit_file(dir.path(), "value.txt", "a\n", "seed");
    let runtime = test_runtime();
    let project = register_isolated_test_project(&runtime, "phase12-replay", dir.path()).await;
    let id = "chadex_task_eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    // This edit would still match after the first execution (a -> aa -> aaa).
    let call = || ToolCall::ExecuteTask {
        project: project.clone(),
        task_id: Some(id.into()),
        goal: "Append exactly once".into(),
        preplan: None,
        package_count: None,
        steps: vec![ChadexTaskStep::Edit {
            changes: vec![unfenced_exact_edit("value.txt", "a\n", "aa\n")],
            dry_run: Some(false),
        }],
        policy: None,
        acceptance: Some(acceptance_without_closeout()),
        session_id: None,
    };
    let (first, _, edited) = drive_task(&runtime, call()).await;
    assert!(first.success, "{first:?}");
    assert!(edited);
    let auth = auth_context(None, true);
    let duplicate = runtime.dispatch_with_auth(call(), Some(&auth)).await;
    assert!(!duplicate.success);
    assert_eq!(duplicate.output["error_kind"], "execution_claim_rejected");
    assert_eq!(
        fs::read_to_string(dir.path().join("value.txt")).unwrap(),
        "aa\n"
    );
    assert!(runtime
        .runner_registry
        .poll(RunnerPollRequest {
            client_id: CLIENT.into(),
            runner_instance_id: "inst".into(),
        })
        .await
        .unwrap()
        .is_none());
}
