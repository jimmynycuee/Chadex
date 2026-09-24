use super::*;

const ID: &str = "chadex_task_0123456789abcdef0123456789abcdef";
const RETRY: &str = "chadex_task_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn control(root: &Path) -> Arc<TaskControl> {
    let steps = vec![ChadexTaskStep::Read {
        items: vec![ReadFilesItem {
            path: "a.txt".into(),
            start_line: None,
            limit: None,
            expected_read_revision: None,
        }],
        with_line_numbers: None,
        max_result_bytes: None,
    }];
    Arc::new(TaskControl {
        snapshot: StdMutex::new(TaskSnapshot {
            execution: Execution::claim(root, ID, None).unwrap(),
            task_id: ID.into(),
            project: "project".into(),
            source_path: "/fixture".into(),
            goal: "read".into(),
            status: "queued".into(),
            current_step: 0,
            total_steps: 1,
            completed_steps: 0,
            plan: vec!["read".into()],
            cancel_requested: false,
            started_at_ms: now_ms(),
            finished_at_ms: None,
            duration_ms: None,
            limits: normalize_policy(None).unwrap(),
            counters: json!({}),
            validation: json!({}),
            review: json!({}),
            steps: vec![],
            failure: None,
            complexity: assess_task_complexity(None, &steps, 0, 0),
            packaging: plan_task_packages(&steps, None, 1).unwrap(),
            workspace: json!({"mode":"none", "state":"active"}),
        }),
        raw_log: StdMutex::new(()),
        cancel_requested: AtomicBool::new(false),
    })
}

#[test]
fn every_terminal_state_is_immutable_and_only_running_can_finish() {
    let terminals = [
        ExecutionState::Succeeded,
        ExecutionState::Failed,
        ExecutionState::Cancelled,
        ExecutionState::Interrupted,
        ExecutionState::Unknown,
    ];
    for terminal in terminals {
        let mut state = ExecutionState::Queued;
        assert!(state.transition(terminal).is_err());
        state.transition(ExecutionState::Running).unwrap();
        state.transition(terminal).unwrap();
        for next in terminals
            .into_iter()
            .chain([ExecutionState::Queued, ExecutionState::Running])
        {
            assert!(state.transition(next).is_err());
            assert_eq!(state, terminal);
        }
    }
}

#[test]
fn success_is_durable_before_observation_and_late_updates_do_nothing() {
    let root = tempfile::tempdir().unwrap();
    let control = control(root.path());
    update_snapshot(&control, |s| s.status = "running".into()).unwrap();
    update_snapshot(&control, |s| {
        s.status = "completed".into();
        s.finished_at_ms = Some(now_ms());
    })
    .unwrap();
    let receipt: Value = serde_json::from_slice(
        &fs::read(
            control
                .snapshot
                .lock()
                .unwrap()
                .execution
                .receipt_path
                .clone(),
        )
        .unwrap(),
    )
    .unwrap();
    let before = task_value(&control).unwrap();
    assert_eq!(before["execution_state"], "succeeded");
    assert_eq!(receipt["execution"]["state"], "succeeded");
    let mut durable_result = receipt["result"].clone();
    durable_result
        .as_object_mut()
        .unwrap()
        .remove("source_path");
    assert_eq!(durable_result, before);
    update_snapshot(&control, |s| {
        s.status = "cancelled".into();
        s.failure = Some(json!({"late":true}));
    })
    .unwrap();
    assert_eq!(task_value(&control).unwrap(), before);
    drop(ExecutionGuard(control.clone()));
    assert_eq!(task_value(&control).unwrap(), before);
}

#[test]
fn receipt_failure_never_publishes_success_and_replay_stays_fenced() {
    let root = tempfile::tempdir().unwrap();
    let control = control(root.path());
    update_snapshot(&control, |s| s.status = "running".into()).unwrap();
    let path = control
        .snapshot
        .lock()
        .unwrap()
        .execution
        .receipt_path
        .clone();
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap(); // deterministic persistence fault
    assert!(update_snapshot(&control, |s| s.status = "completed".into()).is_err());
    assert_eq!(task_value(&control).unwrap()["execution_state"], "unknown");
    assert!(!execute_task_result(&control).success);
    assert!(Execution::claim(root.path(), ID, None).is_err());
}

#[test]
fn duplicate_claims_racing_and_after_restart_cannot_repeat_mutation() {
    let root = tempfile::tempdir().unwrap();
    let mutations = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..8 {
            scope.spawn(|| {
                if Execution::claim(root.path(), ID, None).is_ok() {
                    mutations.fetch_add(1, Ordering::SeqCst);
                    fs::write(root.path().join("mutated"), b"once").unwrap();
                }
            });
        }
    });
    assert_eq!(mutations.load(Ordering::SeqCst), 1);
    assert!(Execution::claim(root.path(), ID, None).is_err());
    assert_eq!(fs::read(root.path().join("mutated")).unwrap(), b"once");
}

#[test]
fn retry_identity_and_lineage_are_durable_before_work() {
    let root = tempfile::tempdir().unwrap();
    let old = Execution::claim(root.path(), ID, None).unwrap();
    let new = Execution::claim(root.path(), RETRY, Some(old.execution_id.clone())).unwrap();
    assert_ne!(new.execution_id, old.execution_id);
    let receipt: Value = serde_json::from_slice(&fs::read(new.receipt_path).unwrap()).unwrap();
    assert_eq!(receipt["execution"]["previous_execution_id"], ID);
    assert_eq!(receipt["execution"]["state"], "queued");
}

#[test]
fn cancellation_is_only_a_request_until_work_stops() {
    let root = tempfile::tempdir().unwrap();
    let control = control(root.path());
    update_snapshot(&control, |s| s.status = "running".into()).unwrap();
    update_snapshot(&control, |s| {
        s.cancel_requested = true;
        s.status = "cancelling".into();
    })
    .unwrap();
    assert_eq!(task_value(&control).unwrap()["execution_state"], "running");
    update_snapshot(&control, |s| s.status = "cancelled".into()).unwrap();
    assert_eq!(
        task_value(&control).unwrap()["execution_state"],
        "cancelled"
    );
}

#[test]
fn dropped_mutating_future_is_unknown_but_read_only_is_interrupted() {
    for effects in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let control = control(root.path());
        update_snapshot(&control, |s| {
            s.status = "running".into();
            s.execution.effects_possible = effects;
        })
        .unwrap();
        drop(ExecutionGuard(control.clone()));
        assert_eq!(
            task_value(&control).unwrap()["execution_state"],
            if effects { "unknown" } else { "interrupted" }
        );
    }
}

#[test]
fn timeout_before_mutation_fails_and_unknown_outweighs_cancellation() {
    for uncertain in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let control = control(root.path());
        update_snapshot(&control, |s| {
            s.status = "running".into();
            s.execution.outcome_uncertain = uncertain;
        })
        .unwrap();
        update_snapshot(&control, |s| {
            s.status = if uncertain { "cancelled" } else { "blocked" }.into();
            s.failure = Some(json!({"kind":"task_timeout", "category":"timeout"}));
        })
        .unwrap();
        assert_eq!(
            task_value(&control).unwrap()["execution_state"],
            if uncertain { "unknown" } else { "failed" }
        );
    }
    let timeout = ToolResult::err_with_output("timeout", json!({"failure_kind":"runner_timeout"}));
    assert!(uncertain_result("edit", &timeout));
    assert!(uncertain_result("validate", &timeout));
    let pre_dispatch = ToolResult::err_with_output(
        "timeout",
        json!({"failure_kind":"runner_timeout", "state_changed":false}),
    );
    assert!(!uncertain_result("edit", &pre_dispatch));
    assert!(!uncertain_result("read", &timeout));
}

#[test]
fn startup_uses_receipt_over_stale_projection_without_rewriting_terminal() {
    for effects in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let control = control(root.path());
        update_snapshot(&control, |s| {
            s.status = "running".into();
            s.execution.effects_possible = effects;
        })
        .unwrap();
        let path = control
            .snapshot
            .lock()
            .unwrap()
            .execution
            .receipt_path
            .clone();
        assert!(
            load_recovered_task_projections_at(root.path()).is_empty(),
            "live owner must not be recovered"
        );
        drop(control); // simulate process loss without the graceful Drop guard
        let recovered = load_recovered_task_projections_at(root.path());
        let expected = if effects { "unknown" } else { "interrupted" };
        assert_eq!(recovered[ID]["execution_state"], expected);
        assert_eq!(recovered[ID]["recovery"]["actions"], json!(["discard"]));
        let before = fs::read(&path).unwrap();
        let again = load_recovered_task_projections_at(root.path());
        assert_eq!(again[ID]["execution_state"], expected);
        assert_eq!(fs::read(&path).unwrap(), before);
        assert!(Execution::claim(root.path(), ID, None).is_err());
    }
}

#[test]
fn unknown_read_result_is_never_automatically_retried() {
    let result = ToolResult::err_with_output(
        "timeout",
        json!({"execution_state":"outcome_unknown", "failure_kind":"runner_timeout"}),
    );
    assert!(!should_retry_read_only_failure("read", &result, 0));
}

#[test]
fn discard_suppresses_projection_but_keeps_permanent_replay_fence() {
    let root = tempfile::tempdir().unwrap();
    let control = control(root.path());
    update_snapshot(&control, |s| s.status = "running".into()).unwrap();
    let claim = root.path().join("executions").join(ID);
    write_private_file(&claim.join("discarded"), b"discarded").unwrap();
    assert!(load_recovered_task_projections_at(root.path()).is_empty());
    assert!(Execution::claim(root.path(), ID, None).is_err());
}

fn resolved() -> ResolvedProject {
    ResolvedProject {
        input: "project".into(),
        resolved_id: "project".into(),
        config: crate::projects::ProjectConfig {
            path: "/fixture".into(),
            client_id: "offline".into(),
            allow_patch: true,
        },
        root_fingerprint: None,
        knowledge_association: None,
    }
}

#[tokio::test]
async fn recovery_rejects_unknown_even_when_explicitly_requested() {
    let runtime = ToolRuntime::new_for_tests();
    runtime.chadex_tasks.update_recovered(
        ID,
        json!({"project":"project", "status":"unknown", "execution_state":"unknown"}),
    );
    let result = runtime
        .chadex_task_recovery(
            resolved(),
            Some(ID.into()),
            "retry".into(),
            None,
            sessions::SessionTransport::Api,
        )
        .await;
    assert_eq!(
        result.output["error_kind"],
        "unknown_execution_not_retryable"
    );
    assert_eq!(runtime.chadex_tasks.tasks.lock().unwrap().len(), 0);
}

#[tokio::test]
async fn existing_recovery_retry_links_new_execution_even_when_retry_fails() {
    let runtime = ToolRuntime::new_for_tests();
    let old = json!({"project":"project", "status":"interrupted", "execution_state":"interrupted"});
    runtime.chadex_tasks.update_recovered(ID, old.clone());
    let root = runtime.chadex_tasks.execution_root().unwrap();
    let plan = TaskRecoveryPlan {
        version: 1,
        project: "project".into(),
        task_id: ID.into(),
        goal: "retry read".into(),
        preplan: None,
        package_count: None,
        steps: vec![ChadexTaskStep::Read {
            items: vec![ReadFilesItem {
                path: "a.txt".into(),
                start_line: None,
                limit: None,
                expected_read_revision: None,
            }],
            with_line_numbers: None,
            max_result_bytes: None,
        }],
        policy: None,
        acceptance: Some(ChadexTaskAcceptance {
            require_validation_success: Some(false),
            require_review: Some(false),
        }),
        created_at_ms: now_ms(),
    };
    assert!(persist_task_recovery_plan_at(&root, &plan));
    let result = runtime
        .chadex_task_recovery(
            resolved(),
            Some(ID.into()),
            "retry".into(),
            None,
            sessions::SessionTransport::Api,
        )
        .await;
    assert!(!result.success); // no connected runner; identity still exists
    assert_ne!(result.output["execution_id"], ID);
    assert_eq!(result.output["previous_execution_id"], ID);
    assert_eq!(result.output["execution_state"], "failed");
    assert_eq!(runtime.chadex_tasks.recovered(ID).unwrap(), old);
    let new_id = result.output["execution_id"].as_str().unwrap();
    let receipt: Value = serde_json::from_slice(
        &fs::read(root.join("executions").join(new_id).join("receipt.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["execution"]["previous_execution_id"], ID);
}

#[test]
fn queued_crash_closes_canonical_state_without_admitting_a_duplicate() {
    let root = tempfile::tempdir().unwrap();
    let control = control(root.path());
    let snapshot = control.snapshot.lock().unwrap().clone();
    let state_dir = root.path().join("state");
    fs::create_dir_all(&state_dir).unwrap();
    write_private_file(
        &state_dir.join(format!("{ID}.json")),
        &serde_json::to_vec(&compact_task_projection(&snapshot)).unwrap(),
    )
    .unwrap();
    assert!(load_recovered_task_projections_at(root.path()).is_empty());
    drop(snapshot);
    drop(control);
    let recovered = load_recovered_task_projections_at(root.path());
    assert_eq!(recovered[ID]["execution_state"], "interrupted");
    assert_eq!(recovered[ID]["status"], "interrupted");
    assert!(Execution::claim(root.path(), ID, None).is_err());
}

#[test]
fn reconciliation_cas_rejection_is_known_but_lost_apply_result_is_unknown() {
    assert!(!uncertain_workspace_error(
        "execution_workspace_source_changed"
    ));
    assert!(!uncertain_workspace_error(
        "execution_workspace_reconciliation_conflict"
    ));
    assert!(uncertain_workspace_error("runner_timeout"));
    assert!(uncertain_workspace_error(
        "execution_workspace_apply_failed"
    ));
    let root = tempfile::tempdir().unwrap();
    let control = control(root.path());
    update_snapshot(&control, |s| {
        s.status = "running".into();
        s.execution.effects_possible = true;
        s.execution.outcome_uncertain = uncertain_workspace_error("runner_timeout");
    })
    .unwrap();
    update_snapshot(&control, |s| {
        s.status = "blocked".into();
        s.failure = Some(json!({"kind":"workspace_source_overlap", "category":"source_overlap"}));
    })
    .unwrap();
    assert_eq!(task_value(&control).unwrap()["execution_state"], "unknown");
}
